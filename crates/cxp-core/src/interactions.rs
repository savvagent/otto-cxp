//! Customer interactions and journeys: `InteractionsExt` on
//! `savvagent_tenant::Tx`, with its model structs.

use std::future::Future;

use chrono::{DateTime, Utc};
use savvagent_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::paging::Page;

/// Column list for [`Interaction`]; never includes `org_id`.
pub(crate) const INTERACTION_COLS: &str =
    "id, customer_id, interaction_type::text AS interaction_type, metadata, created_at";

/// The values of the `interaction_type` enum.
pub const INTERACTION_TYPES: [&str; 7] = [
    "page_view",
    "purchase",
    "support_contact",
    "email_open",
    "email_click",
    "feature_use",
    "login",
];

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub interaction_type: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewInteraction {
    pub customer_id: Uuid,
    /// One of [`INTERACTION_TYPES`].
    pub interaction_type: String,
    /// Defaults to `{}`.
    pub metadata: Option<serde_json::Value>,
}

/// Parse a JSON text a caller supplied (an MCP tool's `metadata` argument)
/// into a value, or say what was wrong with it.
/// Lives here because lib.rs is not this story's to extend; MCP tools use it for
/// their JSON-text arguments (`metadata`, segment `criteria`).
pub fn parse_json(what: &str, text: &str) -> Result<serde_json::Value> {
    serde_json::from_str(text).map_err(|e| Error::Invalid(format!("{what} is not valid JSON: {e}")))
}

pub trait InteractionsExt {
    /// `Error::NotFound` for a customer this organization does not have;
    /// `Error::Invalid` for an unknown interaction type.
    fn record_interaction(
        &mut self,
        new: &NewInteraction,
    ) -> impl Future<Output = Result<Interaction>> + Send;

    /// Every customer's interactions, newest first, with the total.
    fn list_interactions(
        &mut self,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Interaction>, i64)>> + Send;

    /// One customer's interactions, newest first, with the total.
    /// `Error::NotFound` for a customer this organization does not have.
    fn get_journey(
        &mut self,
        customer_id: Uuid,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Interaction>, i64)>> + Send;
}

impl InteractionsExt for Tx<'_> {
    async fn record_interaction(&mut self, new: &NewInteraction) -> Result<Interaction> {
        if !INTERACTION_TYPES.contains(&new.interaction_type.as_str()) {
            return Err(Error::Invalid(format!(
                "unknown interaction type; use one of: {}",
                INTERACTION_TYPES.join(", ")
            )));
        }
        let org = self.org();
        let metadata = new
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        // Selecting from `customers` (rather than inserting blind) turns another
        // org's id into a clean not-found; the composite foreign key backs it.
        sqlx::query_as(&format!(
            "INSERT INTO interactions (org_id, customer_id, interaction_type, metadata) \
             SELECT org_id, id, $3::interaction_type, $4 FROM customers \
             WHERE org_id = $1 AND id = $2 \
             RETURNING {INTERACTION_COLS}"
        ))
        .bind(org)
        .bind(new.customer_id)
        .bind(&new.interaction_type)
        .bind(&metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "customer" })
    }

    async fn list_interactions(&mut self, page: Page) -> Result<(Vec<Interaction>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {INTERACTION_COLS} FROM interactions WHERE org_id = $1 \
             ORDER BY created_at DESC, id LIMIT $2 OFFSET $3"
        ))
        .bind(org)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM interactions WHERE org_id = $1")
                .bind(org)
                .fetch_one(self.conn())
                .await?;
        Ok((rows, total))
    }

    async fn get_journey(
        &mut self,
        customer_id: Uuid,
        page: Page,
    ) -> Result<(Vec<Interaction>, i64)> {
        let org = self.org();
        let exists: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM customers WHERE org_id = $1 AND id = $2")
                .bind(org)
                .bind(customer_id)
                .fetch_optional(self.conn())
                .await?;
        if exists.is_none() {
            return Err(Error::NotFound { kind: "customer" });
        }
        let rows = sqlx::query_as(&format!(
            "SELECT {INTERACTION_COLS} FROM interactions \
             WHERE org_id = $1 AND customer_id = $2 \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(customer_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM interactions WHERE org_id = $1 AND customer_id = $2",
        )
        .bind(org)
        .bind(customer_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }
}
