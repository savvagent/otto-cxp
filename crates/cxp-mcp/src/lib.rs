//! `cxp-mcp` — otto-cxp's MCP server on rmcp 2.0.0: the customer-data,
//! support, and feedback tools plus `whoami` and `usage`, the MCP resources
//! and prompts, and the `/mcp` and `/.well-known/oauth-protected-resource`
//! routes behind bearer-token authentication.
//!
//! ```text
//!   POST /mcp
//!     └─ require_bearer ──── introspect the token at the platform, attach the principal
//!          └─ buffer_body ──── read the body under the body limit, or 413
//!               └─ StreamableHttpService ──── rmcp, JSON-RPC framing, stateless, JSON responses
//!                    └─ Cxp ──── one tool call, resource read, or prompt
//!                         └─ begin_live(org) ──── cxp-core, RLS, commit
//! ```
//!
//! Every tool calls into `cxp-core`; no SQL lives here.

pub mod auth;
pub mod error;
pub mod prompts;
pub mod resources;
pub mod server;
pub mod tools;

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any_service, get};
use axum::Router;
use cxp_core::usage::Meter;
use http::StatusCode;
use otto_resource::PlatformClient;
use otto_tenant::Db;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

pub use server::Cxp;

/// Deployment settings the MCP surface cannot infer.
#[derive(Debug, Clone)]
pub struct Config {
    /// Public base URL of this service (e.g. `https://otto-cxp.savvagent.com`),
    /// for the discovery pointer in a `401`.
    pub public_url: String,
    /// This resource's canonical URI (`CXP_RESOURCE_URI`) and the audience
    /// every token must carry. Advertised in the metadata document; the
    /// audience itself is enforced by the [`PlatformClient`], which was built
    /// with the same value.
    pub resource_uri: String,
    /// The platform's public base URL, advertised as the authorization server.
    pub platform_url: String,
    /// Accepted `Host` headers (DNS-rebinding protection); a request with any
    /// other `Host` is refused with `403`. rmcp's default is loopback only,
    /// which rejects every request to a hosted server, so it is required
    /// here. **An empty list means "allow any host"** (rmcp's
    /// `disable_allowed_hosts`), so never pass one in production: cxp-server
    /// always passes at least the host of its public URL.
    pub allowed_hosts: Vec<String>,
    /// Accepted browser `Origin`s; empty disables the check (CLI agents send
    /// none).
    pub allowed_origins: Vec<String>,
}

/// What the auth middleware and the metadata route share.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) db: Db,
    pub(crate) platform: Arc<PlatformClient>,
    pub(crate) public_url: String,
    pub(crate) resource_uri: String,
    pub(crate) platform_url: String,
}

/// The MCP surface: `/.well-known/oauth-protected-resource` (open, because it
/// is how an unauthenticated client learns to authenticate) and `/mcp`
/// (bearer tokens audienced for [`Config::resource_uri`]).
///
/// Stateless transport with JSON responses: nothing is pushed to a client, so
/// any replica can serve any request without sticky sessions.
pub fn router(db: Db, platform: Arc<PlatformClient>, meter: Arc<Meter>, config: Config) -> Router {
    let shared = Arc::new(Shared {
        db: db.clone(),
        platform: platform.clone(),
        public_url: config.public_url,
        resource_uri: config.resource_uri,
        platform_url: config.platform_url,
    });

    let mut transport = StreamableHttpServerConfig::default();
    transport.stateful_mode = false;
    transport.json_response = true;
    transport.allowed_hosts = config.allowed_hosts;
    transport.allowed_origins = config.allowed_origins;

    let cxp = Cxp::new(db, platform, meter);
    let service = StreamableHttpService::new(
        move || Ok(cxp.clone()),
        Arc::new(LocalSessionManager::default()),
        transport,
    );

    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(auth::protected_resource_metadata),
        )
        .with_state(shared.clone())
        .route_service(
            "/mcp",
            any_service(service)
                .layer(axum::middleware::from_fn(buffer_body))
                .layer(axum::middleware::from_fn_with_state(
                    shared,
                    auth::require_bearer,
                )),
        )
}

/// Read `/mcp`'s body before rmcp does, through axum's `Bytes` extractor, so
/// it honors whatever limit the stack sets (cxp-server's router-wide cap, or
/// axum's default) and an oversized body is a `413` rather than the `500`
/// rmcp answers for a body it fails to read.
async fn buffer_body(req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = match Bytes::from_request(Request::from_parts(parts.clone(), body), &()).await {
        Ok(bytes) => bytes,
        Err(e) if e.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                axum::Json(serde_json::json!({
                    "error": "the request body is too large",
                    "code": "payload_too_large",
                })),
            )
                .into_response();
        }
        Err(e) => return e.into_response(),
    };
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}
