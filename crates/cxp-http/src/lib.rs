//! `cxp-http` — the axum side of authentication, shared by `/mcp` and
//! `/api/v1` so the two surfaces cannot drift apart.
//!
//! `cxp_core::auth` decides (it holds the platform and database calls and
//! knows nothing of HTTP); this crate answers:
//!
//! - No token, or one the platform calls inactive (unknown, expired, revoked,
//!   minted for another resource), or one whose org or member the platform has
//!   since told us is gone: `401` with `WWW-Authenticate` naming
//!   `/.well-known/oauth-protected-resource`, which is how a client learns
//!   where to sign in.
//! - The platform or the database could not answer: `503` with
//!   `Retry-After: 5`, never `401`. A `401` sends every connected client to
//!   re-authenticate against the platform that is down.
//! - The platform refused this service's own credential, or something that
//!   is not the platform answered: `500`, with neither header. A
//!   misconfiguration no retry can fix, logged at `error`.
//!
//! The one place a request's credential becomes a [`Principal`] is
//! [`Authenticator::authenticate`]; a further credential source (a session
//! cookie for the console) is added there, and both surfaces pick it up.
//!
//! The protected-resource metadata document (RFC 9728) is served by
//! [`protected_resource_router`], which the binary mounts: the `401`s of every
//! surface point at it, so it belongs to none of them.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cxp_core::auth::{self, AuthFailure, Principal};
use http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use otto_resource::PlatformClient;
use otto_tenant::Db;

/// Where the protected-resource metadata is served, and what a `401`'s
/// `resource_metadata` points at.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// Turns a request into a [`Principal`], or says why not. Cheap to clone.
#[derive(Clone)]
pub struct Authenticator {
    db: Db,
    platform: Arc<PlatformClient>,
    public_url: String,
    challenge_handler_401: bool,
}

impl Authenticator {
    pub fn new(db: Db, platform: Arc<PlatformClient>, public_url: impl Into<String>) -> Self {
        Self {
            db,
            platform,
            public_url: public_url.into(),
            challenge_handler_401: false,
        }
    }

    /// Give a handler's own `401` without a `WWW-Authenticate` the challenge a
    /// revoked token gets. A handler answers `401` (`access_revoked`) when the
    /// org was deleted or the caller removed between authentication and its
    /// transaction.
    pub fn challenge_handler_401(mut self) -> Self {
        self.challenge_handler_401 = true;
        self
    }

    /// The credential in `headers`, checked. Today that is the bearer token
    /// in `Authorization`.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthFailure> {
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        auth::authenticate(&self.db, &self.platform, authorization).await
    }
}

/// Reject the request unless it carries a live credential audienced for us;
/// otherwise put the [`Principal`] in the request's extensions for handlers
/// (the MCP transport carries it through as `http::request::Parts`).
pub async fn require_bearer(
    State(auth): State<Authenticator>,
    mut req: Request,
    next: Next,
) -> Response {
    match auth.authenticate(req.headers()).await {
        Ok(principal) => {
            req.extensions_mut().insert::<Principal>(principal);
            let mut res = next.run(req).await;
            if auth.challenge_handler_401
                && res.status() == StatusCode::UNAUTHORIZED
                && !res.headers().contains_key(header::WWW_AUTHENTICATE)
            {
                res.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    revoked_challenge(&auth.public_url),
                );
            }
            res
        }
        Err(failure) => refuse(&auth.public_url, failure),
    }
}

/// The response for a request that was not authenticated: the shared
/// [`auth::refusal`] as an axum response.
pub fn refuse(public_url: &str, failure: AuthFailure) -> Response {
    let refusal = auth::refusal(public_url, &failure);
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::UNAUTHORIZED);
    let mut response = (status, Json(refusal.body)).into_response();
    for (name, value) in refusal.headers {
        let value =
            HeaderValue::from_str(&value).expect("cxp_core::auth guarantees header-safe values");
        response
            .headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
    response
}

/// The challenge a handler's `401` (`access_revoked`) gets: the one a revoked
/// token gets from [`require_bearer`].
fn revoked_challenge(public_url: &str) -> HeaderValue {
    let refusal = auth::refusal(public_url, &AuthFailure::Revoked);
    let challenge = refusal
        .www_authenticate()
        .expect("a 401 refusal carries a challenge");
    HeaderValue::from_str(challenge).expect("cxp_core::auth guarantees header-safe values")
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728). Open: it is how an
/// unauthenticated client finds the authorization server (the platform).
pub fn protected_resource_router(resource_uri: &str, platform_url: &str) -> Router {
    let document = auth::metadata(resource_uri, platform_url);
    Router::new().route(
        PROTECTED_RESOURCE_PATH,
        get(move || {
            let document = document.clone();
            async move { Json(document) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_token_is_a_401_that_points_at_the_metadata() {
        let r = refuse("https://cxp.example.com/", AuthFailure::Missing);
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let h = r.headers()[http::header::WWW_AUTHENTICATE]
            .to_str()
            .unwrap();
        assert!(h.starts_with("Bearer "), "{h}");
        assert!(!h.contains("error="), "{h}");
        assert!(
            h.contains(
                r#"resource_metadata="https://cxp.example.com/.well-known/oauth-protected-resource""#
            ),
            "{h}"
        );
        assert!(h.contains(PROTECTED_RESOURCE_PATH));
    }

    #[test]
    fn inactive_and_revoked_are_401_with_invalid_token() {
        for f in [AuthFailure::Inactive, AuthFailure::Revoked] {
            let r = refuse("https://cxp.example.com", f);
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            let h = r.headers()[http::header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap();
            assert!(h.contains(r#"error="invalid_token""#), "{h}");
        }
    }

    #[test]
    fn an_outage_is_a_503_with_retry_after_and_no_challenge() {
        let r = refuse("https://cxp.example.com", AuthFailure::Unavailable);
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.headers()[http::header::RETRY_AFTER], "5");
        assert!(r.headers().get(http::header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn the_metadata_router_serves_the_document_it_was_built_with() {
        use tower::ServiceExt;
        let app = protected_resource_router("https://cxp.example.com/mcp", "https://p.example.com");
        let res = app
            .oneshot(
                Request::get(PROTECTED_RESOURCE_PATH)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(doc["resource"], "https://cxp.example.com/mcp");
        assert_eq!(doc["authorization_servers"][0], "https://p.example.com");
    }
}
