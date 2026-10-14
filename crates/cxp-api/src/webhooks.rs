//! `POST /platform/webhooks`: the otto platform's signed lifecycle events
//! (`org.deleted`, `member.removed`, `team.deleted`).
//!
//! **The status code is the retry protocol.** The platform redelivers anything
//! that is not a 2xx. A missing, malformed, stale, or wrong signature is `401`
//! and nothing is processed (the body is not parsed before the MAC checks
//! out); a signed but unreadable body is `400`; an applied, repeated, or
//! unknown event is `200 {"result": "applied" | "duplicate" | "ignored"}`; a
//! failure to apply is `503`, so the platform tries again. Fails closed: with
//! no secret configured every delivery is refused.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::State;
use axum::Json;
use cxp_core::platform_events::{self, Outcome};
use http::{HeaderMap, StatusCode};
use otto_resource::webhook::{self, WebhookError};

use crate::error::{ApiError, ApiResult};
use crate::AppState;

fn rejected() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_signature",
        "the Otto-Signature header is missing, malformed, stale, or does not match the body",
    )
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Json<Outcome>> {
    let secret = state.config.webhook_secret.as_str();
    if secret.is_empty() {
        tracing::error!("platform webhook refused: no webhook secret is configured");
        return Err(rejected());
    }
    let signature = headers
        .get(webhook::SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(rejected)?;
    let body = body.map_err(|e| crate::extract::rejected(e.status(), e.body_text()))?;
    let event = match webhook::verify(secret, signature, &body) {
        Ok(event) => event,
        Err(WebhookError::BadBody(reason)) => {
            tracing::error!(%reason, "platform webhook was signed but unreadable");
            return Err(ApiError::invalid("the webhook body is not a valid event"));
        }
        Err(e) => {
            tracing::warn!(error = %e, "platform webhook rejected");
            return Err(rejected());
        }
    };
    let outcome = platform_events::apply(&state.db, &event)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, source = ?std::error::Error::source(&e),
                event_id = %event.id, "could not apply a platform event");
            ApiError::unavailable("could not apply the event; redeliver it")
        })?;
    Ok(Json(outcome))
}
