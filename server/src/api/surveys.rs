use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::{PaginatedResponse, PaginationParams};
use super::state::ApiState;
use crate::models::feedback::{Survey, SurveyResponse};

#[derive(Debug, Deserialize)]
pub struct CreateSurveyBody {
    pub title: String,
    pub description: Option<String>,
    pub questions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSurveyBody {
    pub title: Option<String>,
    pub description: Option<String>,
    pub questions: Option<serde_json::Value>,
    pub active: Option<bool>,
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<Survey>>, ApiError> {
    let data = state.feedback.list_surveys(params.limit, params.offset).await?;
    let total = state.feedback.count_surveys().await?;
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
) -> Result<Json<Survey>, ApiError> {
    state
        .feedback
        .get_survey(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Survey not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateSurveyBody>,
) -> Result<Json<Survey>, ApiError> {
    let survey = state
        .feedback
        .create_survey(&body.title, body.description.as_deref(), body.questions)
        .await?;
    Ok(Json(survey))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSurveyBody>,
) -> Result<Json<Survey>, ApiError> {
    state
        .feedback
        .update_survey(id, body.title.as_deref(), body.description.as_deref(), body.questions, body.active)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Survey not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.feedback.delete_survey(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Survey not found".into()))
    }
}

pub async fn get_responses(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<SurveyResponse>>, ApiError> {
    let responses = state.feedback.get_responses(id).await?;
    Ok(Json(responses))
}
