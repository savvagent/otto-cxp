//! Customers (story S5): `CustomersExt` on `otto_tenant::Tx` — create, get,
//! update, delete, list, search, resolve by id, email, or name, and the
//! customer-360 view — with its model structs.

use std::future::Future;

use chrono::{DateTime, Utc};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{on_unique, Error, Result};
use crate::interactions::{Interaction, INTERACTION_COLS};
use crate::paging::Page;

/// Column list for [`Customer`]; never includes `org_id`.
pub(crate) const CUSTOMER_COLS: &str = "id, email, name, phone, company, metadata, tags, \
     experience_id, application_id, page_id, component_id, created_at, updated_at";

/// How many recent rows each section of the 360 view carries.
const RECENT: i64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Customer {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub metadata: serde_json::Value,
    pub tags: Vec<String>,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A support ticket as the 360 view shows it.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub id: Uuid,
    pub subject: String,
    pub status: String,
    pub priority: String,
    pub channel: Option<String>,
    pub assigned_to: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// A piece of feedback as the 360 view shows it.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSummary {
    pub id: Uuid,
    pub feedback_type: String,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub rating: Option<i32>,
    pub created_at: DateTime<Utc>,
}

/// A customer with their ten most recent tickets, interactions, and feedback.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Customer360 {
    pub customer: Customer,
    pub recent_tickets: Vec<TicketSummary>,
    pub recent_interactions: Vec<Interaction>,
    pub recent_feedback: Vec<FeedbackSummary>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewCustomer {
    pub email: String,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    /// Defaults to `{}`.
    pub metadata: Option<serde_json::Value>,
    pub tags: Vec<String>,
}

/// Fields to change; `None` leaves the field as it is.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CustomerUpdate {
    pub email: Option<String>,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub company: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,
}

/// Catalog filters for [`CustomersExt::list_customers`]; all `None` lists every
/// customer.
#[derive(Debug, Clone, Copy, Default)]
pub struct CustomerFilter {
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

const NOT_FOUND: Error = Error::NotFound { kind: "customer" };

fn duplicate_email() -> Error {
    Error::Conflict(
        "a customer with that email already exists in this organization; \
         look them up by email instead of creating them again"
            .into(),
    )
}

fn non_blank_email(email: &str) -> Result<&str> {
    let email = email.trim();
    if email.is_empty() {
        return Err(Error::Invalid("a customer needs an email address".into()));
    }
    Ok(email)
}

/// A `LIKE` pattern matching `q` anywhere, with its own wildcards escaped.
fn contains_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

pub trait CustomersExt {
    fn create_customer(
        &mut self,
        new: &NewCustomer,
    ) -> impl Future<Output = Result<Customer>> + Send;

    /// `Error::NotFound` when this organization has no such customer.
    fn get_customer(&mut self, id: Uuid) -> impl Future<Output = Result<Customer>> + Send;

    fn update_customer(
        &mut self,
        id: Uuid,
        update: &CustomerUpdate,
    ) -> impl Future<Output = Result<Customer>> + Send;

    /// Deletes the customer and, by cascade, their interactions, tickets, chat
    /// sessions, feedback, and segment memberships.
    fn delete_customer(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;

    /// Newest first, with the total matching the filter.
    fn list_customers(
        &mut self,
        filter: &CustomerFilter,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Customer>, i64)>> + Send;

    /// Customers whose email or name contains `query` (case-insensitive), or
    /// whose name is similar to it; email matches first.
    fn search_customers(
        &mut self,
        query: &str,
        page: Page,
    ) -> impl Future<Output = Result<Vec<Customer>>> + Send;

    /// Resolve a uuid, an exact email, or a name to a customer id. A uuid must
    /// name a customer of this organization. A name matching several customers
    /// is `Error::Invalid` (the message lists none of them; search to choose).
    fn resolve_customer(&mut self, identifier: &str) -> impl Future<Output = Result<Uuid>> + Send;

    fn customer_360(&mut self, id: Uuid) -> impl Future<Output = Result<Customer360>> + Send;
}

impl CustomersExt for Tx<'_> {
    async fn create_customer(&mut self, new: &NewCustomer) -> Result<Customer> {
        let email = non_blank_email(&new.email)?;
        let org = self.org();
        let metadata = new
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        sqlx::query_as(&format!(
            "INSERT INTO customers (org_id, email, name, phone, company, metadata, tags) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {CUSTOMER_COLS}"
        ))
        .bind(org)
        .bind(email)
        .bind(&new.name)
        .bind(&new.phone)
        .bind(&new.company)
        .bind(&metadata)
        .bind(&new.tags)
        .fetch_one(self.conn())
        .await
        .map_err(|e| on_unique(e, duplicate_email))
    }

    async fn get_customer(&mut self, id: Uuid) -> Result<Customer> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {CUSTOMER_COLS} FROM customers WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(NOT_FOUND)
    }

