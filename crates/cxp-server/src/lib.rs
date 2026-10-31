//! `cxp-server` — one binary, one port, every surface.
//!
//! ```text
//!   /healthz /readyz                  health    no database on the liveness path
//!   /.well-known/oauth-protected-resource
//!                                     cxp-http  open; every 401 points at it
//!   /mcp                              cxp-mcp   agents: bearer tokens from the platform
//!   /api/v1/…                         cxp-api   bearer tokens, or the console's session cookie
//!   /auth/…                           cxp-http  the console's sign-in through the platform
//!   /platform/webhooks                cxp-api   the platform's signed lifecycle events
//!   /  (GET/HEAD, browsers only)      install   the connector install page
//!   GET/HEAD /console/…               the built console (CXP_CONSOLE_DIR): a file, or
//!                                     index.html for a browser navigation; /console
//!                                     redirects to /console/
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
pub mod secret_env;

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use axum::body::Body;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use axum::{middleware, Router};
use cxp_core::usage::Meter;
use cxp_http::console::{Console, ConsoleKeys, ConsoleSettings};
use http::{header, HeaderValue, Method, StatusCode};
use install_page::InstallPage;
use otto_resource::{ClientConfig, PlatformClient};
use otto_tenant::Db;
use tower::ServiceExt;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub use boot::{migrate, run, run_with, Boot, PlatformCheck};
pub use config::{Config, LogFormat, MigrateConfig};

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

/// The one meter the billable MCP tools charge through.
pub fn meter(platform: Arc<PlatformClient>, config: &Config) -> Arc<Meter> {
    Arc::new(Meter::new(platform, config.upgrade_url()))
}

/// Every surface on one router. `readiness` is what boot's credential check
/// found ([`boot::check_platform`]), with the database-role alarm the
/// sweeper raises ([`health::Readiness`]); `/readyz` reports both. A bare
/// [`PlatformCheck`] converts, with an alarm nothing raises. Fails on a
/// configuration that would leave `/mcp` open to any `Host` (see
/// [`Config::allowed_hosts`]) or a platform URL the install page cannot take
/// an origin from.
pub fn router(
    db: Db,
    platform: Arc<PlatformClient>,
    meter: Arc<Meter>,
    search: cxp_core::search::SearchLimiter,
    config: &Config,
    readiness: impl Into<health::Readiness>,
) -> Result<Router> {
    // `search` is one limiter for the whole process, so `/api` and `/mcp`
    // together hold at most half the pool with searches (#42).
    // cxp-mcp refuses an empty host list itself (rmcp reads one as "accept
    // any"); the only way to get one here is a public URL with no host.
    let mcp = cxp_mcp::router(
        db.clone(),
        platform.clone(),
        meter.clone(),
        search.clone(),
        mcp_config(config),
    )
    .map_err(|e| anyhow!("{e}: CXP_PUBLIC_URL must have a host"))?;
    let console = console(db.clone(), platform.clone(), config)?;
    let mut state = cxp_api::AppState::new(
        db.clone(),
        platform,
        search,
        cxp_api::Config {
            public_url: config.public_url.clone(),
            webhook_secret: config.platform_webhook_secret.clone(),
        },
    );
    // The session cookie is a credential on /api only; /mcp never sees it.
    if let Some(console) = &console {
        state = state.with_console(console.clone());
    }
    let api = cxp_api::router(state);
    let assets = ConsoleAssets::find(&config.console_dir);
    let hsts = config.public_url.starts_with("https://");
    // Rendered once: a bad platform URL fails boot, not a request.
    let page = InstallPage::new(&config.resource_uri, &config.platform_url)?;
    let redirect = Arc::new(OldHosts {
        hosts: config.redirect_hosts.clone(),
        base: config.public_url.trim_end_matches('/').to_string(),
    });
    Ok(health::router(db, readiness)
        // Open, and not part of either surface: every `401` points at it.
        .merge(cxp_http::protected_resource_router(
            &config.resource_uri,
            &config.platform_url,
        ))
        .merge(mcp)
        .merge(api)
        // Mounted with the console off too, so /auth is never the SPA.
        .merge(cxp_http::console::router(console))
        .fallback(move |req: Request| fallback(page.clone(), assets.clone(), req))
        .layer(RequestBodyLimitLayer::new(cxp_api::MAX_BODY_BYTES))
        .layer(middleware::map_response(move |mut res: Response| async move {
            // Every response, JSON errors and console files alike: never
            // sniffed into another type.
            res.headers_mut()
                .entry(header::X_CONTENT_TYPE_OPTIONS)
                .or_insert(HeaderValue::from_static("nosniff"));
            if hsts {
                res.headers_mut()
                    .entry(header::STRICT_TRANSPORT_SECURITY)
                    .or_insert(HeaderValue::from_static(HSTS));
            }
            res
        }))
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
        ))
        // Outermost, so it runs before the trace span, host allow-listing,
        // and auth: an old host answers nothing but the redirect.
        .layer(middleware::from_fn_with_state(redirect, redirect_old_hosts)))
}

