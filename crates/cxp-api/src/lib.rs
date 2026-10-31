//! `cxp-api` — otto-cxp's REST API (`/api/v1/...`, for the web console) and
//! the otto platform's signed lifecycle webhooks (`/platform/webhooks`). Every
//! route calls into `cxp-core`; no SQL lives here.
//!
//! ```text
//!   /api/v1/...            bearer token,  GET needs cxp:read (or cxp:write),
//!                          or the web     every other method cxp:write
//!                          console's
//!                          session cookie
//!   POST /platform/webhooks  Otto-Signature HMAC; no bearer token
//! ```
//!
//! The session cookie is accepted only when the binary passes a console
//! ([`AppState::with_console`]), only for a request with no bearer token, and
//! a cookie-authenticated write must come from this origin
//! (`cxp_http::console::same_origin`) or is `403 cross_site_request`. The
//! cookie's stored platform token is introspected exactly as a bearer is.
//!
//! Every `/api/v1` handler does the same five things in order: take the
//! [`Principal`](cxp_core::auth::Principal) the bearer middleware attached,
//! check its scope, do any platform lookup it needs (a catalog team id)
//! *before* opening a transaction so no pooled connection waits
//! on the network, open the transaction with
//! [`platform_events::begin_live`](cxp_core::platform_events::begin_live), and
//! commit after the one domain call.
//!
//! **Metering:** no route records usage. `POST /api/v1/surveys` is free, as
//! the MCP `create_survey` tool is; a route added for one of the billable
//! operations must charge the same meter its tool does.
//!
//! Errors are JSON `{"error": text, "code": code}` (see [`ApiError`]); an
//! authentication failure is the OAuth-shaped `{"error", "error_description"}`
//! with `WWW-Authenticate`. Bodies are capped at [`MAX_BODY_BYTES`]. There is
//! deliberately no CORS layer: `/api` is called with bearer tokens or, by the
//! console, from the same origin.

mod error;
mod extract;
mod routes;
mod webhooks;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, MethodRouter};
use axum::{middleware, Router};
use cxp_core::auth::Principal;
use cxp_core::search::{SearchLimiter, SearchPermit};
use http::Method;
use otto_resource::PlatformClient;
use otto_tenant::{Db, Tx};

pub use error::{ApiError, ApiResult};

/// The largest request body any route accepts: 256 KiB. Larger is `413`.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// What `cxp-api` needs from the binary's configuration.
#[derive(Clone)]
pub struct Config {
    /// This service's public origin (`CXP_PUBLIC_URL`, e.g.
    /// `https://cxp.savvagent.com`). A `401`'s `WWW-Authenticate` points
    /// at `{public_url}/.well-known/oauth-protected-resource`.
    pub public_url: String,
    /// The secret the platform signs lifecycle webhooks with
    /// (`CXP_PLATFORM_WEBHOOK_SECRET`, `otto_whsec_…`). Empty refuses every
    /// delivery: an empty HMAC key is not a secret.
    pub webhook_secret: String,
}

// Not derived: the webhook secret must never reach a log line.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("public_url", &self.public_url)
            .field("webhook_secret", &"<redacted>")
            .finish()
    }
}

/// Shared by every route. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    /// Token introspection, the caller's teams, and catalog team checks.
    pub platform: Arc<PlatformClient>,
    /// The process-wide cap on concurrent searches, shared with `/mcp`
    /// (#42). A search takes its permit before opening its transaction.
    pub search: SearchLimiter,
    pub config: Arc<Config>,
    /// The web console's sign-in, when it is configured: its session cookie
    /// is then a credential on `/api/v1`.
    pub console: Option<cxp_http::console::Console>,
}

impl AppState {
    pub fn new(
        db: Db,
        platform: Arc<PlatformClient>,
        search: SearchLimiter,
        config: Config,
    ) -> Self {
        Self {
            db,
            platform,
            search,
            config: Arc::new(config),
            console: None,
        }
    }

    /// Accept the web console's session cookie on `/api/v1`.
    pub fn with_console(mut self, console: cxp_http::console::Console) -> Self {
        self.console = Some(console);
        self
    }

