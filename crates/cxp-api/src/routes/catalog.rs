//! `/api/v1/{experiences,applications,pages,components}`: the experience
//! catalog, each entity optionally owned by a platform team.
//!
//! **The team field.** Bodies take `team_id` and, until the console sends
//! camelCase (#15), `teamId` too (sending both is a `400`). On create, absent
//! or `null` means no team. On update, absent keeps the team, an explicit
//! `null` clears it, and an id sets it. Any id is verified at the platform
//! (`cxp_core::catalog::verify_team`) before the transaction opens: a team the
//! platform does not know in the caller's org is `400 unknown_team`, and a
//! platform that cannot answer is `503`.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use cxp_core::catalog::{
    Application, ApplicationFilter, ApplicationUpdate, CatalogExt, Component, ComponentFilter,
    ComponentUpdate, Experience, ExperienceUpdate, NewApplication, NewComponent, NewExperience,
    NewPage, Page, PageFilter, PageUpdate, TeamChange, VerifiedTeam,
};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use super::{deleted, PageParams, Paginated};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{ApiResult, AppState};

/// Deserializes a field that is present (even as `null`) to `Some(_)`; with
/// `#[serde(default)]` an absent field stays `None`. So `Option<Option<T>>`
/// tells "absent" from "explicitly null".
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// A create body's team, verified at the platform in the caller's org. Called
/// before the transaction opens, so no pooled connection waits on the
/// platform.
async fn new_team(
    state: &AppState,
    caller: &Principal,
    team: Option<Uuid>,
) -> ApiResult<Option<VerifiedTeam>> {
    match team {
        None => Ok(None),
        Some(id) => Ok(Some(
            cxp_core::catalog::verify_team(&state.platform, caller.org_id, id).await?,
        )),
    }
}

/// An update body's team: absent keeps it, an explicit `null` clears it, an
/// id (verified, as for [`new_team`]) sets it.
async fn team_change(
    state: &AppState,
    caller: &Principal,
    team: Option<Option<Uuid>>,
) -> ApiResult<TeamChange> {
    Ok(match team {
        None => TeamChange::Keep,
        Some(None) => TeamChange::Clear,
        Some(Some(id)) => TeamChange::Set(
            cxp_core::catalog::verify_team(&state.platform, caller.org_id, id).await?,
        ),
    })
}

// ---------------------------------------------------------------- experiences

#[derive(Debug, Deserialize)]
pub struct CreateExperienceBody {
    pub name: String,
    pub description: Option<String>,
    #[serde(default, alias = "teamId")]
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExperienceBody {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "teamId", deserialize_with = "present")]
    pub team_id: Option<Option<Uuid>>,
    pub metadata: Option<serde_json::Value>,
}

