//! What tools return. Single-object tools return `cxp-core`'s own domain types
//! directly; list tools return [`List`]; the rest are defined here. MCP
//! requires an object root for structured output, so nothing returns a bare
//! array.

use cxp_core::feedback::{CsatCounts, NpsCounts};
use cxp_core::usage::Status;
use savvagent_tenant::ids::{OrgId, UserId};
use schemars::JsonSchema;
use serde::Serialize;

/// Every list tool's envelope. `total` counts every match, not just this
/// page. The two search tools count only up to
/// [`cxp_core::search::TOTAL_CAP`] and add `totalCapped`
/// ([`cxp_core::search::SearchPage`] states the contract): false when
/// `total` and the items are exact, true when `total` is a lower bound
/// (more matches than the cap, or the search stopped before counting every
/// match); the other list tools omit it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct List<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(rename = "totalCapped", skip_serializing_if = "Option::is_none")]
    pub total_capped: Option<bool>,
}

impl<T> List<T> {
    pub fn counted((items, total): (Vec<T>, i64)) -> Self {
        Self {
            items,
            total: Some(total),
            total_capped: None,
        }
    }

    pub fn searched(found: cxp_core::search::SearchPage<T>) -> Self {
        Self {
            items: found.items,
            total: Some(found.total),
            total_capped: Some(found.total_capped),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpsOut {
    /// Net Promoter Score from -100 to 100; null when no response is scored.
    pub nps: Option<f64>,
    #[serde(flatten)]
    pub counts: NpsCounts,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CsatOut {
    /// Percent of ratings that are 4 or 5; null when nothing is rated.
    pub csat: Option<f64>,
    #[serde(flatten)]
    pub counts: CsatCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

impl From<savvagent_resource::Role> for Role {
    fn from(r: savvagent_resource::Role) -> Self {
        match r {
            savvagent_resource::Role::Owner => Role::Owner,
            savvagent_resource::Role::Admin => Role::Admin,
            savvagent_resource::Role::Member => Role::Member,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhoAmI {
    /// This service's name, as the consent screen and its metadata show it
    /// (`Savvagent CXP`).
    pub service: &'static str,
    pub user: UserOut,
    pub org: OrgOut,
    /// The caller's role in this org, as the platform reports it now.
    pub role: Option<Role>,
    pub token: TokenOut,
    /// Null when the platform could not report usage just now.
    pub usage: Option<Status>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserOut {
    pub id: UserId,
    pub email: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgOut {
    pub id: OrgId,
    pub slug: Option<String>,
    pub name: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenOut {
    /// "oauth" for a browser-authorized token, "pat" for a personal access token.
    pub kind: &'static str,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    /// Whether this token may read (`cxp:read` or `cxp:write`).
    pub can_read: bool,
    /// Whether this token may change anything (`cxp:write`).
    pub can_write: bool,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageOut {
    pub usage: Status,
}
