//! The HTTP side of authentication: `cxp_core::auth` decides, this module
//! answers.
//!
//! - No token, or one the platform calls inactive (unknown, expired, revoked,
//!   minted for another resource), or one whose org or member the platform has
//!   since told us is gone: `401` with `WWW-Authenticate` naming
//!   `/.well-known/oauth-protected-resource`, which is how an MCP client learns
//!   where to sign in.
//! - The platform or the database could not answer: `503` with
//!   `Retry-After: 5`, never `401`. A `401` sends every connected agent to
//!   re-authenticate against the platform that is down.
//! - The platform refused this service's own credential, or something that
//!   is not the platform answered: `500`, with neither header. A
//!   misconfiguration no retry can fix, logged at `error`.
//!
//! **The principal is per request, not per session.** It is inserted into the
//! request's extensions, which the MCP transport carries through to every
//! handler as `http::request::Parts`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cxp_core::auth::{AuthFailure, Principal};
use http::StatusCode;

use crate::Shared;

/// Reject the request unless it carries a live token audienced for us.
pub(crate) async fn require_bearer(
    State(shared): State<Arc<Shared>>,
    mut req: Request,
    next: Next,
) -> Response {
    let authorization = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match cxp_core::auth::authenticate(&shared.db, &shared.platform, authorization).await {
        Ok(principal) => {
            req.extensions_mut().insert(principal);
            next.run(req).await
        }
        Err(failure) => refuse(&shared.public_url, failure),
    }
}

/// The response for a request that was not authenticated: the shared
/// [`cxp_core::auth::refusal`] as an axum response.
pub(crate) fn refuse(public_url: &str, failure: AuthFailure) -> Response {
    let refusal = cxp_core::auth::refusal(public_url, &failure);
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::UNAUTHORIZED);
    let mut response = (status, Json(refusal.body)).into_response();
    for (name, value) in refusal.headers {
        let value = http::HeaderValue::from_str(&value)
            .expect("cxp_core::auth guarantees header-safe values");
        response
            .headers_mut()
            .insert(http::HeaderName::from_static(name), value);
    }
    response
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728). Open: it is how an
/// unauthenticated client finds the authorization server (the platform).
pub(crate) async fn protected_resource_metadata(State(shared): State<Arc<Shared>>) -> Response {
    Json(cxp_core::auth::metadata(
        &shared.resource_uri,
        &shared.platform_url,
    ))
    .into_response()
}

/// The authenticated caller of a tool call, resource read, or prompt: the
/// principal [`require_bearer`] attached to the HTTP request.
pub fn principal_from(parts: &http::request::Parts) -> Option<Principal> {
    parts.extensions.get::<Principal>().cloned()
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
}
