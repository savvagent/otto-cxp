use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::pagination::PaginatedResponse;
use super::state::ApiState;
use crate::db::customers::Customer360;
use crate::models::Customer;

#[derive(Debug, Deserialize)]
pub struct CreateCustomerBody {
    pub email: String,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCustomerBody {
    pub email: Option<String>,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CustomerFilterParams {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
}

pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<CustomerFilterParams>,
) -> Result<Json<PaginatedResponse<Customer>>, ApiError> {
    let data = state
        .customers
        .list(
            params.experience_id,
            params.application_id,
            params.page_id,
            params.component_id,
            params.limit,
            params.offset,
        )
        .await?;
    let total = state
        .customers
        .count(
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
) -> Result<Json<Customer>, ApiError> {
    state
        .customers
        .get(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Customer not found".into()))
}

pub async fn create(
    State(state): State<ApiState>,
    Json(body): Json<CreateCustomerBody>,
) -> Result<Json<Customer>, ApiError> {
    let metadata = body.metadata.unwrap_or(serde_json::Value::Object(Default::default()));
    let tags = body.tags.unwrap_or_default();
    let customer = state
        .customers
        .create(
            &body.email,
            body.name.as_deref(),
            body.phone.as_deref(),
            body.company.as_deref(),
            metadata,
            &tags,
        )
        .await?;
    Ok(Json(customer))
}

pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCustomerBody>,
) -> Result<Json<Customer>, ApiError> {
    state
        .customers
        .update(
            id,
            body.email.as_deref(),
            body.name.as_deref(),
            body.phone.as_deref(),
            body.company.as_deref(),
            body.metadata,
            body.tags.as_deref(),
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Customer not found".into()))
}

pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.customers.delete(id).await? {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(ApiError::NotFound("Customer not found".into()))
    }
}

pub async fn search(
    State(state): State<ApiState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Customer>>, ApiError> {
    let results = state.customers.search(&params.q).await?;
    Ok(Json(results))
}

pub async fn get_360(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Customer360>, ApiError> {
    state
        .customers
        .get_360(id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Customer not found".into()))
}
