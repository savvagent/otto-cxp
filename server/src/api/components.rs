use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::models::Component;

#[derive(Debug, Deserialize)]
pub struct CreateComponentBody {
    pub page_id: Option<Uuid>,
    pub name: String,
    pub component_type: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateComponentBody {
    pub name: Option<String>,
    pub component_type: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub page_id: Option<Uuid>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<ComponentFilterParams>,
) -> Result<Json<PaginatedResponse<Component>>, ApiError> {
    let data = if let Some(page_id) = params.page_id {
        state
            .components
            .list_by_page(page_id, params.limit, params.offset)
            .await?
    } else {
        state.components.list(params.limit, params.offset).await?
    };
    let total = state.components.count(params.page_id).await?;
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
) -> Result<Json<Component>, ApiError> {
    state
        .components
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Component not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateComponentBody>,
) -> Result<Json<Component>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let component = state
        .components
        .create(
            body.page_id,
            &body.name,
            body.component_type.as_deref(),
            body.description.as_deref(),
            body.team_id,
            metadata,
        )
        .await?;
    Ok(Json(component))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateComponentBody>,
) -> Result<Json<Component>, ApiError> {
    state
        .components
        .update(
            id,
            body.name.as_deref(),
            body.component_type.as_deref(),
            body.description.as_deref(),
            body.team_id,
            body.metadata,
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Component not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.components.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Component not found".into()))
    }
}
