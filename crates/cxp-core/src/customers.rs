//! Customers: `CustomersExt` on `otto_tenant::Tx` — create, get,
//! update, delete, list, search, resolve by id, email, or name, and the
//! customer-360 view — with its model structs.

use std::future::Future;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contains_pattern;
use crate::error::{on_unique, Error, Result};
use crate::interactions::{Interaction, INTERACTION_COLS};
use crate::paging::Page;

/// Column list for [`Customer`]; never includes `org_id`.
pub(crate) const CUSTOMER_COLS: &str = "id, email, name, phone, company, metadata, tags, \
     experience_id, application_id, page_id, component_id, created_at, updated_at";

/// `search_customers`: `$1` org, `$2` the `ILIKE` pattern, `$3` the trimmed
/// query, `$4`/`$5` limit and offset.
///
/// The final `WHERE` is the whole match predicate, exactly as before #17; the
/// CTEs only choose which rows it is evaluated on, through
/// `customer_search_grams` (migration 0004), whose lookups are leakproof
/// btree conditions and so index conditions under RLS. Each candidate set is
/// a superset of the rows its half of the predicate accepts:
///
/// - `substring_hits`: `x ILIKE '%q%'` means lower(x) contains lower(q), so
///   a match has every 4-character window of lower(q) among its `s` grams.
///   Each window's posting count is taken up to [`DF_PROBE`] rows, and the
///   rarest window's customers that also hold the next two rarest are the
///   candidates; a query under four characters instead takes every customer
///   with a window starting with it.
/// - `fuzzy_hits`: `name % q` means similarity >= t (pg_trgm's
///   `similarity_threshold`): with |A| the name's trigrams, |B| the query's,
///   and c those they share, c / (|A| + |B| - c) >= t, so
///   c >= k(|A|) = t(|A| + |B|) / (1 + t), and, as c <= |A|, c >= t|B|
///   (`kmin`). A match therefore shares at least one of any |B| - kmin + 1
///   query trigrams; only the rarest that many are looked up (rarity from
///   the capped counts, ties broken by shape: word-initial trigrams are the
///   most common), and a customer is a candidate when its count p among them
///   reaches k(|A|) - (kmin - 1), the most the trigrams left out could add.
///   The slack of 0.001 covers pg_trgm's float4 rounding. A threshold of 0
///   or below makes every named customer a match, so it falls back to the
///   org's customers.
pub(crate) static SEARCH_CUSTOMERS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH w AS ( \
           SELECT DISTINCT 's' || g AS gram FROM search_windows(lower($3)) AS g \
         ), ranked AS ( \
           SELECT gram, row_number() OVER (ORDER BY n, gram) AS rk FROM ( \
             SELECT w.gram, (SELECT count(*) FROM (SELECT 1 FROM customer_search_grams g \
                              WHERE g.org_id = $1 AND g.gram = w.gram LIMIT {DF_PROBE}) AS p) AS n \
             FROM w) AS c \
         ), substring_hits AS ( \
           SELECT g.customer_id FROM customer_search_grams g \
           WHERE g.org_id = $1 AND g.gram = (SELECT gram FROM ranked WHERE rk = 1) \
             AND NOT EXISTS (SELECT 1 FROM ranked r WHERE r.rk IN (2, 3) AND NOT EXISTS ( \
                   SELECT 1 FROM customer_search_grams g2 \
                   WHERE g2.org_id = $1 AND g2.gram = r.gram AND g2.customer_id = g.customer_id)) \
           UNION \
           SELECT g.customer_id FROM customer_search_grams g \
           WHERE length(lower($3)) < 4 AND g.org_id = $1 \
             AND g.gram >= 's' || lower($3) \
             AND g.gram <= 's' || lower($3) || U&'\\+10FFFF\\+10FFFF\\+10FFFF' \
         ), fq AS ( \
           SELECT 't' || t AS gram, \
                  (SELECT count(*) FROM (SELECT 1 FROM customer_search_grams g \
                     WHERE g.org_id = $1 AND g.gram = 't' || t LIMIT {DF_PROBE}) AS p) AS n, \
                  CASE WHEN t LIKE '  %' THEN 2 WHEN t LIKE ' %' THEN 1 ELSE 0 END AS shape \
           FROM unnest(show_trgm($3)) AS t \
         ), fz AS ( \
           SELECT gram, row_number() OVER (ORDER BY n, shape, gram) AS rk FROM fq \
         ), fp AS ( \
           SELECT b, th, greatest(ceil(th * b - 0.001), 1) AS kmin FROM ( \
             SELECT (SELECT count(*) FROM fq) AS b, \
                    current_setting('pg_trgm.similarity_threshold')::float8 AS th) AS x \
         ), fuzzy_hits AS ( \
           SELECT g.customer_id FROM customer_search_grams g \
           WHERE g.org_id = $1 \
             AND g.gram = ANY (ARRAY(SELECT gram FROM fz, fp WHERE fz.rk <= fp.b - fp.kmin + 1)) \
           GROUP BY g.customer_id \
           HAVING count(*) >= ceil((SELECT th FROM fp) * (max(g.name_grams) + (SELECT b FROM fp)) \
                                   / (1 + (SELECT th FROM fp)) - 0.001) \
                              - ((SELECT kmin FROM fp) - 1) \
           UNION ALL \
           SELECT id FROM customers \
           WHERE org_id = $1 AND current_setting('pg_trgm.similarity_threshold')::float8 <= 0 \
         ) \
         SELECT {CUSTOMER_COLS} FROM customers \
         WHERE org_id = $1 \
           AND id = ANY (ARRAY(SELECT customer_id FROM substring_hits \
                               UNION SELECT customer_id FROM fuzzy_hits)) \
           AND (email ILIKE $2 OR name ILIKE $2 OR name % $3) \
         ORDER BY CASE WHEN email ILIKE $2 THEN 0 ELSE 1 END, \
                  similarity(name, $3) DESC NULLS LAST, created_at DESC, id \
         LIMIT $4 OFFSET $5"
    )
});

