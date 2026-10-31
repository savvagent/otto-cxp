//! The REST error body: `{"error": text, "code": code}`.
//!
//! `code` is the stable, machine-readable value a client branches on (the same
//! codes MCP puts in `data.code`); `error` is for a person. Neither ever holds
//! database or platform text: a domain error's `Display` is already free of
//! it, and internal failures are logged here with their source and answered
//! with a generic message.
//!
//! | code | status |
//! |---|---|
//! | `not_found` | 404 (also any unknown `/api/v1` path) |
//! | `invalid_argument`, `unknown_team` | 400 |
//! | `conflict` | 409 |
//! | `insufficient_scope` | 403, with `WWW-Authenticate: Bearer error="insufficient_scope", scope="…"` (RFC 6750 §3.1) |
//! | `access_revoked` | 401, with the `WWW-Authenticate` challenge |
//! | `invalid_signature` | 401 (`/platform/webhooks` only) |
//! | `budget_exceeded`, `payment_required` | 402 (a billable call while billing is blocked: this service's budget hard stop, or a payment method needed; reads and free routes still work) |
//! | `payload_too_large` | 413 |
//! | `unsupported_media_type` | 415 (a JSON body without `Content-Type: application/json`) |
//! | `method_not_allowed` | 405 (a known `/api/v1` path, another method) |
//! | `unavailable` | 503, with `Retry-After` ([`cxp_core::auth::RETRY_AFTER_SECONDS`]): only a failure that can pass, or a search finding every search permit taken (#42) |
//! | `query_too_broad` | 422, no `Retry-After`: a search ran past its time limit; narrow the query (repeating it unchanged cannot help) |
//! | `internal` | 500, never with `Retry-After` (including a platform failure a retry cannot fix, such as this service's credential being refused) |

use axum::response::{IntoResponse, Response};
use axum::Json;
use cxp_core::auth::MissingScope;
use http::{header, HeaderValue, StatusCode};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// A `WWW-Authenticate` value to send with the response, if any.
    challenge: Option<String>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            challenge: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_argument", message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    /// A body over [`crate::MAX_BODY_BYTES`], from any extractor that reads one.
    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!(
                "the request body is larger than {} KiB",
                crate::MAX_BODY_BYTES / 1024
            ),
        )
    }
}

