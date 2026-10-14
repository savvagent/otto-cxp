//! Support tickets: `TicketsExt` on `otto_tenant::Tx`, with its
//! model structs.

use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{map_fk, Error, Result};
use crate::paging::Page;

/// Valid `ticket_status` values.
pub const STATUSES: [&str; 5] = ["open", "in_progress", "waiting", "resolved", "closed"];
/// Valid `ticket_priority` values.
pub const PRIORITIES: [&str; 4] = ["low", "medium", "high", "urgent"];
/// Valid `ticket_channel` values.
pub const CHANNELS: [&str; 5] = ["email", "chat", "phone", "web", "social"];

const TICKET_COLS: &str = "id, customer_id, subject, description, \
    status::text AS status, priority::text AS priority, channel::text AS channel, \
    assigned_to, tags, metadata, experience_id, application_id, page_id, component_id, \
    resolved_at, created_at, updated_at";

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub subject: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub channel: Option<String>,
    pub assigned_to: Option<String>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A new ticket. `customer_id` is already resolved. `priority` defaults to
/// `medium`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewTicket {
    pub customer_id: Uuid,
    pub subject: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub channel: Option<String>,
}

/// Fields to change on a ticket; `None` leaves a field as it is, so a field
/// cannot be cleared once set.
///
/// `resolved_at` follows `status`: moving to `resolved` or `closed` stamps it
/// only if unset (the first resolve time is kept), and moving to `open`,
/// `in_progress` or `waiting` clears it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketUpdate {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assigned_to: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Filters for [`TicketsExt::list_tickets`]; `None` means no filter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketFilter {
    pub customer_id: Option<Uuid>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// The row kind a foreign-key violation names, by the exact generated
/// constraint names of `tickets` (see [`map_fk`]).
const FK_KINDS: &[(&str, &str)] = &[
    ("tickets_org_id_customer_id_fkey", "customer"),
    ("tickets_org_id_experience_id_fkey", "experience"),
    ("tickets_org_id_application_id_fkey", "application"),
    ("tickets_org_id_page_id_fkey", "page"),
    ("tickets_org_id_component_id_fkey", "component"),
];

fn check_enum(what: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{what} must be one of {}",
            allowed.join(", ")
        )))
    }
}

fn check_opt(what: &str, value: Option<&str>, allowed: &[&str]) -> Result<()> {
    value.map_or(Ok(()), |v| check_enum(what, v, allowed))
}

pub trait TicketsExt {
    /// Open a ticket for a customer of this org. A customer that does not
    /// exist here (including another org's) is `NotFound { kind: "customer" }`.
    fn create_ticket(
        &mut self,
        new: &NewTicket,
    ) -> impl std::future::Future<Output = Result<Ticket>> + Send;

    fn get_ticket(&mut self, id: Uuid) -> impl std::future::Future<Output = Result<Ticket>> + Send;

    fn update_ticket(
        &mut self,
        id: Uuid,
        update: &TicketUpdate,
    ) -> impl std::future::Future<Output = Result<Ticket>> + Send;

    /// Newest first, with the total matching the filter.
    fn list_tickets(
        &mut self,
        filter: &TicketFilter,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<Ticket>, i64)>> + Send;

    /// Set status `closed`; `resolved_at` is stamped only if unset.
    fn close_ticket(
        &mut self,
        id: Uuid,
    ) -> impl std::future::Future<Output = Result<Ticket>> + Send;

    fn delete_ticket(&mut self, id: Uuid) -> impl std::future::Future<Output = Result<()>> + Send;
}

const FILTER_SQL: &str = "org_id = $1 \
    AND ($2::uuid IS NULL OR customer_id = $2) \
    AND ($3::text IS NULL OR status = $3::ticket_status) \
    AND ($4::text IS NULL OR priority = $4::ticket_priority) \
    AND ($5::uuid IS NULL OR experience_id = $5) \
    AND ($6::uuid IS NULL OR application_id = $6) \
    AND ($7::uuid IS NULL OR page_id = $7) \
    AND ($8::uuid IS NULL OR component_id = $8)";

