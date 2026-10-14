//! Customer segments and their members: `SegmentsExt` on
//! `otto_tenant::Tx`, with its model structs.
//!
//! A `static` segment's members are the rows in `segment_members`. A `dynamic`
//! segment's members are the customers its `criteria` match, evaluated when
//! asked. Criteria keys (all optional, all ANDed): `tags_contain` (a tag),
//! `company_eq` (exact company), `created_after` / `created_before` (any
//! timestamp Postgres reads, e.g. `2024-01-01` or `2024-01-01 09:30:00`).
//! A timestamp without a zone is read in the database session's `TimeZone`,
//! and `now` and `infinity` are accepted (`now` is re-evaluated on every read
//! of the members). Values are bound, never spliced into SQL.

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
    after: Option<String>,
    before: Option<String>,
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
        Some(_) => Err(invalid(format!(
            "criteria `{key}` must be a string; supported keys: {}",
            CRITERIA_KEYS.join(", ")
        ))),
    }
}

impl Criteria {
    fn parse(criteria: &serde_json::Value) -> Result<Self> {
        let map = criteria
            .as_object()
            .ok_or_else(|| invalid("criteria must be a JSON object"))?;
        if map.keys().any(|k| !CRITERIA_KEYS.contains(&k.as_str())) {
            return Err(invalid(format!(
                "criteria has an unsupported key; supported keys: {}",
                CRITERIA_KEYS.join(", ")
            )));
        }
        Ok(Self {
            tag: criteria_text(map, "tags_contain")?.map(str::to_owned),
            company: criteria_text(map, "company_eq")?.map(str::to_owned),
            after: criteria_text(map, "created_after")?.map(str::to_owned),
            before: criteria_text(map, "created_before")?.map(str::to_owned),
        })
    }

    /// Check both timestamps with `pg_input_is_valid`, which never raises, so
    /// a bad value cannot abort the transaction and the caller sees our
    /// message, never the database's.
    async fn check_times(self, tx: &mut Tx<'_>) -> Result<Self> {
        let (after_ok, before_ok): (bool, bool) = sqlx::query_as(
            "SELECT $1::text IS NULL OR pg_input_is_valid($1::text, 'timestamptz'), \
                    $2::text IS NULL OR pg_input_is_valid($2::text, 'timestamptz')",
        )
        .bind(&self.after)
        .bind(&self.before)
        .fetch_one(tx.conn())
        .await?;
        for (ok, key) in [(after_ok, "created_after"), (before_ok, "created_before")] {
            if !ok {
                return Err(invalid(format!(
                    "criteria `{key}` is not a timestamp; use a date or date and time, \
                     like 2026-01-31 or 2026-01-31T09:30:00Z"
                )));
            }
        }
        Ok(self)
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

    /// The segment's customers, newest first, with the total.
    fn get_segment_members(
        &mut self,
        id: Uuid,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Customer>, i64)>> + Send;
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
                Criteria::parse(&new.criteria)?.check_times(self).await?;
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

        let c = Criteria::parse(&segment.criteria)?
            .check_times(self)
            .await?;
        const MATCH: &str = "org_id = $1 \
             AND ($2::text IS NULL OR $2 = ANY(tags)) \
             AND ($3::text IS NULL OR company = $3) \
             AND ($4::text IS NULL OR created_at > $4::text::timestamptz) \
             AND ($5::text IS NULL OR created_at < $5::text::timestamptz)";
        let rows = sqlx::query_as(&format!(
            "SELECT {CUSTOMER_COLS} FROM customers WHERE {MATCH} \
             ORDER BY created_at DESC, id LIMIT $6 OFFSET $7"
        ))
        .bind(org)
        .bind(&c.tag)
        .bind(&c.company)
        .bind(&c.after)
        .bind(&c.before)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) =
            sqlx::query_as(&format!("SELECT count(*) FROM customers WHERE {MATCH}"))
                .bind(org)
                .bind(&c.tag)
                .bind(&c.company)
                .bind(&c.after)
                .bind(&c.before)
                .fetch_one(self.conn())
                .await?;
        Ok((rows, total))
    }
}