pub async fn list_experiences(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<PageParams>,
) -> ApiResult<Json<Paginated<Experience>>> {
    caller.require_read()?;
    let page = q.page();
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_experiences(page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get_experience(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Experience>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let row = tx.get_experience(id).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn create_experience(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateExperienceBody>,
) -> ApiResult<Json<Experience>> {
    caller.require_write()?;
    let team = new_team(&state, &caller, body.team_id).await?;
    let new = NewExperience {
        name: body.name,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.create_experience(&new).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn update_experience(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateExperienceBody>,
) -> ApiResult<Json<Experience>> {
    caller.require_write()?;
    let team = team_change(&state, &caller, body.team_id).await?;
    let update = ExperienceUpdate {
        name: body.name,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.update_experience(id, &update).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn delete_experience(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_experience(id).await?;
    tx.commit().await?;
    Ok(deleted())
}

// ---------------------------------------------------------------- applications

#[derive(Debug, Deserialize)]
pub struct CreateApplicationBody {
    pub experience_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub platform: Option<String>,
    #[serde(default, alias = "teamId")]
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateApplicationBody {
    pub name: Option<String>,
    pub description: Option<String>,
    pub platform: Option<String>,
    #[serde(default, alias = "teamId", deserialize_with = "present")]
    pub team_id: Option<Option<Uuid>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ApplicationParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub experience_id: Option<Uuid>,
}

pub async fn list_applications(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<ApplicationParams>,
) -> ApiResult<Json<Paginated<Application>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = ApplicationFilter {
        experience_id: q.experience_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_applications(filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get_application(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Application>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let row = tx.get_application(id).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn create_application(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateApplicationBody>,
) -> ApiResult<Json<Application>> {
    caller.require_write()?;
    let team = new_team(&state, &caller, body.team_id).await?;
    let new = NewApplication {
        experience_id: body.experience_id,
        name: body.name,
        platform: body.platform,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.create_application(&new).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn update_application(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateApplicationBody>,
) -> ApiResult<Json<Application>> {
    caller.require_write()?;
    let team = team_change(&state, &caller, body.team_id).await?;
    let update = ApplicationUpdate {
        name: body.name,
        platform: body.platform,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.update_application(id, &update).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn delete_application(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_application(id).await?;
    tx.commit().await?;
    Ok(deleted())
}

// ---------------------------------------------------------------- pages

#[derive(Debug, Deserialize)]
pub struct CreatePageBody {
    pub application_id: Option<Uuid>,
    pub name: String,
    pub path: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "teamId")]
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePageBody {
    pub name: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "teamId", deserialize_with = "present")]
    pub team_id: Option<Option<Uuid>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct PageParamsByApplication {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub application_id: Option<Uuid>,
}

pub async fn list_pages(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<PageParamsByApplication>,
) -> ApiResult<Json<Paginated<Page>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = PageFilter {
        application_id: q.application_id,
    };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_pages(filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get_page(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Page>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let row = tx.get_page(id).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn create_page(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreatePageBody>,
) -> ApiResult<Json<Page>> {
    caller.require_write()?;
    let team = new_team(&state, &caller, body.team_id).await?;
    let new = NewPage {
        application_id: body.application_id,
        name: body.name,
        path: body.path,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.create_page(&new).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn update_page(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdatePageBody>,
) -> ApiResult<Json<Page>> {
    caller.require_write()?;
    let team = team_change(&state, &caller, body.team_id).await?;
    let update = PageUpdate {
        name: body.name,
        path: body.path,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.update_page(id, &update).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn delete_page(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_page(id).await?;
    tx.commit().await?;
    Ok(deleted())
}

// ---------------------------------------------------------------- components

#[derive(Debug, Deserialize)]
pub struct CreateComponentBody {
    pub page_id: Option<Uuid>,
    pub name: String,
    pub component_type: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "teamId")]
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateComponentBody {
    pub name: Option<String>,
    pub component_type: Option<String>,
    pub description: Option<String>,
    #[serde(default, alias = "teamId", deserialize_with = "present")]
    pub team_id: Option<Option<Uuid>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub page_id: Option<Uuid>,
}

pub async fn list_components(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<ComponentParams>,
) -> ApiResult<Json<Paginated<Component>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let filter = ComponentFilter { page_id: q.page_id };
    let mut tx = state.begin(&caller).await?;
    let rows = tx.list_components(filter, page).await?;
    tx.commit().await?;
    Ok(Json(Paginated::new(rows, page)))
}

pub async fn get_component(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Component>> {
    caller.require_read()?;
    let mut tx = state.begin(&caller).await?;
    let row = tx.get_component(id).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn create_component(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiJson(body): ApiJson<CreateComponentBody>,
) -> ApiResult<Json<Component>> {
    caller.require_write()?;
    let team = new_team(&state, &caller, body.team_id).await?;
    let new = NewComponent {
        page_id: body.page_id,
        name: body.name,
        component_type: body.component_type,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.create_component(&new).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn update_component(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateComponentBody>,
) -> ApiResult<Json<Component>> {
    caller.require_write()?;
    let team = team_change(&state, &caller, body.team_id).await?;
    let update = ComponentUpdate {
        name: body.name,
        component_type: body.component_type,
        description: body.description,
        team,
        metadata: body.metadata,
    };
    let mut tx = state.begin(&caller).await?;
    let row = tx.update_component(id, &update).await?;
    tx.commit().await?;
    Ok(Json(row))
}

pub async fn delete_component(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    caller.require_write()?;
    let mut tx = state.begin(&caller).await?;
    tx.delete_component(id).await?;
    tx.commit().await?;
    Ok(deleted())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update body's team field: `None` absent (keep), `Some(None)`
    /// explicit null (clear), `Some(Some(id))` set.
    fn team_of(json: &str) -> Result<Option<Option<Uuid>>, serde_json::Error> {
        serde_json::from_str::<UpdateExperienceBody>(json).map(|b| b.team_id)
    }

    #[test]
    fn the_team_field_tells_absent_from_null_and_accepts_both_spellings() {
        let id = Uuid::new_v4();
        assert_eq!(team_of("{}").unwrap(), None);
        assert_eq!(team_of(r#"{"team_id": null}"#).unwrap(), Some(None));
        assert_eq!(team_of(r#"{"teamId": null}"#).unwrap(), Some(None));
        assert_eq!(
            team_of(&format!(r#"{{"team_id": "{id}"}}"#)).unwrap(),
            Some(Some(id))
        );
        assert_eq!(
            team_of(&format!(r#"{{"teamId": "{id}"}}"#)).unwrap(),
            Some(Some(id))
        );
        assert!(team_of(&format!(r#"{{"teamId": "{id}", "team_id": null}}"#)).is_err());
        assert!(team_of(r#"{"team_id": "not-a-uuid"}"#).is_err());

        let create: CreatePageBody =
            serde_json::from_str(&format!(r#"{{"name": "p", "teamId": "{id}"}}"#)).unwrap();
        assert_eq!(create.team_id, Some(id));
        let create: CreatePageBody =
            serde_json::from_str(r#"{"name": "p", "team_id": null}"#).unwrap();
        assert_eq!(create.team_id, None);
    }
}
