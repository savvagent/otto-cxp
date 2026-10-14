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
//! | `quota_exceeded` | 429 (the org's plan allowance is used up; reads still work) |
//! | `payload_too_large` | 413 |
//! | `unsupported_media_type` | 415 (a JSON body without `Content-Type: application/json`) |
//! | `method_not_allowed` | 405 (a known `/api/v1` path, another method) |
//! | `unavailable` | 503, with `Retry-After: 5` |
//! | `internal` | 500 |

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
}

fn status_for(code: &str) -> StatusCode {
    match code {
        "not_found" => StatusCode::NOT_FOUND,
        "invalid_argument" | "unknown_team" => StatusCode::BAD_REQUEST,
        "conflict" => StatusCode::CONFLICT,
        "access_revoked" => StatusCode::UNAUTHORIZED,
        "quota_exceeded" => StatusCode::TOO_MANY_REQUESTS,
        "unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl From<cxp_core::Error> for ApiError {
    fn from(e: cxp_core::Error) -> Self {
        let code = e.code();
        if e.is_internal() || code == "internal" {
            tracing::error!(error = %e, code, source = ?std::error::Error::source(&e),
                "REST request failed");
        }
        Self::new(status_for(code), code, e.to_string())
    }
}

impl From<otto_tenant::Error> for ApiError {
    fn from(e: otto_tenant::Error) -> Self {
        cxp_core::Error::from(e).into()
    }
}

impl From<otto_resource::Error> for ApiError {
    fn from(e: otto_resource::Error) -> Self {
        cxp_core::Error::Platform(e).into()
    }
}

impl From<MissingScope> for ApiError {
    fn from(e: MissingScope) -> Self {
        let mut err = Self::new(StatusCode::FORBIDDEN, "insufficient_scope", e.to_string());
        err.challenge = Some(format!(
            r#"Bearer realm="{}", error="insufficient_scope", scope="{}""#,
            cxp_core::scopes::RESOURCE_NAME,
            e.0
        ));
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
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cxp_core::Error;

    #[test]
    fn every_domain_code_has_its_status() {
        let cases = [
            (Error::NotFound { kind: "customer" }, StatusCode::NOT_FOUND),
            (Error::Invalid("bad".into()), StatusCode::BAD_REQUEST),
            (Error::UnknownTeam, StatusCode::BAD_REQUEST),
            (Error::Conflict("dup".into()), StatusCode::CONFLICT),
            (Error::AccessRevoked, StatusCode::UNAUTHORIZED),
            (
                Error::QuotaExceeded {
                    tool: "create_survey".into(),
                    used: 10,
                    included: 10,
                    plan: "free".into(),
                    upgrade_url: "https://otto.example.com".into(),
                },
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (Error::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
            (
                Error::Platform(otto_resource::Error::Unauthorized),
                StatusCode::SERVICE_UNAVAILABLE,
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
            r#"Bearer realm="otto-cxp", error="insufficient_scope", scope="cxp:write""#
        );
    }
}
