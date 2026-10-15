//! `/api/v1/dashboard/metrics`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::dashboard::{DashboardExt, DashboardMetrics};
use serde::Serialize;

use crate::{ApiResult, AppState};

/// The headline counts: the camelCase keys of the convention, plus the
/// snake_case keys this route answered with before #24.
#[derive(Debug, Serialize)]
pub struct Metrics {
    #[serde(flatten)]
    pub current: DashboardMetrics,
    #[serde(flatten)]
    pub legacy: LegacyMetrics,
}

/// Deprecated: the pre-#24 snake_case keys, the same values as `current`.
/// Kept so consumers that read `total_customers` keep working; removal is
/// tracked in #35.
#[derive(Debug, Serialize)]
pub struct LegacyMetrics {
    pub total_customers: i64,
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub total_feedback: i64,
    pub total_surveys: i64,
    pub total_experiences: i64,
}

impl From<DashboardMetrics> for Metrics {
    fn from(m: DashboardMetrics) -> Self {
        Self {
            legacy: LegacyMetrics {
                total_customers: m.total_customers,
                total_tickets: m.total_tickets,
                open_tickets: m.open_tickets,
                total_feedback: m.total_feedback,
                total_surveys: m.total_surveys,
                total_experiences: m.total_experiences,
            },
            current: m,
        }
    }
}

pub async fn metrics(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
) -> ApiResult<Json<Metrics>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let m = tx.dashboard_metrics().await?;
    tx.commit().await?;
    Ok(Json(Metrics::from(m)))
}
