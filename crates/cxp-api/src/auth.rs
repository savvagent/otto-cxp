//! The bearer-token gate in front of `/api/v1`.
//!
//! `cxp_core::auth::authenticate` decides; this maps its answer to HTTP:
//! `Missing`, `Inactive`, and `Revoked` are `401` with `WWW-Authenticate`
//! pointing at the protected-resource metadata, `Unavailable` is `503` with
//! `Retry-After` (an outage must never send a client to re-authenticate),
//! and `Internal` (the platform refused this service's own credential) is
//! `500`. On
//! success the [`Principal`] goes into the request's extensions for handlers.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cxp_core::auth::{self, AuthFailure, Principal};
use http::{header, HeaderName, HeaderValue, StatusCode};

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
                    revoked_challenge(&state.config.public_url),
                );
            }
            res
        }
        Err(failure) => refuse(&state.config.public_url, failure),
    }
}

/// The shared [`auth::refusal`] as an axum response.
fn refuse(public_url: &str, failure: AuthFailure) -> Response {
    let refusal = auth::refusal(public_url, &failure);
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::UNAUTHORIZED);
    let mut res = (status, Json(refusal.body)).into_response();
    for (name, value) in refusal.headers {
        let value =
            HeaderValue::from_str(&value).expect("cxp_core::auth guarantees header-safe values");
        res.headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
    res
}

/// The challenge a handler's `401` (`access_revoked`) gets: the one a revoked
/// token gets here.
fn revoked_challenge(public_url: &str) -> HeaderValue {
    let refusal = auth::refusal(public_url, &AuthFailure::Revoked);
    let challenge = refusal
        .www_authenticate()
        .expect("a 401 refusal carries a challenge");
    HeaderValue::from_str(challenge).expect("cxp_core::auth guarantees header-safe values")
}
