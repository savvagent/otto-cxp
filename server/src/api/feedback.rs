use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::models::Feedback;

#[derive(Debug, Deserialize)]
pub struct FeedbackFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub customer_id: Option<Uuid>,
    pub feedback_type: Option<String>,
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
    Query(params): Query<FeedbackFilterParams>,
) -> Result<Json<PaginatedResponse<Feedback>>, ApiError> {
    let data = state
        .feedback
        .list_feedback(
            params.customer_id,
            params.feedback_type.as_deref(),
            params.experience_id,
            params.application_id,
            params.page_id,
            params.component_id,
            params.limit,
            params.offset,
        )
        .await?;
    let total = state
        .feedback
        .count_feedback(
            params.feedback_type.as_deref(),
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
