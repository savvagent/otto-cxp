//! Customer segments and their members (story S5): `SegmentsExt` on
//! `otto_tenant::Tx`, with its model structs.
//!
//! A `static` segment's members are the rows in `segment_members`. A `dynamic`
//! segment's members are the customers its `criteria` match, evaluated when
//! asked. Criteria keys (all optional, all ANDed): `tags_contain` (a tag),
//! `company_eq` (exact company), `created_after` / `created_before` (RFC 3339
//! timestamps). Values are bound, never spliced into SQL.

use std::future::Future;

use chrono::{DateTime, Utc};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::customers::{Customer, CUSTOMER_COLS};
use crate::error::{Error, Result};
use crate::paging::Page;

const SEGMENT_COLS: &str = "id, name, description, criteria, segment_type, created_at, updated_at";

const CRITERIA_KEYS: [&str; 4] = [
    "tags_contain",
    "company_eq",
    "created_after",
    "created_before",
];

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub criteria: serde_json::Value,
    pub segment_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewSegment {
    pub name: String,
    pub description: Option<String>,
    /// A JSON object. For a dynamic segment its keys must be supported criteria.
    pub criteria: serde_json::Value,
    /// `"dynamic"` (the default) or `"static"`.
    pub segment_type: Option<String>,
}

/// A dynamic segment's criteria, validated.
#[derive(Debug, Default)]
struct Criteria {
    tag: Option<String>,
    company: Option<String>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

fn criteria_text<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<&'a str>> {
    match map.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(invalid(format!("criteria `{key}` must be a string"))),
    }
}

fn criteria_time(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<DateTime<Utc>>> {
    criteria_text(map, key)?
        .map(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|_| {
                    invalid(format!(
                        "criteria `{key}` must be an RFC 3339 timestamp, like 2026-01-31T00:00:00Z"
                    ))
                })
        })
        .transpose()
}

impl Criteria {
    fn parse(criteria: &serde_json::Value) -> Result<Self> {
        let map = criteria
            .as_object()
            .ok_or_else(|| invalid("criteria must be a JSON object"))?;
        if let Some(bad) = map.keys().find(|k| !CRITERIA_KEYS.contains(&k.as_str())) {
            return Err(invalid(format!(
                "unsupported criteria key `{bad}`; supported keys: {}",
                CRITERIA_KEYS.join(", ")
            )));
        }
        Ok(Self {
            tag: criteria_text(map, "tags_contain")?.map(str::to_owned),
            company: criteria_text(map, "company_eq")?.map(str::to_owned),
            after: criteria_time(map, "created_after")?,
            before: criteria_time(map, "created_before")?,
        })
    }
}

pub trait SegmentsExt {
    /// `Error::Invalid` for a blank name, an unknown segment type, or criteria
    /// that are not a JSON object (a dynamic segment's must also use
    /// supported keys and value types).
    fn create_segment(&mut self, new: &NewSegment) -> impl Future<Output = Result<Segment>> + Send;

    fn get_segment(&mut self, id: Uuid) -> impl Future<Output = Result<Segment>> + Send;

    /// Newest first, with the total.
    fn list_segments(
        &mut self,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Segment>, i64)>> + Send;

