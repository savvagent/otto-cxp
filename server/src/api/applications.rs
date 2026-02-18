use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::models::Application;

#[derive(Debug, Deserialize)]
pub struct CreateApplicationBody {
    pub experience_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub platform: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApplicationBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub platform: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ApplicationFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub experience_id: Option<Uuid>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<ApplicationFilterParams>,
) -> Result<Json<PaginatedResponse<Application>>, ApiError> {
    let data = if let Some(exp_id) = params.experience_id {
        state
            .applications
            .list_by_experience(exp_id, params.limit, params.offset)
            .await?
    } else {
        state.applications.list(params.limit, params.offset).await?
    };
    let total = state.applications.count(params.experience_id).await?;
    Ok(Json(PaginatedResponse {
        data,
        total,
        limit: params.limit,
        offset: params.offset,
    }))
}

pub async fn get(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Application>, ApiError> {
    state
        .applications
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Application not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateApplicationBody>,
) -> Result<Json<Application>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let app = state
        .applications
        .create(
            body.experience_id,
            &body.name,
            body.description.as_deref(),
            body.platform.as_deref(),
            metadata,
        )
        .await?;
    Ok(Json(app))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateApplicationBody>,
) -> Result<Json<Application>, ApiError> {
    state
        .applications
        .update(
            id,
            body.name.as_deref(),
            body.description.as_deref(),
            body.platform.as_deref(),
            body.metadata,
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Application not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.applications.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Application not found".into()))
    }
}
