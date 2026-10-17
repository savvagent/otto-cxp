//! `/api/v1/dashboard/metrics`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::dashboard::{DashboardExt, DashboardMetrics};
use serde::Serialize;

use crate::{ApiResult, AppState};

/// The headline counts: the camelCase keys of the `/api/v1` convention.
#[derive(Debug, Serialize)]
pub struct Metrics {
    #[serde(flatten)]
    current: DashboardMetrics,
}

impl From<DashboardMetrics> for Metrics {
    fn from(m: DashboardMetrics) -> Self {
        Self { current: m }
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
