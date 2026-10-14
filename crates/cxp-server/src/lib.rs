//! `cxp-server` — one binary, one port, every surface.
//!
//! ```text
//!   /healthz /readyz                  health    no database on the liveness path
//!   /.well-known/oauth-protected-resource
//!                                     cxp-http  open; every 401 points at it
//!   /mcp                              cxp-mcp   agents: bearer tokens from the platform
//!   /api/v1/…                         cxp-api   the console: bearer tokens from the platform
//!   /platform/webhooks                cxp-api   the platform's signed lifecycle events
//!   everything else                   JSON 404
//! ```
//!
//! Every request body, on every surface, is capped at
//! [`cxp_api::MAX_BODY_BYTES`] by one layer on the composed router. axum's
//! `DefaultBodyLimit` reaches only axum extractors, and only on the routes it
//! is layered on: `/mcp` is a `route_service` outside cxp-api's, whose body
//! cxp-mcp reads through axum's `Bytes` extractor, so without this layer
//! `/mcp` would take bodies up to axum's 2 MiB default rather than this
//! service's cap.
//!
//! Assembly is a library function so a test can build the whole router: axum
//! panics on a route registered twice, and that is better found by a test than
//! by a deploy. So is the process lifecycle ([`run`]): `main` only reads the
//! configuration, binds the port, and waits for a signal.

pub mod boot;
pub mod config;
pub mod health;
pub mod install_page;

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use axum::response::{IntoResponse, Response};
use axum::{middleware, Router};
use cxp_core::usage::Meter;
use http::{StatusCode, Uri};
use otto_resource::{ClientConfig, PlatformClient};
use otto_tenant::Db;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub use boot::{run, run_with, Boot, PlatformCheck};
pub use config::{Config, LogFormat};

/// The one client every platform call goes through (it holds the caches).
///
/// Built with [`Config::resource_uri`] as the audience: the client is what
/// refuses a token minted for another service, so this is the only place the
/// audience is set.
pub fn platform_client(config: &Config) -> Result<Arc<PlatformClient>> {
    let client = PlatformClient::new(ClientConfig::new(
        &config.platform_url,
        &config.resource_uri,
        &config.introspection_secret,
    ))
    .context("CXP_PLATFORM_URL is not usable")?;
    Ok(Arc::new(client))
}

/// The one meter the MCP tools and `POST /api/v1/surveys` charge through.
pub fn meter(platform: Arc<PlatformClient>, config: &Config) -> Arc<Meter> {
    Arc::new(Meter::new(
        platform,
        config.enforce_quotas,
        config.upgrade_url(),
    ))
}

/// Every surface on one router. `platform_check` is what boot's credential
/// check found ([`boot::check_platform`]); `/readyz` reports it. Fails only
/// on a configuration that would leave `/mcp` open to any `Host` (see
/// [`Config::allowed_hosts`]).
pub fn router(
    db: Db,
    platform: Arc<PlatformClient>,
    meter: Arc<Meter>,
    config: &Config,
    platform_check: PlatformCheck,
) -> Result<Router> {
    // cxp-mcp refuses an empty host list itself (rmcp reads one as "accept
    // any"); the only way to get one here is a public URL with no host.
    let mcp = cxp_mcp::router(
        db.clone(),
        platform.clone(),
        meter.clone(),
        mcp_config(config),
    )
    .map_err(|e| anyhow!("{e}: CXP_PUBLIC_URL must have a host"))?;
    let api = cxp_api::router(cxp_api::AppState::new(
        db.clone(),
        platform,
        meter,
        cxp_api::Config {
            public_url: config.public_url.clone(),
            webhook_secret: config.platform_webhook_secret.clone(),
        },
    ));
    Ok(health::router(db, platform_check)
        // Open, and not part of either surface: every `401` points at it.
        .merge(cxp_http::protected_resource_router(
            &config.resource_uri,
            &config.platform_url,
        ))
        .merge(mcp)
        .merge(api)
        .fallback(not_found)
        .layer(RequestBodyLimitLayer::new(cxp_api::MAX_BODY_BYTES))
        // Outside the limit: its own 413 is plain text, rewritten here to the
        // `{error, code}` shape every other error has.
        .layer(middleware::map_response(json_payload_too_large))
        // One INFO span per request with the method and path only: headers
        // carry bearer tokens and webhook signatures, and a query string can
        // carry a customer search.
        .layer(TraceLayer::new_for_http().make_span_with(
            |req: &http::Request<axum::body::Body>| {
                tracing::info_span!("request", method = %req.method(), path = %req.uri().path())
            },
        )))
}

/// The MCP surface's settings, from the server's.
fn mcp_config(config: &Config) -> cxp_mcp::Config {
    cxp_mcp::Config {
        public_url: config.public_url.clone(),
        allowed_hosts: config.allowed_hosts(),
        allowed_origins: config.allowed_origins.clone(),
    }
}

/// A plain-text `413` (the body-limit layer's own, sent when `Content-Length`
/// is over the cap) becomes the JSON `payload_too_large` error. A `413` a
/// handler already answered in JSON passes through untouched.
async fn json_payload_too_large(res: Response) -> Response {
    let plain = res
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE && plain {
        return cxp_api::ApiError::payload_too_large().into_response();
    }
    res
}

