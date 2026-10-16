//! `/api/v1/customers`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::customers::{
    Customer, Customer360, CustomerFilter, CustomerUpdate, CustomersExt, NewCustomer,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{deleted, CappedPaginated, Paginated};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{ApiResult, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    pub email: String,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBody {
    pub email: Option<String>,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    #[serde(alias = "experience_id")]
    pub experience_id: Option<Uuid>,
    #[serde(alias = "application_id")]
    pub application_id: Option<Uuid>,
    #[serde(alias = "page_id")]
    pub page_id: Option<Uuid>,
    #[serde(alias = "component_id")]
    pub component_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<ListParams>,
) -> ApiResult<Json<Paginated<Customer>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = CustomerFilter {
        experience_id: q.experience_id,
        application_id: q.application_id,
        page_id: q.page_id,
        component_id: q.component_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_customers(&filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

/// The best matches first, in the list envelope plus `totalCapped`
/// ([`CappedPaginated`]); paged by `limit`/`offset` (default 50).
pub async fn search(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<SearchParams>,
) -> ApiResult<Json<CappedPaginated<Customer>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let (permit, mut tx) = state.begin_search(&caller).await?;
    let found = tx.search_customers(&permit, &q.q, page).await?;
    tx.commit().await?;
    Ok(Json(CappedPaginated::new(found, page)))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Customer>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let customer = tx.get_customer(id).await?;
    tx.commit().await?;
    Ok(Json(customer))
}

pub async fn get_360(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Customer360>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let view = tx.customer_360(id).await?;
    tx.commit().await?;
    Ok(Json(view))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<Json<Customer>> {
    caller.require_write()?;
    let new = NewCustomer {
        email: body.email,
        name: body.name,
        phone: body.phone,
        company: body.company,
        metadata: body.metadata,
        tags: body.tags.unwrap_or_default(),
    };
    let mut tx = state.begin(&caller).await?;
    let customer = tx.create_customer(&new).await?;
    tx.commit().await?;
    Ok(Json(customer))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> ApiResult<Json<Customer>> {
    caller.require_write()?;
    let update = CustomerUpdate {
        email: body.email,
        name: body.name,
        phone: body.phone,
        company: body.company,
        metadata: body.metadata,
        tags: body.tags,
    };
    let mut tx = state.begin(&caller).await?;
    let customer = tx.update_customer(id, &update).await?;
    tx.commit().await?;
    Ok(Json(customer))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_customer(id).await?;
    tx.commit().await?;
    Ok(deleted())
}
