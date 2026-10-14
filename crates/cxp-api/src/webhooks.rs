//! `POST /platform/webhooks`: the otto platform's signed lifecycle events
//! (`org.deleted`, `member.removed`, `team.deleted`).
//!
//! **The status code is the retry protocol.** The platform redelivers anything
//! that is not a 2xx. A missing, malformed, stale, or wrong signature is `401`
//! and nothing is processed (the body is not parsed before the MAC checks
//! out); a signed but unreadable body is `400`; an applied, repeated, or
//! unknown event is `200 {"result": "applied" | "duplicate" | "ignored"}`; a
//! failure to apply that can pass (a database blip, or an event this build
//! parses but has no handler for) is `503`, so the platform tries again; one
//! that redelivery cannot fix is `400` `invalid_event`, so it is not retried
//! forever. Fails closed: with no secret configured every delivery is
//! refused.

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
    let Some(signature) = headers.get(webhook::SIGNATURE_HEADER) else {
        tracing::warn!(
            header = webhook::SIGNATURE_HEADER,
            "platform webhook rejected: no signature header"
        );
        return Err(rejected());
    };
    let Ok(signature) = signature.to_str() else {
        tracing::warn!(
            header = webhook::SIGNATURE_HEADER,
            "platform webhook rejected: the signature header is not visible ASCII"
        );
        return Err(rejected());
    };
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
        .map_err(|e| apply_failed(&e, event.id))?;
    Ok(Json(outcome))
}

/// The answer to a verified event that failed to apply: `503` when a
/// redelivery could succeed, `400` `invalid_event` when it never can (so the
/// platform stops redelivering it). Logged at `error` either way.
fn apply_failed(e: &cxp_core::Error, event_id: uuid::Uuid) -> ApiError {
    tracing::error!(error = %e.log_detail(), code = e.code(), retriable = e.retriable(),
        source = ?std::error::Error::source(e), %event_id, "could not apply a platform event");
    if e.retriable() {
        ApiError::unavailable("could not apply the event; redeliver it")
    } else {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_event",
            "this event cannot be applied; redelivering it will not help",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cxp_core::Error;

    #[test]
    fn only_a_failure_that_can_pass_asks_for_redelivery() {
        let id = uuid::Uuid::new_v4();
        let retry = apply_failed(&Error::Unavailable, id);
        assert_eq!(
            (retry.status, retry.code),
            (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        );
        let db_blip = apply_failed(&Error::Database(sqlx::Error::PoolTimedOut), id);
        assert_eq!(db_blip.status, StatusCode::SERVICE_UNAVAILABLE);

        for never in [
            Error::Invalid("bad org".into()),
            Error::Tenant(otto_tenant::Error::Invalid("bad org".into())),
            Error::Internal("bug".into()),
        ] {
            let e = apply_failed(&never, id);
            assert_eq!(
                (e.status, e.code),
                (StatusCode::BAD_REQUEST, "invalid_event"),
                "{never:?}"
            );
        }
    }
}