/// The hosts that only redirect (`CXP_REDIRECT_HOSTS`) and where to.
struct OldHosts {
    hosts: Vec<String>,
    /// `CXP_PUBLIC_URL` without a trailing `/`: the only scheme and host a
    /// `Location` ever carries.
    base: String,
}

/// `308` every request for an old host to the same path and query under the
/// public URL, for every method, with an empty body.
///
/// The `Location` is the public URL followed by the request's raw
/// path-and-query, byte for byte: nothing is decoded and re-encoded, and
/// neither the scheme nor the host of the request is used.
async fn redirect_old_hosts(
    axum::extract::State(old): axum::extract::State<Arc<OldHosts>>,
    req: Request,
    next: middleware::Next,
) -> Response {
    if old.hosts.is_empty() || !is_old_host(&old.hosts, &req) {
        return next.run(req).await;
    }
    let target = req.uri().path_and_query().map_or("/", |pq| pq.as_str());
    let target = if target.starts_with('/') { target } else { "/" };
    let location = format!("{}{target}", old.base);
    match HeaderValue::try_from(location) {
        Ok(location) => {
            let mut res = Response::new(Body::empty());
            *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
            res.headers_mut().insert(header::LOCATION, location);
            res
        }
        // A request URI is visible ASCII, so this does not happen; if it
        // ever did, the root of the public URL beats serving the old host.
        Err(_) => {
            let mut res = Response::new(Body::empty());
            *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
            if let Ok(root) = HeaderValue::try_from(format!("{}/", old.base)) {
                res.headers_mut().insert(header::LOCATION, root);
            }
            res
        }
    }
}

/// Whether the request's `Host` (HTTP/2's `:authority` when there is no
/// header), without its port, is one of `hosts` (all lowercase).
fn is_old_host(hosts: &[String], req: &Request) -> bool {
    let authority = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    let Some(authority) = authority else {
        return false;
    };
    let host = authority.split(':').next().unwrap_or_default();
    let host = host.strip_suffix('.').unwrap_or(host);
    hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
}

/// The web console's sign-in, when `CXP_CONSOLE_CLIENT_ID` is set.
fn console(db: Db, platform: Arc<PlatformClient>, config: &Config) -> Result<Option<Console>> {
    let Some(c) = &config.console else {
        tracing::info!("console sign-in is off (CXP_CONSOLE_CLIENT_ID is unset)");
        return Ok(None);
    };
    let keys = ConsoleKeys::derive(&c.encryption_key).map_err(|e| anyhow!(e))?;
    Ok(Some(Console::new(
        db,
        platform,
        ConsoleSettings {
            client_id: c.client_id.clone(),
            keys,
            public_url: config.public_url.clone(),
            platform_url: config.platform_url.clone(),
            resource_uri: config.resource_uri.clone(),
        },
    )))
}

/// Where the built console is served: `/console/`, never `/`.
const CONSOLE_PREFIX: &str = "/console";

/// The console's `Content-Security-Policy`. Scripts, connections, and frames
/// are this origin's only. `style-src` allows inline styles because the
/// console's toast and dialog libraries inject `<style>` elements at runtime;
/// no inline script is allowed.
const CONSOLE_CSP: &str = "default-src 'self'; script-src 'self'; connect-src 'self'; \
                           style-src 'self' 'unsafe-inline'; object-src 'none'; \
                           base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// `Strict-Transport-Security` on every response when the public URL is
