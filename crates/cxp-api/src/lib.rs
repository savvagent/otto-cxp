//! `cxp-api` — otto-cxp's REST API (`/api/v1/...`, for the web console) and
//! the otto platform's signed lifecycle webhooks (`/platform/webhooks`). Every
//! route calls into `cxp-core`; no SQL lives here.
//!
//! ```text
//!   /api/v1/...            bearer token   GET needs cxp:read (or cxp:write),
//!                                         every other method cxp:write
//!   POST /platform/webhooks  Otto-Signature HMAC; no bearer token
//! ```
//!
//! Every `/api/v1` handler does the same five things in order: take the
//! [`Principal`](cxp_core::auth::Principal) the bearer middleware attached,
//! check its scope, do any platform lookup it needs (a catalog team id, the
//! usage quota) *before* opening a transaction so no pooled connection waits
//! on the network, open the transaction with
//! [`platform_events::begin_live`](cxp_core::platform_events::begin_live), and
//! commit after the one domain call.
//!
//! **Metering:** the only route that records usage is `POST /api/v1/surveys`,
//! charged as `create_survey` in the same transaction as the insert, because
//! it is the same operation as the MCP tool. Nothing else here charges.
//!
//! Errors are JSON `{"error": text, "code": code}` (see [`ApiError`]); an
//! authentication failure is the OAuth-shaped `{"error", "error_description"}`
//! with `WWW-Authenticate`. Bodies are capped at [`MAX_BODY_BYTES`]. There is
//! deliberately no CORS layer: `/api` is called with bearer tokens or from the
//! same origin.

mod auth;
mod error;
mod extract;
mod routes;
mod webhooks;

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::{middleware, Router};
use cxp_core::auth::Principal;
use cxp_core::usage::Meter;
use otto_resource::PlatformClient;
use otto_tenant::{Db, Tx};

pub use error::{ApiError, ApiResult};

/// The largest request body any route accepts: 256 KiB. Larger is `413`.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// What `cxp-api` needs from the binary's configuration.
#[derive(Clone)]
pub struct Config {
    /// This service's public origin (`CXP_PUBLIC_URL`, e.g.
    /// `https://otto-cxp.savvagent.com`). A `401`'s `WWW-Authenticate` points
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
    /// Records `create_survey` usage for `POST /api/v1/surveys`.
    pub meter: Arc<Meter>,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(db: Db, platform: Arc<PlatformClient>, meter: Arc<Meter>, config: Config) -> Self {
        Self {
            db,
            platform,
            meter,
            config: Arc::new(config),
        }
    }

    /// The caller's transaction: pinned to their org, refused if the org was
    /// deleted or the caller removed.
    pub(crate) async fn begin(&self, caller: &Principal) -> cxp_core::Result<Tx<'static>> {
        cxp_core::platform_events::begin_live(&self.db, caller.org_id, Some(caller.user_id)).await
    }
}

/// `/api/v1/...` behind the bearer middleware, and `POST /platform/webhooks`.
pub fn router(state: AppState) -> Router {
    use routes::{catalog, customers, dashboard, feedback, me, surveys, tickets};

    let api = Router::new()
        .route("/me/teams", get(me::teams))
        .route("/dashboard/metrics", get(dashboard::metrics))
        .route("/customers", get(customers::list).post(customers::create))
        .route("/customers/search", get(customers::search))
        .route(
            "/customers/{id}",
            get(customers::get)
                .put(customers::update)
                .delete(customers::delete),
        )
        .route("/customers/{id}/360", get(customers::get_360))
        .route("/tickets", get(tickets::list).post(tickets::create))
        .route(
            "/tickets/{id}",
            get(tickets::get)
                .put(tickets::update)
                .delete(tickets::delete),
        )
        .route("/feedback", get(feedback::list))
        .route("/surveys", get(surveys::list).post(surveys::create))
        .route(
            "/surveys/{id}",
            get(surveys::get)
                .put(surveys::update)
                .delete(surveys::delete),
        )
        .route("/surveys/{id}/responses", get(surveys::responses))
        .route(
            "/experiences",
            get(catalog::list_experiences).post(catalog::create_experience),
        )
        .route(
            "/experiences/{id}",
            get(catalog::get_experience)
                .put(catalog::update_experience)
                .delete(catalog::delete_experience),
        )
        .route(
            "/applications",
            get(catalog::list_applications).post(catalog::create_application),
        )
        .route(
            "/applications/{id}",
            get(catalog::get_application)
                .put(catalog::update_application)
                .delete(catalog::delete_application),
        )
        .route(
            "/pages",
            get(catalog::list_pages).post(catalog::create_page),
        )
        .route(
            "/pages/{id}",
            get(catalog::get_page)
                .put(catalog::update_page)
                .delete(catalog::delete_page),
        )
        .route(
            "/components",
            get(catalog::list_components).post(catalog::create_component),
        )
        .route(
            "/components/{id}",
            get(catalog::get_component)
                .put(catalog::update_component)
                .delete(catalog::delete_component),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    Router::new()
        .nest("/api/v1", api)
        .route("/platform/webhooks", post(webhooks::receive))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}