impl TicketsExt for Tx<'_> {
    async fn create_ticket(&mut self, new: &NewTicket) -> Result<Ticket> {
        let priority = new.priority.as_deref().unwrap_or("medium");
        check_enum("priority", priority, &PRIORITIES)?;
        check_opt("channel", new.channel.as_deref(), &CHANNELS)?;
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO tickets (org_id, customer_id, subject, description, priority, channel, \
                                  created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5::ticket_priority, $6::ticket_channel, \
                     clock_timestamp(), clock_timestamp()) \
             RETURNING {TICKET_COLS}"
        ))
        .bind(org)
        .bind(new.customer_id)
        .bind(&new.subject)
        .bind(&new.description)
        .bind(priority)
        .bind(&new.channel)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn get_ticket(&mut self, id: Uuid) -> Result<Ticket> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {TICKET_COLS} FROM tickets WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "ticket" })
    }

    async fn update_ticket(&mut self, id: Uuid, update: &TicketUpdate) -> Result<Ticket> {
        check_opt("status", update.status.as_deref(), &STATUSES)?;
        check_opt("priority", update.priority.as_deref(), &PRIORITIES)?;
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE tickets SET \
               status = COALESCE($3::ticket_status, status), \
               priority = COALESCE($4::ticket_priority, priority), \
               assigned_to = COALESCE($5, assigned_to), \
               subject = COALESCE($6, subject), \
               description = COALESCE($7, description), \
               tags = COALESCE($8, tags), \
               resolved_at = CASE \
                 WHEN $3::text IN ('resolved', 'closed') \
                   THEN COALESCE(resolved_at, clock_timestamp()) \
                 WHEN $3::text IS NOT NULL THEN NULL \
                 ELSE resolved_at END, \
               updated_at = clock_timestamp() \
             WHERE org_id = $1 AND id = $2 \
             RETURNING {TICKET_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.status)
        .bind(&update.priority)
        .bind(&update.assigned_to)
        .bind(&update.subject)
        .bind(&update.description)
        .bind(&update.tags)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "ticket" })
    }

    async fn list_tickets(
        &mut self,
        filter: &TicketFilter,
        page: Page,
    ) -> Result<(Vec<Ticket>, i64)> {
        check_opt("status", filter.status.as_deref(), &STATUSES)?;
        check_opt("priority", filter.priority.as_deref(), &PRIORITIES)?;
        let org = self.org();
        let total: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM tickets WHERE {FILTER_SQL}"))
                .bind(org)
                .bind(filter.customer_id)
                .bind(&filter.status)
                .bind(&filter.priority)
                .bind(filter.experience_id)
                .bind(filter.application_id)
                .bind(filter.page_id)
                .bind(filter.component_id)
                .fetch_one(self.conn())
                .await?;
        let rows = sqlx::query_as(&format!(
            "SELECT {TICKET_COLS} FROM tickets WHERE {FILTER_SQL} \
             ORDER BY created_at DESC, id LIMIT $9 OFFSET $10"
        ))
        .bind(org)
        .bind(filter.customer_id)
        .bind(&filter.status)
        .bind(&filter.priority)
        .bind(filter.experience_id)
        .bind(filter.application_id)
        .bind(filter.page_id)
        .bind(filter.component_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn close_ticket(&mut self, id: Uuid) -> Result<Ticket> {
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE tickets SET status = 'closed', resolved_at = COALESCE(resolved_at, clock_timestamp()), updated_at = clock_timestamp() \
             WHERE org_id = $1 AND id = $2 RETURNING {TICKET_COLS}"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "ticket" })
    }

    async fn delete_ticket(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM tickets WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "ticket" });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_constraint_names_its_kind() {
        crate::error::fake::assert_kinds(FK_KINDS);
    }
}
