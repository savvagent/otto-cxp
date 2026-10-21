//! `/api/v1/surveys`. Free, like the `create_survey` tool: no route here
//! records usage.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::feedback::{
    FeedbackExt, NewSurvey, Survey, SurveyFilter, SurveyResponse, SurveyUpdate,
};
use serde::Deserialize;
use uuid::Uuid;

use super::{deleted, PageParams, Paginated};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{ApiResult, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    pub title: String,
    pub description: Option<String>,
    pub questions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBody {
    pub title: Option<String>,
    pub description: Option<String>,
    pub questions: Option<serde_json::Value>,
    pub active: Option<bool>,
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

pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<ListParams>,
) -> ApiResult<Json<Paginated<Survey>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = SurveyFilter {
        experience_id: q.experience_id,
        application_id: q.application_id,
        page_id: q.page_id,
        component_id: q.component_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_surveys(&filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Survey>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let survey = tx.get_survey(id).await?;
    tx.commit().await?;
    Ok(Json(survey))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateBody>,
) -> ApiResult<Json<Survey>> {
    caller.require_write()?;
    let new = NewSurvey {
        title: body.title,
        description: body.description,
        questions: body.questions,
        ..Default::default()
    };
    let mut tx = state.begin(&caller).await?;
    let survey = tx.create_survey(&new).await?;
    tx.commit().await?;
    Ok(Json(survey))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> ApiResult<Json<Survey>> {
    caller.require_write()?;
    let update = SurveyUpdate {
        title: body.title,
        description: body.description,
        questions: body.questions,
        active: body.active,
    };
    let mut tx = state.begin(&caller).await?;
    let survey = tx.update_survey(id, &update).await?;
    tx.commit().await?;
    Ok(Json(survey))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_survey(id).await?;
    tx.commit().await?;
    Ok(deleted())
}

/// One page of the survey's responses, newest first, in the [`Paginated`]
/// envelope (`limit`/`offset`, default 50). `404` when the survey is not in the
/// caller's org.
pub async fn responses(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(q): ApiQuery<PageParams>,
) -> ApiResult<Json<Paginated<SurveyResponse>>> {
    caller.require_read()?;
    let page = q.page();
    let mut tx = state.begin(&caller).await?;
    let found = tx.list_survey_responses(id, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(found, page)))
}