    /// The caller's transaction: pinned to their org, refused if the org was
    /// deleted or the caller removed.
    pub(crate) async fn begin(&self, caller: &Principal) -> cxp_core::Result<Tx<'static>> {
        cxp_core::platform_events::begin_live(&self.db, caller.org_id, Some(caller.user_id)).await
    }

    /// A search permit, then the caller's transaction, in that order (#42):
    /// waiting for the permit must not hold a pooled connection. Bind the
    /// pair as `let (permit, mut tx)` so the transaction drops first.
    pub(crate) async fn begin_search(
        &self,
        caller: &Principal,
    ) -> cxp_core::Result<(SearchPermit, Tx<'static>)> {
        let permit = self.search.acquire().await?;
        let tx = self.begin(caller).await?;
        Ok((permit, tx))
    }
}

/// The prefix every REST route is served under.
pub const PREFIX: &str = "/api/v1";

/// Every `/api/v1` route, as (method, full path with `{id}` placeholders).
/// The single source [`router`] is built from: each path here is mounted
/// with [`methods_for`], and a unit test probes every method on every path
/// so a method served but not listed (or listed but not served) fails.
/// Callers that must cover every route (the e2e isolation and metering
/// tests in cxp-server) iterate this.
pub fn route_table() -> Vec<(Method, &'static str)> {
    let mut routes = vec![
        (Method::GET, "/api/v1/me/teams"),
        (Method::GET, "/api/v1/dashboard/metrics"),
        (Method::GET, "/api/v1/customers"),
        (Method::POST, "/api/v1/customers"),
        (Method::GET, "/api/v1/customers/search"),
        (Method::GET, "/api/v1/customers/{id}"),
        (Method::PUT, "/api/v1/customers/{id}"),
        (Method::DELETE, "/api/v1/customers/{id}"),
        (Method::GET, "/api/v1/customers/{id}/360"),
        (Method::GET, "/api/v1/tickets"),
        (Method::POST, "/api/v1/tickets"),
        (Method::GET, "/api/v1/tickets/{id}"),
        (Method::PUT, "/api/v1/tickets/{id}"),
        (Method::DELETE, "/api/v1/tickets/{id}"),
        (Method::GET, "/api/v1/feedback"),
        (Method::GET, "/api/v1/surveys"),
        (Method::POST, "/api/v1/surveys"),
        (Method::GET, "/api/v1/surveys/{id}"),
        (Method::PUT, "/api/v1/surveys/{id}"),
        (Method::DELETE, "/api/v1/surveys/{id}"),
        (Method::GET, "/api/v1/surveys/{id}/responses"),
    ];
    for (collection, item) in [
        ("/api/v1/experiences", "/api/v1/experiences/{id}"),
        ("/api/v1/applications", "/api/v1/applications/{id}"),
        ("/api/v1/pages", "/api/v1/pages/{id}"),
        ("/api/v1/components", "/api/v1/components/{id}"),
    ] {
        routes.push((Method::GET, collection));
        routes.push((Method::POST, collection));
        routes.push((Method::GET, item));
        routes.push((Method::PUT, item));
        routes.push((Method::DELETE, item));
    }
    routes
}

/// The handlers for one path of [`route_table`]. Panics on a path the table
/// lists but nothing here serves, which the router's own construction (and
/// so every test) hits at once.
fn methods_for(path: &str) -> MethodRouter<AppState> {
    use routes::{catalog, customers, dashboard, feedback, me, surveys, tickets};
    match path {
        "/api/v1/me/teams" => get(me::teams),
        "/api/v1/dashboard/metrics" => get(dashboard::metrics),
        "/api/v1/customers" => get(customers::list).post(customers::create),
        "/api/v1/customers/search" => get(customers::search),
        "/api/v1/customers/{id}" => get(customers::get)
            .put(customers::update)
            .delete(customers::delete),
        "/api/v1/customers/{id}/360" => get(customers::get_360),
        "/api/v1/tickets" => get(tickets::list).post(tickets::create),
        "/api/v1/tickets/{id}" => get(tickets::get)
            .put(tickets::update)
            .delete(tickets::delete),
        "/api/v1/feedback" => get(feedback::list),
        "/api/v1/surveys" => get(surveys::list).post(surveys::create),
        "/api/v1/surveys/{id}" => get(surveys::get)
            .put(surveys::update)
            .delete(surveys::delete),
        "/api/v1/surveys/{id}/responses" => get(surveys::responses),
        "/api/v1/experiences" => get(catalog::list_experiences).post(catalog::create_experience),
        "/api/v1/experiences/{id}" => get(catalog::get_experience)
            .put(catalog::update_experience)
            .delete(catalog::delete_experience),
        "/api/v1/applications" => get(catalog::list_applications).post(catalog::create_application),
        "/api/v1/applications/{id}" => get(catalog::get_application)
            .put(catalog::update_application)
            .delete(catalog::delete_application),
        "/api/v1/pages" => get(catalog::list_pages).post(catalog::create_page),
        "/api/v1/pages/{id}" => get(catalog::get_page)
            .put(catalog::update_page)
            .delete(catalog::delete_page),
        "/api/v1/components" => get(catalog::list_components).post(catalog::create_component),
        "/api/v1/components/{id}" => get(catalog::get_component)
            .put(catalog::update_component)
            .delete(catalog::delete_component),
        other => panic!("route_table lists {other}, which methods_for does not serve"),
    }
}