    /// Delete a segment; its memberships go with it, its customers stay.
    fn delete_segment(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;

    /// The segment's customers, newest first, with the total.
    fn get_segment_members(
        &mut self,
        id: Uuid,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Customer>, i64)>> + Send;

    /// Add a customer to a static segment (adding again changes nothing).
    /// `Error::Invalid` for a dynamic segment; `Error::NotFound` for a segment
    /// or customer this organization does not have.
    fn add_segment_member(
        &mut self,
        segment_id: Uuid,
        customer_id: Uuid,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Remove a customer from a static segment. `Error::NotFound` when they
    /// were not a member.
    fn remove_segment_member(
        &mut self,
        segment_id: Uuid,
        customer_id: Uuid,
    ) -> impl Future<Output = Result<()>> + Send;
}

impl SegmentsExt for Tx<'_> {
    async fn create_segment(&mut self, new: &NewSegment) -> Result<Segment> {
        let name = new.name.trim();
        if name.is_empty() {
            return Err(invalid("a segment needs a name"));
        }
        let segment_type = new.segment_type.as_deref().unwrap_or("dynamic");
        match segment_type {
            "dynamic" => {
                Criteria::parse(&new.criteria)?;
            }
            "static" => {
                if !new.criteria.is_object() {
                    return Err(invalid("criteria must be a JSON object"));
                }
            }
            _ => return Err(invalid("segment type must be `dynamic` or `static`")),
        }
        let org = self.org();
        Ok(sqlx::query_as(&format!(
            "INSERT INTO segments (org_id, name, description, criteria, segment_type) \
             VALUES ($1, $2, $3, $4, $5) RETURNING {SEGMENT_COLS}"
        ))
        .bind(org)
        .bind(name)
        .bind(&new.description)
        .bind(&new.criteria)
        .bind(segment_type)
        .fetch_one(self.conn())
        .await?)
    }

    async fn get_segment(&mut self, id: Uuid) -> Result<Segment> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {SEGMENT_COLS} FROM segments WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "segment" })
    }

    async fn list_segments(&mut self, page: Page) -> Result<(Vec<Segment>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {SEGMENT_COLS} FROM segments WHERE org_id = $1 \
             ORDER BY created_at DESC, id LIMIT $2 OFFSET $3"
        ))
        .bind(org)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as("SELECT count(*) FROM segments WHERE org_id = $1")
            .bind(org)
            .fetch_one(self.conn())
            .await?;
        Ok((rows, total))
    }

    async fn delete_segment(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM segments WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "segment" });
        }
        Ok(())
    }

    async fn get_segment_members(&mut self, id: Uuid, page: Page) -> Result<(Vec<Customer>, i64)> {
        let segment = self.get_segment(id).await?;
        let org = self.org();
        if segment.segment_type == "static" {
            const MEMBER: &str = "org_id = $1 AND id IN \
                 (SELECT customer_id FROM segment_members WHERE org_id = $1 AND segment_id = $2)";
            let rows = sqlx::query_as(&format!(
                "SELECT {CUSTOMER_COLS} FROM customers WHERE {MEMBER} \
                 ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
            ))
            .bind(org)
            .bind(id)
            .bind(page.limit())
            .bind(page.offset())
            .fetch_all(self.conn())
            .await?;
            let (total,): (i64,) =
                sqlx::query_as(&format!("SELECT count(*) FROM customers WHERE {MEMBER}"))
                    .bind(org)
                    .bind(id)
                    .fetch_one(self.conn())
                    .await?;
            return Ok((rows, total));
        }

        let c = Criteria::parse(&segment.criteria)?;
        const MATCH: &str = "org_id = $1 \
             AND ($2::text IS NULL OR $2 = ANY(tags)) \
             AND ($3::text IS NULL OR company = $3) \
             AND ($4::timestamptz IS NULL OR created_at > $4) \
             AND ($5::timestamptz IS NULL OR created_at < $5)";
        let rows = sqlx::query_as(&format!(
            "SELECT {CUSTOMER_COLS} FROM customers WHERE {MATCH} \
             ORDER BY created_at DESC, id LIMIT $6 OFFSET $7"
        ))
        .bind(org)
        .bind(&c.tag)
        .bind(&c.company)
        .bind(c.after)
        .bind(c.before)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) =
            sqlx::query_as(&format!("SELECT count(*) FROM customers WHERE {MATCH}"))
                .bind(org)
                .bind(&c.tag)
                .bind(&c.company)
                .bind(c.after)
                .bind(c.before)
                .fetch_one(self.conn())
                .await?;
        Ok((rows, total))
    }

    async fn add_segment_member(&mut self, segment_id: Uuid, customer_id: Uuid) -> Result<()> {
        let segment = self.get_segment(segment_id).await?;
        if segment.segment_type != "static" {
            return Err(invalid(
                "only a static segment has stored members; a dynamic segment's members \
                 are the customers its criteria match",
            ));
        }
        let org = self.org();
        let customer: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM customers WHERE org_id = $1 AND id = $2")
                .bind(org)
                .bind(customer_id)
                .fetch_optional(self.conn())
                .await?;
        if customer.is_none() {
            return Err(Error::NotFound { kind: "customer" });
        }
        sqlx::query(
            "INSERT INTO segment_members (org_id, segment_id, customer_id) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(org)
        .bind(segment_id)
        .bind(customer_id)
        .execute(self.conn())
        .await?;
        Ok(())
    }

    async fn remove_segment_member(&mut self, segment_id: Uuid, customer_id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query(
            "DELETE FROM segment_members WHERE org_id = $1 AND segment_id = $2 AND customer_id = $3",
        )
        .bind(org)
        .bind(segment_id)
        .bind(customer_id)
        .execute(self.conn())
        .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound {
                kind: "segment member",
            });
        }
        Ok(())
    }
}
