//! `POST /platform/webhooks`: the otto platform's signed lifecycle events
//! (`org.deleted`, `member.removed`, `team.deleted`).
//!
//! **The status code is the retry protocol.** The platform redelivers anything
//! that is not a 2xx. A missing, malformed, stale, or wrong signature is `401`
//! and nothing is processed (the body is not parsed before the MAC checks
//! out); a signed but unreadable body is `400`; an applied, repeated, or
//! unknown event is `200 {"result": "applied" | "duplicate" | "ignored"}`; an
//! event that is itself at fault (its data is invalid: code
//! `invalid_argument`) is `400` `invalid_event`, so it is not retried forever;
//! any other failure to apply it (a database blip, an event this build parses
//! but has no handler for, a bug on this side) is `503`, so the platform
//! redelivers it and a fix deployed meanwhile applies it. Fails closed: with
//! no secret configured every delivery is refused.

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::State;
use axum::Json;
use cxp_core::platform_events::{self, Outcome};
use http::{HeaderMap, StatusCode};
use savvagent_resource::webhook::{self, WebhookError};

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

/// The answer to a verified event that failed to apply: `400`
/// `invalid_event` only when the event itself is at fault (its data is
/// invalid, so no redelivery can apply it and the platform should stop);
/// `503` for anything else, including a bug or misconfiguration here, so the
/// platform keeps redelivering until a fix applies it rather than the event
/// being dropped. Logged at `error` either way.
fn apply_failed(e: &cxp_core::Error, event_id: uuid::Uuid) -> ApiError {
    tracing::error!(error = %e.log_detail(), code = e.code(), retriable = e.retriable(),
        source = ?std::error::Error::source(e), %event_id, "could not apply a platform event");
    if e.code() == "invalid_argument" {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_event",
            "this event cannot be applied; redelivering it will not help",
        )
    } else {
        ApiError::unavailable("could not apply the event; redeliver it")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cxp_core::Error;

    /// Only an event at fault is refused for good; everything else, a bug
    /// on this side included, asks for redelivery so a fix can apply it.
    #[test]
    fn only_an_event_at_fault_stops_redelivery() {
        let id = uuid::Uuid::new_v4();
        for redeliver in [
            Error::Unavailable,
            Error::Database(sqlx::Error::PoolTimedOut),
            Error::Internal("bug".into()),
            Error::Tenant(savvagent_tenant::Error::Db(sqlx::Error::PoolClosed)),
            Error::Tenant(savvagent_tenant::Error::Config("bad key".into())),
            Error::Platform(savvagent_resource::Error::Unauthorized),
        ] {
            let e = apply_failed(&redeliver, id);
            assert_eq!(
                (e.status, e.code),
                (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
                "{redeliver:?}"
            );
        }

        for never in [
            Error::Invalid("bad org".into()),
            Error::Tenant(savvagent_tenant::Error::Invalid("bad org".into())),
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