/// The `/api/v1` routes of [`route_table`], relative to [`PREFIX`], without
/// the bearer layer.
fn api_routes() -> Router<AppState> {
    let mut api = Router::new();
    let mut mounted: Vec<&str> = vec![];
    for (_, path) in route_table() {
        if !mounted.contains(&path) {
            mounted.push(path);
            let relative = path
                .strip_prefix(PREFIX)
                .expect("every route is under /api/v1");
            api = api.route(relative, methods_for(path));
        }
    }
    api
}

/// `/api/v1/...` behind the authentication middleware, and `POST
/// /platform/webhooks`.
pub fn router(state: AppState) -> Router {
    // A handler's `401` is `access_revoked`; it carries the same challenge as
    // a revoked token.
    let mut authenticator = cxp_http::Authenticator::new(
        state.db.clone(),
        state.platform.clone(),
        state.config.public_url.clone(),
    )
    .challenge_handler_401();
    if let Some(console) = &state.console {
        authenticator = authenticator.with_console(console.clone());
    }
    let api = api_routes()
        // Inside the bearer layer: an unknown path or method is answered
        // only to an authenticated caller, and in the JSON error shape.
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            authenticator,
            cxp_http::require_bearer,
        ));

    Router::new()
        .nest(PREFIX, api)
        .route("/platform/webhooks", post(webhooks::receive))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

async fn not_found() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        "not_found",
        "no such route under /api/v1",
    )
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(
        http::StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "this route does not accept that method",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use otto_resource::{ClientConfig, PlatformClient};
    use tower::ServiceExt;

    /// The routes without the bearer layer, over a database and platform
    /// nothing here reaches: with no principal attached, a request that
    /// reaches a handler fails its `Extension<Principal>` extractor (500),
    /// which no fallback answers.
    fn bare() -> Router {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://cxp:cxp@127.0.0.1:1/none")
            .unwrap();
        let platform = Arc::new(
            PlatformClient::new(ClientConfig::new(
                "https://platform.example.com",
                "https://cxp.example.com/mcp",
                "otto_rs_test",
            ))
            .unwrap(),
        );
        let state = AppState::new(
            cxp_core::tenant_db(pool),
            platform,
            SearchLimiter::for_pool(5),
            Config {
                public_url: "https://cxp.example.com".into(),
                webhook_secret: "otto_whsec_test".into(),
            },
        );
        Router::new()
            .nest(
                PREFIX,
                api_routes().method_not_allowed_fallback(method_not_allowed),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn the_router_serves_exactly_the_route_table() {
        let table = route_table();
        let mut paths: Vec<&str> = table.iter().map(|(_, p)| *p).collect();
        paths.sort_unstable();
        paths.dedup();
        let unique: std::collections::HashSet<_> = table.iter().collect();
        assert_eq!(unique.len(), table.len(), "a route is listed twice");
        for path in paths {
            let uri = path.replace("{id}", "00000000-0000-0000-0000-000000000000");
            for method in [
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ] {
                let res = bare()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(&uri)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let listed = table.contains(&(method.clone(), path));
                if listed {
                    assert_eq!(
                        res.status(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "{method} {path} is listed but no handler answered it"
                    );
                } else {
                    assert_eq!(
                        res.status(),
                        StatusCode::METHOD_NOT_ALLOWED,
                        "{method} {path} is served but not listed in route_table"
                    );
                }
            }
        }
    }
}
