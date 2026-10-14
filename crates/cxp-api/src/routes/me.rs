//! `/api/v1/me/teams`: the caller's teams at the platform, for the console's
//! team selector. Teams belong to the platform; this database has no teams
//! table.

use axum::extract::State;
use axum::{Extension, Json};
use cxp_core::auth::Principal;
use serde::Serialize;
use uuid::Uuid;

use crate::{ApiResult, AppState};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Team {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// The caller's teams in the token's org, ordered by name.
///
/// The platform answering "not a member" (`None`) is `[]`, not an error: the
/// token was just introspected as live, so this is the moment between a
/// removal and the token going inactive, and an empty selector is the honest
/// answer. A platform failure is `503`. No transaction is opened: nothing
/// here touches this database, and authentication has already checked the
/// revocation tombstones.
pub async fn teams(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
) -> ApiResult<Json<Vec<Team>>> {
    caller.require_read()?;
    let teams = state
        .platform
        .member_teams(caller.org_id.as_uuid(), caller.user_id.as_uuid())
        .await?
        .unwrap_or_default();
    Ok(Json(
        teams
            .into_iter()
            .map(|t| Team {
                id: t.id,
                slug: t.slug,
                name: t.name,
            })
            .collect(),
    ))
}
