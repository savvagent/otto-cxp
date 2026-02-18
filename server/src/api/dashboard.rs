use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::error::ApiError;
use super::state::ApiState;

#[derive(Debug, Serialize)]
pub struct DashboardMetrics {
    pub total_customers: i64,
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub total_feedback: i64,
    pub total_surveys: i64,
    pub total_experiences: i64,
}

pub async fn metrics(
    State(state): State<ApiState>,
) -> Result<Json<DashboardMetrics>, ApiError> {
    let total_customers = state.customers.count().await?;
    let total_tickets = state.tickets.count(None, None).await?;
    let open_tickets = state.tickets.count(Some("open"), None).await?;
    let total_feedback = state.feedback.count_feedback(None).await?;
    let total_surveys = state.feedback.count_surveys().await?;
    let total_experiences = state.experiences.count().await?;

    Ok(Json(DashboardMetrics {
        total_customers,
        total_tickets,
        open_tickets,
        total_feedback,
        total_surveys,
        total_experiences,
    }))
}