/// How many postings of one gram search counts before calling it common:
/// enough to tell a rare gram from a common one, cheap for every window of a
/// long query.
pub(crate) const DF_PROBE: i64 = 256;

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
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    /// Defaults to `{}`.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
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
    /// whose name is similar to it (pg_trgm `%`); email matches first, then
    /// by name similarity, newest first. Neither `ILIKE` nor `%` is
    /// leakproof, so under row-level security no index can serve them;
    /// candidates come from `customer_search_grams` by leakproof btree
    /// lookups instead, and the original predicate is applied to those
    /// (`SEARCH_CUSTOMERS_SQL`).
    fn search_customers(
        &mut self,
        query: &str,
        page: Page,
    ) -> impl Future<Output = Result<Vec<Customer>>> + Send;

    /// Resolve a uuid, an exact email, or a name to a customer id. A uuid must
    /// name a customer of this organization; anything containing `@` must be
    /// an exact email. Anything else is matched by exact (case-insensitive)
    /// name, and failing that by a case-insensitive substring of the name or
    /// the email, which must match exactly one customer. Never a fuzzy
    /// match. Several matches at either step are `Error::Invalid` (the
    /// message lists none of them; search to choose).
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
        crate::without_jit(self).await?;
        Ok(sqlx::query_as(&SEARCH_CUSTOMERS_SQL)
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
        if identifier.is_empty() {
            return Err(Error::Invalid(
                "name the customer by id, email address, or name; the identifier was empty".into(),
            ));
        }
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
        let ambiguous = || {
            Error::Invalid(
                "more than one customer matches that name; pass the customer's email address \
                 or id instead (search_customers lists the candidates)"
                    .into(),
            )
        };
        // An exact (case-insensitive) name, through the stored `name_lower`
        // and its (org_id, name_lower) index, else a single substring match of
        // the name or email. Never a fuzzy match: write tools resolve through
        // here, so a near miss must not land on the wrong customer.
        let exact: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM customers WHERE org_id = $1 AND name_lower = lower($2) LIMIT 2",
        )
        .bind(org)
        .bind(identifier)
        .fetch_all(self.conn())
        .await?;
        match exact[..] {
            [(id,)] => return Ok(id),
            [_, _] => return Err(ambiguous()),
            _ => {}
        }
        let found: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM customers WHERE org_id = $1 AND (name ILIKE $2 OR email ILIKE $2) \
             LIMIT 2",
        )
        .bind(org)
        .bind(contains_pattern(identifier))
        .fetch_all(self.conn())
        .await?;
        match found[..] {
            [] => Err(NOT_FOUND),
            [(id,)] => Ok(id),
            _ => Err(ambiguous()),
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
