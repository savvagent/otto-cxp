use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::models::Ticket;

#[derive(Debug, Deserialize)]
pub struct CreateTicketBody {
    pub customer_id: Uuid,
    pub subject: String,
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub channel: Option<String>,
}

fn default_priority() -> String {
    "medium".into()
}

#[derive(Debug, Deserialize)]
pub struct UpdateTicketBody {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assigned_to: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct TicketFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub customer_id: Option<Uuid>,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<TicketFilterParams>,
) -> Result<Json<PaginatedResponse<Ticket>>, ApiError> {
    let data = state
        .tickets
        .list(
            params.customer_id,
            params.status.as_deref(),
            params.priority.as_deref(),
            params.experience_id,
            params.application_id,
            params.page_id,
            params.component_id,
            params.limit,
            params.offset,
        )
        .await?;
    let total = state
        .tickets
        .count(
            params.status.as_deref(),
            params.priority.as_deref(),
            params.experience_id,
            params.application_id,
            params.page_id,
            params.component_id,
        )
        .await?;
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
) -> Result<Json<Ticket>, ApiError> {
    state
        .tickets
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Ticket not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateTicketBody>,
) -> Result<Json<Ticket>, ApiError> {
    let ticket = state
        .tickets
        .create(
            body.customer_id,
            &body.subject,
            body.description.as_deref(),
            &body.priority,
            body.channel.as_deref(),
        )
        .await?;
    Ok(Json(ticket))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTicketBody>,
) -> Result<Json<Ticket>, ApiError> {
    state
        .tickets
        .update(
            id,
            body.status.as_deref(),
            body.priority.as_deref(),
            body.assigned_to.as_deref(),
            body.tags.as_deref(),
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Ticket not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.tickets.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Ticket not found".into()))
    }
}
