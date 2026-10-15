//! `cxp-mcp` — otto-cxp's MCP server on rmcp 2.0.0: the customer-data,
//! support, and feedback tools plus `whoami` and `usage`, the MCP resources
//! and prompts, and the `/mcp` route behind bearer-token authentication.
//!
//! ```text
//!   POST /mcp
//!     └─ cxp_http::require_bearer ──── introspect the token at the platform, attach the principal
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
use axum::routing::any_service;
use axum::Router;
use cxp_core::search::SearchLimiter;
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
    /// Accepted `Host` headers (DNS-rebinding protection); a request with any
    /// other `Host` is refused with `403`. rmcp's default is loopback only,
    /// which rejects every request to a hosted server, so it is required
    /// here. rmcp reads an empty list as "allow any host", so [`router`]
    /// refuses one (and a list of only blank entries) rather than serve `/mcp`
    /// open to DNS rebinding.
    pub allowed_hosts: Vec<String>,
    /// Accepted browser `Origin`s; empty disables the check (CLI agents send
    /// none).
    pub allowed_origins: Vec<String>,
}

/// [`router`] was given no allowed `Host`, which rmcp would read as "accept
/// any". Fails closed at the crate boundary, whoever the caller is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "no Host header would be accepted on /mcp, and rmcp reads an empty allowed-host list as \
     \"accept any\"; give at least one allowed host"
)]
pub struct NoAllowedHosts;

/// The MCP surface: `/mcp` (bearer tokens audienced for this resource; the
/// audience is enforced by the [`PlatformClient`]). The open
/// `/.well-known/oauth-protected-resource` that its `401`s point at is
/// mounted by the binary, with `cxp_http::protected_resource_router`.
///
/// Stateless transport with JSON responses: nothing is pushed to a client, so
/// any replica can serve any request without sticky sessions.
///
/// Refuses ([`NoAllowedHosts`]) a [`Config::allowed_hosts`] with no
/// non-blank entry.
pub fn router(
    db: Db,
    platform: Arc<PlatformClient>,
    meter: Arc<Meter>,
    search: SearchLimiter,
    config: Config,
) -> Result<Router, NoAllowedHosts> {
    if config.allowed_hosts.iter().all(|h| h.trim().is_empty()) {
        return Err(NoAllowedHosts);
    }
    let authenticator =
        cxp_http::Authenticator::new(db.clone(), platform.clone(), config.public_url);

    let mut transport = StreamableHttpServerConfig::default();
    transport.stateful_mode = false;
    transport.json_response = true;
    transport.allowed_hosts = config.allowed_hosts;
    transport.allowed_origins = config.allowed_origins;

    let cxp = Cxp::new(db, platform, meter, search);
    let service = StreamableHttpService::new(
        move || Ok(cxp.clone()),
        Arc::new(LocalSessionManager::default()),
        transport,
    );

    Ok(Router::new().route_service(
        "/mcp",
        any_service(service)
            .layer(axum::middleware::from_fn(buffer_body))
            .layer(axum::middleware::from_fn_with_state(
                authenticator,
                cxp_http::require_bearer,
            )),
    ))
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
