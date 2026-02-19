use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::{PaginatedResponse, PaginationParams};
use super::state::ApiState;
use crate::models::Team;
use axum::extract::Query;

#[derive(Debug, Deserialize)]
pub struct CreateTeamBody {
    pub name: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTeamBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<Team>>, ApiError> {
    let data = state.teams.list(params.limit, params.offset).await?;
    let total = state.teams.count().await?;
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
) -> Result<Json<Team>, ApiError> {
    state
        .teams
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Team not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateTeamBody>,
) -> Result<Json<Team>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let team = state
        .teams
        .create(&body.name, body.description.as_deref(), body.owner.as_deref(), metadata)
        .await?;
    Ok(Json(team))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTeamBody>,
) -> Result<Json<Team>, ApiError> {
    state
        .teams
        .update(id, body.name.as_deref(), body.description.as_deref(), body.owner.as_deref(), body.metadata)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Team not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.teams.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Team not found".into()))
    }
}
