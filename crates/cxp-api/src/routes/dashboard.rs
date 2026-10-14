//! `/api/v1/dashboard/metrics`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::dashboard::DashboardExt;
use serde::Serialize;

use crate::{ApiResult, AppState};

/// The headline counts, with the snake_case keys this route has always
/// answered with (the console reads `total_customers`, ...), unlike the
/// camelCase `cxp_core` model.
#[derive(Debug, Serialize)]
pub struct Metrics {
    pub total_customers: i64,
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub total_feedback: i64,
    pub total_surveys: i64,
    pub total_experiences: i64,
}

pub async fn metrics(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
) -> ApiResult<Json<Metrics>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let m = tx.dashboard_metrics().await?;
    tx.commit().await?;
    Ok(Json(Metrics {
        total_customers: m.total_customers,
        total_tickets: m.total_tickets,
        open_tickets: m.open_tickets,
        total_feedback: m.total_feedback,
        total_surveys: m.total_surveys,
        total_experiences: m.total_experiences,
    }))
}
