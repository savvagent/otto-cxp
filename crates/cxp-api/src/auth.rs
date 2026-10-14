//! The bearer-token gate in front of `/api/v1`.
//!
//! `cxp_core::auth::authenticate` decides; this maps its answer to HTTP:
//! `Missing`, `Inactive`, and `Revoked` are `401` with `WWW-Authenticate`
//! pointing at the protected-resource metadata, `Unavailable` is `503` with
//! `Retry-After` (an outage must never send a client to re-authenticate). On
//! success the [`Principal`] goes into the request's extensions for handlers.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cxp_core::auth::{self, AuthFailure, Principal};
use http::{header, HeaderValue, StatusCode};

use crate::AppState;

pub async fn require_bearer(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let authorization = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth::authenticate(&state.db, &state.platform, authorization).await {
        Ok(principal) => {
            req.extensions_mut().insert::<Principal>(principal);
            let mut res = next.run(req).await;
            // A handler's `401` is `access_revoked` (the org was deleted or
            // the caller removed between authentication and the transaction);
            // it carries the same challenge as one from here.
            if res.status() == StatusCode::UNAUTHORIZED
                && !res.headers().contains_key(header::WWW_AUTHENTICATE)
            {
                res.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    challenge(&state.config.public_url, &AuthFailure::Revoked),
                );
            }
            res
        }
        Err(failure) => refuse(&state.config.public_url, failure),
    }
}

fn refuse(public_url: &str, failure: AuthFailure) -> Response {
    if failure.is_unavailable() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "5")],
            Json(serde_json::json!({
                "error": "temporarily_unavailable",
                "error_description": failure.reason(),
            })),
        )
            .into_response();
    }
    let mut res = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "error": error_name(&failure),
            "error_description": failure.reason(),
        })),
    )
        .into_response();
    res.headers_mut()
        .insert(header::WWW_AUTHENTICATE, challenge(public_url, &failure));
    res
}

/// The body's `error`. A request with no credentials is `unauthenticated`,
/// not `invalid_token`: there was no token to be invalid, and the challenge
/// header carries no `error` for it either (RFC 6750 §3.1).
fn error_name(failure: &AuthFailure) -> &'static str {
    match failure {
        AuthFailure::Missing => "unauthenticated",
        _ => "invalid_token",
    }
}

fn challenge(public_url: &str, failure: &AuthFailure) -> HeaderValue {
    // `www_authenticate` emits only visible ASCII; should a misconfigured
    // public URL break that, keep the scheme rather than drop the header.
    HeaderValue::from_str(&auth::www_authenticate(public_url, failure))
        .unwrap_or_else(|_| HeaderValue::from_static(r#"Bearer realm="otto-cxp""#))
}