/// `https`: the session cookie must never travel over plain http.
const HSTS: &str = "max-age=31536000";

/// The built console: its files, and `index.html` for client-side routes.
#[derive(Clone)]
struct ConsoleAssets {
    files: ServeDir,
    index: ServeFile,
}

impl ConsoleAssets {
    /// The console in `dir`, or `None` (logged) when there is no
    /// `index.html` there to serve.
    fn find(dir: &Path) -> Option<Self> {
        let index = dir.join("index.html");
        if !index.is_file() {
            tracing::warn!(dir = %dir.display(),
                "no built console at CXP_CONSOLE_DIR (no index.html); /console answers 404");
            return None;
        }
        Some(Self {
            // A directory is never listed or redirected to: only files.
            files: ServeDir::new(dir).append_index_html_on_directories(false),
            index: ServeFile::new(index),
        })
    }
}

/// Whether the request is a browser navigation: it accepts HTML.
fn wants_html(req: &Request) -> bool {
    req.headers()
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.contains("text/html"))
}

/// Whether a path under the console names a dotfile or a dot segment,
/// plainly or percent-encoded. The check runs on the decoded path, split on
/// both separators, because the file server decodes before it splits:
/// `x%2F.env` reaches it as `x/.env`. A path that does not decode cleanly
/// counts as one. Never served, whatever is on disk.
fn has_dot_segment(path: &str) -> bool {
    let Some(decoded) = percent_decode(path) else {
        return true;
    };
    decoded.split(['/', '\\']).any(|seg| seg.starts_with('.'))
}

/// `path` with every `%XX` escape decoded, or `None` if an escape is
/// malformed or the result is not UTF-8.
fn percent_decode(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Everything no route serves. Under `/console/`, a `GET` or `HEAD` is a file
/// of the built console, or `index.html` for a browser navigation the console
/// routes itself; `/console` redirects to `/console/`. Everything else is
/// the JSON `404` ([`fallback`] has already answered `GET`/`HEAD /`), so an
/// agent or script that guessed a path still gets a machine-readable answer.
async fn console_or_not_found(assets: Option<ConsoleAssets>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let Some(assets) = assets else {
        return not_found(&path);
    };
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return not_found(&path);
    }
    if path == CONSOLE_PREFIX {
        let to = match req.uri().query() {
            Some(q) => format!("{CONSOLE_PREFIX}/?{q}"),
            None => format!("{CONSOLE_PREFIX}/"),
        };
        return axum::response::Redirect::permanent(&to).into_response();
    }
    let Some(rest) = path
        .strip_prefix(CONSOLE_PREFIX)
        .filter(|r| r.starts_with('/'))
    else {
        return not_found(&path);
    };
    if has_dot_segment(rest) {
        return not_found(&path);
    }
    let html = wants_html(&req);
    let (mut parts, _) = req.into_parts();
    let Ok(uri) = rest.parse::<http::Uri>() else {
        return not_found(&path);
    };
    parts.uri = uri;
    let file = match assets
        .files
        .oneshot(Request::from_parts(parts.clone(), Body::empty()))
        .await
    {
        Ok(res) => res.map(Body::new),
        Err(e) => match e {},
    };
    let status = file.status();
    let mut res = if status.is_success() || status == StatusCode::NOT_MODIFIED {
        file
    } else if status.is_server_error() {
        tracing::error!(%status, path = %path, "could not serve a console file");
        return internal_error();
    } else if html && status == StatusCode::NOT_FOUND {
        match assets
            .index
            .oneshot(Request::from_parts(parts, Body::empty()))
            .await
        {
            Ok(res) if res.status().is_server_error() => {
                tracing::error!(path = %path, "could not serve the console's index.html");
                return internal_error();
            }
            Ok(res) => res.map(Body::new),
            Err(e) => match e {},
        }
    } else {
        // A miss that is not a navigation, or anything ServeDir would answer
        // with a redirect or another non-JSON shape.
        return not_found(&path);
    };
    let headers = res.headers_mut();
    // Hashed build output never changes under its name; everything else
    // (index.html, the service worker, the manifest) must be revalidated, or
    // a deploy is invisible until a cache expires.
    let cache = if rest.starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    // The console acts with an ambient session cookie: never inside a frame.
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONSOLE_CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    res
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

/// Everything no route serves. A browser opening `/` gets the connector
/// install page ([`install_page`]); serving it here rather than from a `/`
/// route keeps `POST /` a 404 instead of a 405. Any other `GET`/`HEAD /` gets
/// the JSON 404 with the page's `Vary`, since `Accept` and `Authorization`
/// chose between the two. Anything else goes to [`console_or_not_found`].
async fn fallback(page: InstallPage, assets: Option<ConsoleAssets>, req: Request) -> Response {
    let head = *req.method() == Method::HEAD;
    if req.uri().path() == "/" && (head || *req.method() == Method::GET) {
        if install_page::wants_page(req.headers()) {
            return page.response(head);
        }
        let mut res = not_found("/");
        res.headers_mut()
            .insert(header::VARY, HeaderValue::from_static(install_page::VARY));
        return res;
    }
    console_or_not_found(assets, req).await
}

/// The same `{error, code}` shape as every `/api/v1` error.
fn not_found(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": format!(
                "no route serves {path}. Agents: the MCP endpoint is /mcp (see \
                 /.well-known/oauth-protected-resource). The REST API is under /api/v1."
            ),
            "code": "not_found",
        })),
    )
        .into_response()
}

