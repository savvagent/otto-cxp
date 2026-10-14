//! axum's `Json`, `Query`, and `Path`, with their rejections answered as an
//! [`ApiError`] (`{"error", "code"}`) instead of axum's plain-text bodies. The
//! rejection text describes the caller's request (a missing field, a
//! malformed id), never server state.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use http::request::Parts;
use http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

pub(crate) fn rejected(status: StatusCode, text: String) -> ApiError {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::payload_too_large()
    } else {
        ApiError::invalid(text)
    }
}

/// A JSON request body.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Self(v)),
            Err(e @ JsonRejection::MissingJsonContentType(_)) => Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                e.body_text(),
            )),
            Err(e) => Err(rejected(e.status(), e.body_text())),
        }
    }
}

/// Query-string parameters.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|q| Self(q.0))
            .map_err(|e: QueryRejection| rejected(e.status(), e.body_text()))
    }
}

/// Path parameters (the `{id}` in a route).
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Path::<T>::from_request_parts(parts, state)
            .await
            .map(|p| Self(p.0))
            .map_err(|e: PathRejection| {
                if e.status().is_server_error() {
                    tracing::error!(error = %e.body_text(), "path extraction failed");
                    ApiError::new(e.status(), "internal", cxp_core::error::INTERNAL_MESSAGE)
                } else {
                    ApiError::invalid(e.body_text())
                }
            })
    }
}