/// The status for a domain error, decided by its variant. The match is
/// exhaustive on purpose: a new variant does not compile until it is given a
/// status here, rather than falling through to 500. The two variants whose
/// code depends on what failed inside them (a database or tenant error the
/// caller's input caused is `invalid_argument`) defer to that code.
fn status_for(e: &cxp_core::Error) -> StatusCode {
    use cxp_core::Error;
    match e {
        Error::NotFound { .. } => StatusCode::NOT_FOUND,
        Error::Invalid(_) | Error::UnknownTeam => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::AccessRevoked => StatusCode::UNAUTHORIZED,
        Error::BillingBlocked { .. } => StatusCode::PAYMENT_REQUIRED,
        Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        // The caller's query, not the server, is the problem, and repeating
        // it unchanged cannot help: a 4xx, so no generic retry layer repeats
        // a 2 s query on the small shared pool.
        Error::QueryTooBroad => StatusCode::UNPROCESSABLE_ENTITY,
        // Split by retriability: only a failure that can pass is a 503 (and
        // so gets `Retry-After`); a platform that no longer knows the org is a
        // revocation; anything else is ours to fix, not the caller's to retry.
        Error::Platform(p) if p.is_retriable() => StatusCode::SERVICE_UNAVAILABLE,
        Error::Platform(savvagent_resource::Error::NotFound) => StatusCode::UNAUTHORIZED,
        Error::Platform(_) | Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        Error::Tenant(_) | Error::Database(_) => {
            if e.code() == "invalid_argument" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl From<cxp_core::Error> for ApiError {
    fn from(e: cxp_core::Error) -> Self {
        let code = e.code();
        if e.is_internal() || code == "internal" {
            tracing::error!(error = %e.log_detail(), code,
                source = ?std::error::Error::source(&e), "REST request failed");
        }
        Self::new(status_for(&e), code, e.to_string())
    }
}

impl From<savvagent_tenant::Error> for ApiError {
    fn from(e: savvagent_tenant::Error) -> Self {
        cxp_core::Error::from(e).into()
    }
}

impl From<savvagent_resource::Error> for ApiError {
    fn from(e: savvagent_resource::Error) -> Self {
        cxp_core::Error::Platform(e).into()
    }
}

impl From<MissingScope> for ApiError {
    fn from(e: MissingScope) -> Self {
        let mut err = Self::new(StatusCode::FORBIDDEN, "insufficient_scope", e.to_string());
        err.challenge = Some(cxp_core::auth::scope_challenge(e.0));
        err
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (
            self.status,
            Json(serde_json::json!({ "error": self.message, "code": self.code })),
        )
            .into_response();
        if let Some(value) = self
            .challenge
            .as_deref()
            .and_then(|c| HeaderValue::from_str(c).ok())
        {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
        }
        if self.status == StatusCode::SERVICE_UNAVAILABLE {
            res.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(cxp_core::auth::RETRY_AFTER_SECONDS),
            );
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cxp_core::error::BlockedReason;
    use cxp_core::Error;

    fn blocked(reason: BlockedReason) -> Error {
        Error::BillingBlocked {
            reason,
            tool: "send_survey".into(),
            url: "https://otto.example.com".into(),
        }
    }

    /// A billable call refused while billing is blocked is a 402 with the
    /// usual `{error, code}` body, and nothing tells the client to retry.
    #[tokio::test]
    async fn billing_refusals_are_payment_required() {
        for code in ["budget_exceeded", "payment_required"] {
            let reason = if code == "budget_exceeded" {
                BlockedReason::BudgetExceeded
            } else {
                BlockedReason::PaymentRequired
            };
            let res = ApiError::from(blocked(reason)).into_response();
            assert_eq!(res.status(), StatusCode::PAYMENT_REQUIRED, "{code}");
            assert!(res.headers().get(header::RETRY_AFTER).is_none(), "{code}");
            let body = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["code"], code);
            let text = body["error"].as_str().unwrap();
            assert!(text.contains("send_survey"), "{text}");
            assert_eq!(body.as_object().unwrap().len(), 2, "{body}");
        }
    }

    #[test]
    fn every_domain_code_has_its_status() {
        let cases = [
            (Error::NotFound { kind: "customer" }, StatusCode::NOT_FOUND),
            (Error::Invalid("bad".into()), StatusCode::BAD_REQUEST),
            (Error::UnknownTeam, StatusCode::BAD_REQUEST),
            (Error::Conflict("dup".into()), StatusCode::CONFLICT),
            (Error::AccessRevoked, StatusCode::UNAUTHORIZED),
            (
                blocked(BlockedReason::BudgetExceeded),
                StatusCode::PAYMENT_REQUIRED,
            ),
            (
                blocked(BlockedReason::PaymentRequired),
                StatusCode::PAYMENT_REQUIRED,
            ),
            (Error::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
            (Error::QueryTooBroad, StatusCode::UNPROCESSABLE_ENTITY),
            (
                Error::Platform(savvagent_resource::Error::Status {
                    status: 503,
                    body: String::new(),
                }),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                Error::Platform(savvagent_resource::Error::NotFound),
                StatusCode::UNAUTHORIZED,
            ),
            (
                Error::Platform(savvagent_resource::Error::Unauthorized),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                Error::Internal("bug".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                Error::Tenant(savvagent_tenant::Error::Invalid("bad".into())),
                StatusCode::BAD_REQUEST,
            ),
            (
                Error::Database(sqlx::Error::RowNotFound),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (e, status) in cases {
            let code = e.code();
            let api = ApiError::from(e);
            assert_eq!(api.status, status, "{code}");
            assert_eq!(api.code, code);
        }
        let scope = ApiError::from(MissingScope("cxp:write"));
        assert_eq!(
            (scope.status, scope.code),
            (StatusCode::FORBIDDEN, "insufficient_scope")
        );
        let res = scope.into_response();
        assert_eq!(
            res.headers()[header::WWW_AUTHENTICATE],
            r#"Bearer realm="Savvagent CXP", error="insufficient_scope", scope="cxp:write""#
        );
    }

    #[test]
    fn unavailable_tells_the_client_when_to_retry() {
        let res = ApiError::from(Error::Unavailable).into_response();
        assert_eq!(
            res.headers()[header::RETRY_AFTER],
            cxp_core::auth::RETRY_AFTER_SECONDS
        );
        // A query too broad to finish is the caller's to narrow, not to retry.
        let err = ApiError::from(Error::QueryTooBroad);
        assert_eq!(err.code, "query_too_broad");
        let res = err.into_response();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(res.headers().get(header::RETRY_AFTER).is_none());
        // A platform failure no retry can fix is not told to retry.
        let res = ApiError::from(Error::Platform(savvagent_resource::Error::Unauthorized))
            .into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(res.headers().get(header::RETRY_AFTER).is_none());
        let big = ApiError::payload_too_large();
        assert_eq!(
            (big.status, big.code),
            (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
        );
    }
}
