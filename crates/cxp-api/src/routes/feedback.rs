//! `/api/v1/feedback` (list only; feedback is submitted over MCP).

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::feedback::{Feedback, FeedbackExt, FeedbackFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::Paginated;
use crate::extract::ApiQuery;
use crate::{ApiResult, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    #[serde(alias = "customer_id")]
    pub customer_id: Option<Uuid>,
    #[serde(alias = "feedback_type")]
    pub feedback_type: Option<String>,
    #[serde(alias = "experience_id")]
    pub experience_id: Option<Uuid>,
    #[serde(alias = "application_id")]
    pub application_id: Option<Uuid>,
    #[serde(alias = "page_id")]
    pub page_id: Option<Uuid>,
    #[serde(alias = "component_id")]
    pub component_id: Option<Uuid>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<ListParams>,
) -> ApiResult<Json<Paginated<Feedback>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = FeedbackFilter {
        customer_id: q.customer_id,
        feedback_type: q.feedback_type,
        experience_id: q.experience_id,
        application_id: q.application_id,
        page_id: q.page_id,
        component_id: q.component_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_feedback(&filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}