    async fn update_customer(&mut self, id: Uuid, update: &CustomerUpdate) -> Result<Customer> {
        let email = update.email.as_deref().map(non_blank_email).transpose()?;
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE customers SET \
                 email = COALESCE($3, email), \
                 name = COALESCE($4, name), \
                 phone = COALESCE($5, phone), \
                 company = COALESCE($6, company), \
                 metadata = COALESCE($7, metadata), \
                 tags = COALESCE($8, tags), \
                 updated_at = now() \
             WHERE org_id = $1 AND id = $2 RETURNING {CUSTOMER_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(email)
        .bind(&update.name)
        .bind(&update.phone)
        .bind(&update.company)
        .bind(&update.metadata)
        .bind(&update.tags)
        .fetch_optional(self.conn())
        .await
        .map_err(|e| on_unique(e, duplicate_email))?
        .ok_or(NOT_FOUND)
    }

    async fn delete_customer(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM customers WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(NOT_FOUND);
        }
        Ok(())
    }

    async fn list_customers(
        &mut self,
        filter: &CustomerFilter,
        page: Page,
    ) -> Result<(Vec<Customer>, i64)> {
        let org = self.org();
        const WHERE: &str = "org_id = $1 \
             AND ($2::uuid IS NULL OR experience_id = $2) \
             AND ($3::uuid IS NULL OR application_id = $3) \
             AND ($4::uuid IS NULL OR page_id = $4) \
             AND ($5::uuid IS NULL OR component_id = $5)";
        let rows = sqlx::query_as(&format!(
            "SELECT {CUSTOMER_COLS} FROM customers WHERE {WHERE} \
             ORDER BY created_at DESC, id LIMIT $6 OFFSET $7"
        ))
        .bind(org)
        .bind(filter.experience_id)
        .bind(filter.application_id)
        .bind(filter.page_id)
        .bind(filter.component_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) =
            sqlx::query_as(&format!("SELECT count(*) FROM customers WHERE {WHERE}"))
                .bind(org)
                .bind(filter.experience_id)
                .bind(filter.application_id)
                .bind(filter.page_id)
                .bind(filter.component_id)
                .fetch_one(self.conn())
                .await?;
        Ok((rows, total))
    }

    async fn search_customers(&mut self, query: &str, page: Page) -> Result<Vec<Customer>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::Invalid(
                "search for a name or an email; the query was empty".into(),
            ));
        }
        let org = self.org();
        Ok(sqlx::query_as(&format!(
            "SELECT {CUSTOMER_COLS} FROM customers \
             WHERE org_id = $1 \
               AND (email ILIKE $2 OR name ILIKE $2 OR similarity(name, $3) > 0.3) \
             ORDER BY CASE WHEN email ILIKE $2 THEN 0 ELSE 1 END, \
                      similarity(name, $3) DESC NULLS LAST, created_at DESC, id \
             LIMIT $4 OFFSET $5"
        ))
        .bind(org)
        .bind(contains_pattern(query))
        .bind(query)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?)
    }

    async fn resolve_customer(&mut self, identifier: &str) -> Result<Uuid> {
        let identifier = identifier.trim();
        if let Ok(id) = Uuid::parse_str(identifier) {
            return Ok(self.get_customer(id).await?.id);
        }
        let org = self.org();
        if identifier.contains('@') {
            let found: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM customers WHERE org_id = $1 AND email = $2")
                    .bind(org)
                    .bind(identifier)
                    .fetch_optional(self.conn())
                    .await?;
            return found.map(|(id,)| id).ok_or(NOT_FOUND);
        }
        // One exact (case-insensitive) name beats looser matches.
        let exact: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM customers WHERE org_id = $1 AND lower(name) = lower($2) LIMIT 2",
        )
        .bind(org)
        .bind(identifier)
        .fetch_all(self.conn())
        .await?;
        if let [(id,)] = exact[..] {
            return Ok(id);
        }
        let found = self
            .search_customers(identifier, Page::new(Some(2), None))
            .await?;
        match found.as_slice() {
            [] => Err(NOT_FOUND),
            [one] => Ok(one.id),
            _ => Err(Error::Invalid(
                "more than one customer matches that name; pass the customer's email address \
                 or id instead (search_customers lists the candidates)"
                    .into(),
            )),
        }
    }

    async fn customer_360(&mut self, id: Uuid) -> Result<Customer360> {
        let customer = self.get_customer(id).await?;
        let org = self.org();
        let recent_tickets = sqlx::query_as(
            "SELECT id, subject, status::text AS status, priority::text AS priority, \
                    channel::text AS channel, assigned_to, created_at, resolved_at \
             FROM tickets WHERE org_id = $1 AND customer_id = $2 \
             ORDER BY created_at DESC, id LIMIT $3",
        )
        .bind(org)
        .bind(id)
        .bind(RECENT)
        .fetch_all(self.conn())
        .await?;
        let recent_interactions = sqlx::query_as(&format!(
            "SELECT {INTERACTION_COLS} FROM interactions WHERE org_id = $1 AND customer_id = $2 \
             ORDER BY created_at DESC, id LIMIT $3"
        ))
        .bind(org)
        .bind(id)
        .bind(RECENT)
        .fetch_all(self.conn())
        .await?;
        let recent_feedback = sqlx::query_as(
            "SELECT id, feedback_type::text AS feedback_type, subject, body, rating, created_at \
             FROM feedback WHERE org_id = $1 AND customer_id = $2 \
             ORDER BY created_at DESC, id LIMIT $3",
        )
        .bind(org)
        .bind(id)
        .bind(RECENT)
        .fetch_all(self.conn())
        .await?;
        Ok(Customer360 {
            customer,
            recent_tickets,
            recent_interactions,
            recent_feedback,
        })
    }
}
