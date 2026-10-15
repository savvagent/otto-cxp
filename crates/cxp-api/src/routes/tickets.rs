//! `/api/v1/tickets`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::tickets::{NewTicket, Ticket, TicketFilter, TicketUpdate, TicketsExt};
use serde::Deserialize;
use uuid::Uuid;

use super::{deleted, Paginated};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{ApiResult, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    #[serde(alias = "customer_id")]
    pub customer_id: Uuid,
    pub subject: String,
    pub description: Option<String>,
    /// Defaults to `medium`.
    pub priority: Option<String>,
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBody {
    pub status: Option<String>,
    pub priority: Option<String>,
    #[serde(alias = "assigned_to")]
    pub assigned_to: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub status: Option<String>,
    pub priority: Option<String>,
    #[serde(alias = "customer_id")]
    pub customer_id: Option<Uuid>,
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
) -> ApiResult<Json<Paginated<Ticket>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = TicketFilter {
        customer_id: q.customer_id,
        status: q.status,
        priority: q.priority,
        experience_id: q.experience_id,
        application_id: q.application_id,
        page_id: q.page_id,
        component_id: q.component_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_tickets(&filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Ticket>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let ticket = tx.get_ticket(id).await?;
    tx.commit().await?;
    Ok(Json(ticket))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<Json<Ticket>> {
    caller.require_write()?;
    let new = NewTicket {
        customer_id: body.customer_id,
        subject: body.subject,
        description: body.description,
        priority: body.priority,
        channel: body.channel,
    };
    let mut tx = state.begin(&caller).await?;
    let ticket = tx.create_ticket(&new).await?;
    tx.commit().await?;
    Ok(Json(ticket))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> ApiResult<Json<Ticket>> {
    caller.require_write()?;
    let update = TicketUpdate {
        status: body.status,
        priority: body.priority,
        assigned_to: body.assigned_to,
        subject: body.subject,
        description: body.description,
        tags: body.tags,
    };
    let mut tx = state.begin(&caller).await?;
    let ticket = tx.update_ticket(id, &update).await?;
    tx.commit().await?;
    Ok(Json(ticket))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_ticket(id).await?;
    tx.commit().await?;
    Ok(deleted())
}