/// The `{error, code}` shape for a failure of the server's own.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "error": cxp_core::error::INTERNAL_MESSAGE,
            "code": "internal",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, Bytes};
    use http::Request;
    use tower::ServiceExt;

    /// The whole router over a database that is never reached: nothing tested
    /// here may touch it except `/readyz`, which must notice.
    fn app() -> Router {
        app_with(PlatformCheck::Accepted)
    }

    /// [`app`] as if boot's platform check had found `check`.
    fn app_with(check: PlatformCheck) -> Router {
        app_from(Config::for_test(), check)
    }

    /// The whole router over `config`, with an unreachable database.
    fn app_from(mut config: Config, check: PlatformCheck) -> Router {
        // Nothing listens on port 1: a probe is refused at once.
        config.database_url = "postgres://cxp:cxp@127.0.0.1:1/none".into();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy(&config.database_url)
            .unwrap();
        let platform = platform_client(&config).unwrap();
        router(
            cxp_core::tenant_db(pool),
            platform.clone(),
            meter(platform, &config),
            cxp_core::search::SearchLimiter::for_pool(config.db_max_connections),
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

    /// The response with its body as bytes: [`send_to`] parses JSON and
    /// defaults when it cannot, so it can check neither HTML nor exact bytes.
    async fn send_raw(app: Router, req: Request<Body>) -> (StatusCode, http::HeaderMap, Bytes) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, headers, bytes)
    }

    /// What every browser sends on a navigation.
    const BROWSER_ACCEPT: &str =
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,*/*;q=0.8";

    fn request(method: &str, path: &str, headers: &[(&str, &str)]) -> Request<Body> {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "cxp.example.com");
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        req.body(Body::empty()).unwrap()
    }

    /// The JSON 404 the fallback has always given `/`, byte for byte.
    async fn root_404_bytes() -> Bytes {
        axum::body::to_bytes(not_found("/").into_body(), 1 << 20)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_browser_at_the_root_gets_the_install_page() {
        let mut config = Config::for_test();
        config.resource_uri = "https://cxp.example.com/custom/mcp".into();
        let (status, headers, body) = send_raw(
            app_from(config, PlatformCheck::Accepted),
            request("GET", "/", &[("accept", BROWSER_ACCEPT)]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "text/html; charset=utf-8");
        assert_eq!(headers["cache-control"], "public, max-age=3600");
        assert_eq!(headers["vary"], "Accept, Authorization");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
        assert_eq!(
            headers["content-security-policy"],
            install_page::CONTENT_SECURITY_POLICY
        );
        let html = std::str::from_utf8(&body).unwrap();
        assert!(
            html.contains("<title>Savvagent CXP connector</title>"),
            "{html}"
        );
        assert!(
            html.contains("<code id=\"address\">https://cxp.example.com/custom/mcp</code>"),
            "the address comes from CXP_RESOURCE_URI"
        );
        assert!(html.contains("href=\"https://platform.example.com/\""));
        // A query string does not change the path.
        let (status, _, _) = send_raw(
            app(),
            request("GET", "/?from=console", &[("accept", BROWSER_ACCEPT)]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn head_at_the_root_gets_the_headers_and_no_body() {
        let (status, headers, body) =
            send_raw(app(), request("HEAD", "/", &[("accept", BROWSER_ACCEPT)])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-type"], "text/html; charset=utf-8");
        assert!(body.is_empty(), "{body:?}");
    }

    /// curl (`*/*`), agents (`application/json`), and anything presenting
    /// credentials keep exactly the JSON 404 they got before the page, now
    /// with the page's `Vary`: both answers share one URL, so a shared cache
    /// that stored this 404 must not hand it to a browser, nor the page to a
    /// program (#32).
    #[tokio::test]
    async fn programs_at_the_root_still_get_the_json_404() {
        let expected = root_404_bytes().await;
        let cases: [&[(&str, &str)]; 7] = [
            &[],
            &[("accept", "*/*")],
            &[("accept", "application/json")],
            &[("accept", "text/*")],
            &[("accept", "text/html;q=0")],
            &[
                ("accept", BROWSER_ACCEPT),
                ("authorization", "Bearer otto_at_x"),
            ],
            &[("accept", BROWSER_ACCEPT), ("authorization", "")],
        ];
        for headers in cases {
            let (status, h, body) = send_raw(app(), request("GET", "/", headers)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{headers:?}");
            assert_eq!(h["content-type"], "application/json", "{headers:?}");
            assert_eq!(h["vary"], "Accept, Authorization", "{headers:?}");
            assert_eq!(body, expected, "{headers:?}");
        }
        // HEAD chooses between the same two answers; a query string does
        // not change the path.
        for (method, path) in [("HEAD", "/"), ("GET", "/?from=agent")] {
            let (status, h, body) =
                send_raw(app(), request(method, path, &[("accept", "*/*")])).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
            assert_eq!(h["content-type"], "application/json", "{method} {path}");
            assert_eq!(h["vary"], "Accept, Authorization", "{method} {path}");
            if method == "GET" {
                assert_eq!(body, expected, "{method} {path}");
            }
        }
    }

    /// Only `GET`/`HEAD /` changed: other methods on `/` and every other
    /// path are the JSON 404 even for a browser, with no `Vary`, since
    /// nothing in the request chooses their answer.
    #[tokio::test]
    async fn other_methods_and_paths_stay_json_404_for_a_browser() {
        for accept in [BROWSER_ACCEPT, "*/*"] {
            for (method, path) in [
                ("POST", "/"),
                ("PUT", "/"),
                ("DELETE", "/"),
                ("GET", "/nope"),
                ("GET", "/index.html"),
                ("GET", "//"),
            ] {
                let (status, h, body) =
                    send_raw(app(), request(method, path, &[("accept", accept)])).await;
                let case = format!("{method} {path} ({accept})");
                assert_eq!(status, StatusCode::NOT_FOUND, "{case}");
                assert_eq!(h["content-type"], "application/json", "{case}");
                assert!(h.get("vary").is_none(), "{case}");
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["code"], "not_found", "{case}");
            }
        }
    }

    /// A browser that wanders onto `/mcp` or `/api` is told to authenticate,
    /// exactly like any other caller.
    #[tokio::test]
    async fn a_browser_on_the_protected_surfaces_still_gets_401() {
        for (method, path) in [("POST", "/mcp"), ("GET", "/api/v1/customers")] {
            let (status, headers, _) =
                send_raw(app(), request(method, path, &[("accept", BROWSER_ACCEPT)])).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
            let challenge = headers["www-authenticate"].to_str().unwrap();
            assert!(
                challenge.contains(cxp_http::PROTECTED_RESOURCE_PATH),
                "{challenge}"
            );
        }
    }

    fn redirecting_app() -> Router {
        let mut config = Config::for_test();
        config.redirect_hosts = vec!["old.example.com".into()];
        app_from(config, PlatformCheck::Accepted)
    }

    #[tokio::test]
    async fn an_old_host_gets_a_308_to_the_public_url_on_every_method_and_path() {
        for (method, target, host, location) in [
            (
                "GET",
                "/mcp?session=a%2Fb&x=1",
                "old.example.com",
                "https://cxp.example.com/mcp?session=a%2Fb&x=1",
            ),
            (
                "POST",
                "/mcp?x=1",
                "OLD.example.com:8080",
                "https://cxp.example.com/mcp?x=1",
            ),
            (
                "DELETE",
                "/api/v1/customers/1",
                "old.example.com.",
                "https://cxp.example.com/api/v1/customers/1",
            ),
            (
                "GET",
                "/.well-known/oauth-protected-resource",
                "old.example.com",
                "https://cxp.example.com/.well-known/oauth-protected-resource",
            ),
            (
                "GET",
                "/healthz",
                "old.example.com",
                "https://cxp.example.com/healthz",
            ),
            (
                "GET",
                "/console/",
                "old.example.com",
                "https://cxp.example.com/console/",
            ),
            ("GET", "/", "old.example.com", "https://cxp.example.com/"),
            // Raw: dot segments, doubled slashes, and escapes are not touched.
            (
                "GET",
                "//evil.example/%2e%2e/a%20b?q=%zz",
                "old.example.com",
                "https://cxp.example.com//evil.example/%2e%2e/a%20b?q=%zz",
            ),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(target)
                .header("host", host)
                .header("authorization", "Bearer nope")
                .body(Body::from("ignored"))
                .unwrap();
            let (status, headers, body) = send_raw(redirecting_app(), req).await;
            assert_eq!(status, StatusCode::PERMANENT_REDIRECT, "{method} {target}");
            assert_eq!(headers[header::LOCATION], location, "{method} {target}");
            assert!(body.is_empty(), "{method} {target}");
        }
    }

    #[tokio::test]
    async fn a_request_cannot_choose_where_the_redirect_goes() {
        let req = Request::builder()
            .uri("http://evil.example/mcp")
            .header("host", "old.example.com")
            .header("x-forwarded-host", "evil.example")
            .header("x-forwarded-proto", "http")
            .body(Body::empty())
            .unwrap();
        let (status, headers, _) = send_raw(redirecting_app(), req).await;
        assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(headers[header::LOCATION], "https://cxp.example.com/mcp");
    }

    #[tokio::test]
    async fn other_hosts_are_not_redirected() {
        for host in [
            "cxp.example.com",
            "old.example.com.evil.example",
            "xold.example.com",
        ] {
            let req = request("GET", "/healthz", &[]);
            let (mut parts, body) = req.into_parts();
            parts.headers.insert("host", host.parse().unwrap());
            let req = Request::from_parts(parts, body);
            let (status, headers, _) = send_raw(redirecting_app(), req).await;
            assert_eq!(status, StatusCode::OK, "{host}");
            assert!(headers.get(header::LOCATION).is_none(), "{host}");
        }
        // With none configured (the default) nothing redirects.
        let (status, _, _) = call("GET", "/healthz").await;
        assert_eq!(status, StatusCode::OK);
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

    /// A built console in a fresh directory: `index.html`, one hashed asset,
    /// and the service worker.
    fn console_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cxp-console-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><title>CXP</title>").unwrap();
        std::fs::write(dir.join("assets/app-abc123.js"), "console.log(1)").unwrap();
        std::fs::write(dir.join("sw.js"), "self.x = 1").unwrap();
        dir
    }

    async fn get_with(
        app: Router,
        path: &str,
        accept: Option<&str>,
    ) -> (StatusCode, http::HeaderMap, String) {
        let mut req = Request::get(path).header("host", "cxp.example.com");
        if let Some(a) = accept {
            req = req.header("accept", a);
        }
        let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            headers,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }

    const NAVIGATION: &str = "text/html,application/xhtml+xml,*/*;q=0.8";

    #[tokio::test]
    async fn the_built_console_is_served_under_console_with_an_index_fallback() {
        let dir = console_dir();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config"), "x").unwrap();
        std::fs::create_dir_all(dir.join("x")).unwrap();
        std::fs::write(dir.join("x/.env"), "SECRET=1").unwrap();
        let mut config = Config::for_test();
        config.console_dir = dir.clone();
        let app = app_from(config, PlatformCheck::Accepted);

        // A client-side route, navigated to: the console, with its headers.
        for path in [
            "/console/",
            "/console/tickets",
            "/console/customers/123?tab=x",
        ] {
            let (status, headers, body) = get_with(app.clone(), path, Some(NAVIGATION)).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("<title>CXP</title>"), "{path}: {body}");
            assert_eq!(headers["cache-control"], "no-cache", "{path}");
            assert_eq!(headers["x-frame-options"], "DENY", "{path}");
            assert_eq!(headers["content-security-policy"], CONSOLE_CSP, "{path}");
            assert_eq!(headers["strict-transport-security"], HSTS, "{path}");
        }
        for directive in [
            "default-src 'self'",
            "script-src 'self'",
            "connect-src 'self'",
            "object-src 'none'",
            "base-uri 'none'",
            "form-action 'self'",
            "frame-ancestors 'none'",
        ] {
            assert!(CONSOLE_CSP.contains(directive), "{directive}");
        }
        assert!(!CONSOLE_CSP.contains("script-src 'self' 'unsafe"));
        // /console itself: to /console/, the query kept.
        let (status, headers, _) = get_with(app.clone(), "/console", Some(NAVIGATION)).await;
        assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(headers["location"], "/console/");
        let (_, headers, _) = get_with(app.clone(), "/console?login_error=failed", None).await;
        assert_eq!(headers["location"], "/console/?login_error=failed");
        // Its files, to anyone.
        let (status, headers, body) =
            get_with(app.clone(), "/console/assets/app-abc123.js", None).await;
        assert_eq!((status, body.as_str()), (StatusCode::OK, "console.log(1)"));
        assert!(headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable"));
        let (status, headers, _) = get_with(app.clone(), "/console/sw.js", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-cache");

        // A browser at / gets the connector install page, never the console.
        let (status, headers, body) = get_with(app.clone(), "/", Some(NAVIGATION)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers["content-security-policy"],
            install_page::CONTENT_SECURITY_POLICY
        );
        assert!(!body.contains("<title>CXP</title>"), "{body}");

        // Outside /console/, not a navigation, a dotfile, a directory, or
        // under another prefix: the JSON 404, never the console's HTML.
        for (path, accept) in [
            ("/", None),
            ("/tickets", Some(NAVIGATION)),
            ("/index.html", Some(NAVIGATION)),
            ("/assets/app-abc123.js", None),
            ("/consoles/x", Some(NAVIGATION)),
            ("/nope", None),
            ("/nope", Some("application/json")),
            ("/console/nope", None),
            ("/console/assets/missing.js", None),
            ("/console/.env", None),
            ("/console/.env", Some(NAVIGATION)),
            ("/console/.git/config", None),
            ("/console/%2eenv", None),
            ("/console/x/.env", None),
            ("/console/x%2F.env", None),
            ("/console/x%2f.env", Some(NAVIGATION)),
            ("/console/x%5C.env", None),
            ("/console/assets", None),
            ("/console/assets/", None),
            ("/auth/nope", Some(NAVIGATION)),
            ("/.well-known/nope", Some(NAVIGATION)),
            ("/platform/nope", Some(NAVIGATION)),
            ("/mcp/nope", Some(NAVIGATION)),
            ("/healthz/nope", Some(NAVIGATION)),
        ] {
            let (status, headers, body) = get_with(app.clone(), path, accept).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} {accept:?}");
            assert!(
                headers["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("application/json"),
                "{path}"
            );
            assert!(body.contains("not_found"), "{path}: {body}");
            assert!(!body.contains("SECRET"), "{path}");
            assert_eq!(headers["x-content-type-options"], "nosniff", "{path}");
        }
        // /api answers for itself: authentication first, in JSON.
        let (status, _, body) = get_with(app.clone(), "/api/v1/nope", Some(NAVIGATION)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!body.contains("<title>"), "{body}");
        // A write is never the console.
        let res = app
            .clone()
            .oneshot(
                Request::post("/console/tickets")
                    .header("accept", NAVIGATION)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_a_built_console_console_paths_are_json_404s() {
        let mut config = Config::for_test();
        config.console_dir =
            std::env::temp_dir().join(format!("cxp-none-{}", uuid::Uuid::new_v4()));
        let app = app_from(config, PlatformCheck::Accepted);
        for path in ["/console", "/console/", "/console/tickets"] {
            let (status, _, body) = get_with(app.clone(), path, Some(NAVIGATION)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(body.contains("not_found"), "{path}");
        }
    }

    #[test]
    fn dot_segments_are_found_plain_and_encoded() {
        for p in [
            "/.env",
            "/a/.git/x",
            "/%2Eenv",
            "/x/%2e%2e/y",
            "/..",
            "/x%2F.env",
            "/x%2f%2egit/config",
            "/x%5C.env",
            "/x\\.env",
            "/bad%zz",
            "/bad%2",
            "/%ff",
        ] {
            assert!(has_dot_segment(p), "{p}");
        }
        for p in [
            "/",
            "/assets/app.js",
            "/a.b/c",
            "/sw.js",
            "/a%20b.js",
            "/a%2Fb.js",
        ] {
            assert!(!has_dot_segment(p), "{p}");
        }
    }

    #[tokio::test]
    async fn hsts_is_sent_only_for_an_https_public_url() {
        let (_, headers, _) = get_with(app(), "/healthz", None).await;
        assert_eq!(headers["strict-transport-security"], HSTS);
        // nosniff goes on every response, JSON included.
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let mut config = Config::for_test();
        config.public_url = "http://localhost:8080".into();
        config.resource_uri = "http://localhost:8080/mcp".into();
        let (_, headers, _) =
            get_with(app_from(config, PlatformCheck::Accepted), "/healthz", None).await;
        assert!(headers.get("strict-transport-security").is_none());
    }

    /// Merging with no console configured deploys safely: sign-in says it is
    /// off, rather than the console's HTML or a redirect that cannot work.
    #[tokio::test]
    async fn console_sign_in_is_off_by_default() {
        let (status, _, body) = call("GET", "/auth/login").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "console_login_disabled");
        let (status, _, body) = call("GET", "/auth/session").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["loginEnabled"], false);
    }

    #[tokio::test]
    async fn console_sign_in_is_on_with_a_client_id_and_key() {
        let mut config = Config::for_test();
        config.console = Some(config::ConsoleConfig {
            client_id: "cxp-console".into(),
            encryption_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into(),
        });
        let app = app_from(config, PlatformCheck::Accepted);
        let res = app
            .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let to = res.headers()["location"].to_str().unwrap();
        assert!(
            to.starts_with("https://platform.example.com/oauth/authorize?"),
            "{to}"
        );
        assert!(to.contains("client_id=cxp-console"), "{to}");
    }

    #[test]
    fn mcp_gets_the_public_host_and_the_extras() {
        let mut config = Config::for_test();
        config.extra_allowed_hosts =
            vec!["otto-cxp.fly.dev".into(), "otto-cxp.savvagent.com".into()];
        let mcp = mcp_config(&config);
        assert_eq!(
            mcp.allowed_hosts,
            [
                "cxp.example.com",
                "otto-cxp.fly.dev",
                "otto-cxp.savvagent.com"
            ]
        );
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
            cxp_core::tenant_db(pool),
            platform.clone(),
            meter(platform, &config),
            cxp_core::search::SearchLimiter::for_pool(config.db_max_connections),
            &config,
            PlatformCheck::Accepted,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("accept any"), "{err}");
        assert!(err.contains("CXP_PUBLIC_URL"), "{err}");
    }
}
