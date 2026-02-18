use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::{PaginatedResponse, PaginationParams};
use super::state::ApiState;
use crate::models::Experience;

#[derive(Debug, Deserialize)]
pub struct CreateExperienceBody {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExperienceBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<Experience>>, ApiError> {
    let data = state.experiences.list(params.limit, params.offset).await?;
    let total = state.experiences.count().await?;
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
) -> Result<Json<Experience>, ApiError> {
    state
        .experiences
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Experience not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateExperienceBody>,
) -> Result<Json<Experience>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let experience = state
        .experiences
        .create(&body.name, body.description.as_deref(), metadata)
        .await?;
    Ok(Json(experience))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateExperienceBody>,
) -> Result<Json<Experience>, ApiError> {
    state
        .experiences
        .update(id, body.name.as_deref(), body.description.as_deref(), body.metadata)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Experience not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.experiences.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Experience not found".into()))
    }
}