/// The same `{error, code}` shape as every `/api/v1` error.
async fn not_found(uri: Uri) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": format!(
                "no route serves {}. Agents: the MCP endpoint is /mcp (see \
                 /.well-known/oauth-protected-resource). The REST API is under /api/v1.",
                uri.path()
            ),
            "code": "not_found",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt;

    /// The whole router over a database that is never reached: nothing tested
    /// here may touch it except `/readyz`, which must notice.
    fn app() -> Router {
        app_with(PlatformCheck::Accepted)
    }

    /// [`app`] as if boot's platform check had found `check`.
    fn app_with(check: PlatformCheck) -> Router {
        let mut config = Config::for_test();
        // Nothing listens on port 1: a probe is refused at once.
        config.database_url = "postgres://cxp:cxp@127.0.0.1:1/none".into();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy(&config.database_url)
            .unwrap();
        let platform = platform_client(&config).unwrap();
        router(
            Db::from_pool(pool),
            platform.clone(),
            meter(platform, &config),
            &config,
            check,
        )
        .unwrap()
    }

    async fn call(method: &str, path: &str) -> (StatusCode, http::HeaderMap, serde_json::Value) {
        send(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "cxp.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn send(req: Request<Body>) -> (StatusCode, http::HeaderMap, serde_json::Value) {
        send_to(app(), req).await
    }

    async fn send_to(
        app: Router,
        req: Request<Body>,
    ) -> (StatusCode, http::HeaderMap, serde_json::Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or_default(),
        )
    }

    #[tokio::test]
    async fn the_whole_router_assembles_and_discovery_names_the_platform() {
        let (status, _, body) = call("GET", "/.well-known/oauth-protected-resource").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["resource"], "https://cxp.example.com/mcp");
        assert_eq!(
            body["authorization_servers"][0],
            "https://platform.example.com"
        );
        assert_eq!(
            body["scopes_supported"],
            serde_json::json!(["cxp:read", "cxp:write"])
        );
    }

    #[tokio::test]
    async fn unauthenticated_calls_are_told_where_to_authenticate() {
        for (method, path) in [("POST", "/mcp"), ("GET", "/api/v1/customers")] {
            let (status, headers, _) = call(method, path).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
            let challenge = headers["www-authenticate"].to_str().unwrap();
            assert!(
                challenge.contains(
                    "resource_metadata=\"https://cxp.example.com/.well-known/oauth-protected-resource\""
                ),
                "{challenge}"
            );
            // ...and what it points at is served, open, by this router.
            let (status, _, _) = call("GET", cxp_http::PROTECTED_RESOURCE_PATH).await;
            assert_eq!(status, StatusCode::OK);
            assert!(
                challenge.contains(cxp_http::PROTECTED_RESOURCE_PATH),
                "{challenge}"
            );
        }
    }

    #[tokio::test]
    async fn liveness_never_touches_the_database_and_readiness_does() {
        let (status, _, body) = call("GET", "/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");

        let (status, _, body) = call("GET", "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "unready");
        assert!(
            body["reason"].as_str().unwrap().contains("database"),
            "{body}"
        );
    }

    /// A credential the platform refused at boot, or a platform URL that is
    /// not the platform, makes the replica unready, so a deploy with a bad
    /// secret fails its health check; an unreachable platform does not.
    #[tokio::test]
    async fn readiness_reports_a_rejected_or_misconfigured_platform() {
        for (check, reason) in [
            (PlatformCheck::Rejected, "platform credential rejected"),
            (
                PlatformCheck::Misconfigured("HTTP 404".into()),
                "platform misconfigured",
            ),
        ] {
            let req = Request::get("/readyz")
                .header("host", "cxp.example.com")
                .body(Body::empty())
                .unwrap();
            let (status, _, body) = send_to(app_with(check), req).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{reason}");
            assert_eq!(body["status"], "unready");
            assert_eq!(body["reason"], reason);
        }
        // Unreachable at boot: readiness is the database's answer alone
        // (here, unready because the database is unreachable too).
        let req = Request::get("/readyz").body(Body::empty()).unwrap();
        let (_, _, body) =
            send_to(app_with(PlatformCheck::Unreachable("refused".into())), req).await;
        assert!(
            body["reason"].as_str().unwrap().contains("database"),
            "{body}"
        );
    }

    /// `/mcp`'s body is read by rmcp, not an axum extractor, so only the
    /// router-wide layer caps it; every other surface is capped the same way.
    #[tokio::test]
    async fn an_oversized_body_is_a_json_413_on_every_surface() {
        let big = vec![b' '; cxp_api::MAX_BODY_BYTES + 1];
        for path in ["/mcp", "/api/v1/customers", "/platform/webhooks", "/nope"] {
            let (status, _, body) = send(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", "cxp.example.com")
                    .header("content-type", "application/json")
                    .header("content-length", big.len())
                    .body(Body::from(big.clone()))
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
            assert_eq!(body["code"], "payload_too_large", "{path}");
        }
    }

    #[tokio::test]
    async fn anything_else_is_a_json_404() {
        let (status, _, body) = call("GET", "/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        assert!(body["error"].as_str().unwrap().contains("/nope"), "{body}");
    }

    #[test]
    fn mcp_gets_the_public_host_and_the_extras() {
        let mut config = Config::for_test();
        config.extra_allowed_hosts = vec!["otto-cxp.fly.dev".into()];
        let mcp = mcp_config(&config);
        assert_eq!(mcp.allowed_hosts, ["cxp.example.com", "otto-cxp.fly.dev"]);
        assert_eq!(mcp.public_url, config.public_url);
    }

    #[tokio::test]
    async fn mcp_refuses_to_start_open_to_any_host() {
        let mut config = Config::for_test();
        config.public_url = "not a url".into();
        assert!(mcp_config(&config).allowed_hosts.is_empty());
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://cxp:cxp@127.0.0.1:1/none")
            .unwrap();
        let platform = platform_client(&Config::for_test()).unwrap();
        let err = router(
            Db::from_pool(pool),
            platform.clone(),
            meter(platform, &config),
            &config,
            PlatformCheck::Accepted,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("accept any"), "{err}");
        assert!(err.contains("CXP_PUBLIC_URL"), "{err}");
    }
}
