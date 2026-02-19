use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::models::Page;

#[derive(Debug, Deserialize)]
pub struct CreatePageBody {
    pub application_id: Option<Uuid>,
    pub name: String,
    pub path: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePageBody {
    pub name: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct PageFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub application_id: Option<Uuid>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<PageFilterParams>,
) -> Result<Json<PaginatedResponse<Page>>, ApiError> {
    let data = if let Some(app_id) = params.application_id {
        state
            .pages
            .list_by_application(app_id, params.limit, params.offset)
            .await?
    } else {
        state.pages.list(params.limit, params.offset).await?
    };
    let total = state.pages.count(params.application_id).await?;
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
) -> Result<Json<Page>, ApiError> {
    state
        .pages
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Page not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreatePageBody>,
) -> Result<Json<Page>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let page = state
        .pages
        .create(
            body.application_id,
            &body.name,
            body.path.as_deref(),
            body.description.as_deref(),
            body.team_id,
            metadata,
        )
        .await?;
    Ok(Json(page))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdatePageBody>,
) -> Result<Json<Page>, ApiError> {
    state
        .pages
        .update(
            id,
            body.name.as_deref(),
            body.path.as_deref(),
            body.description.as_deref(),
            body.team_id,
            body.metadata,
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Page not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.pages.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Page not found".into()))
    }
}
