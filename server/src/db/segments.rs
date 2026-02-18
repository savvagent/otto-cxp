use sqlx::PgPool;
use uuid::Uuid;

use crate::models::customer::Customer;
use crate::models::segment::Segment;

#[derive(Debug, Clone)]
pub struct SegmentDb {
    pool: PgPool,
}

impl SegmentDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        name: &str,
        description: Option<&str>,
        criteria: serde_json::Value,
        segment_type: &str,
    ) -> Result<Segment, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Segment>(
            r#"
            INSERT INTO segments (id, name, description, criteria, segment_type, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(&criteria)
        .bind(segment_type)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Segment>, sqlx::Error> {
        sqlx::query_as::<_, Segment>("SELECT * FROM segments WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Segment>, sqlx::Error> {
        sqlx::query_as::<_, Segment>(
            "SELECT * FROM segments ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_members(&self, segment_id: Uuid) -> Result<Vec<Customer>, sqlx::Error> {
        // First fetch the segment to determine its type
        let segment = sqlx::query_as::<_, Segment>(
            r#"SELECT * FROM segments WHERE id = $1"#,
        )
        .bind(segment_id)
        .fetch_optional(&self.pool)
        .await?;

        let segment = match segment {
            Some(s) => s,
            None => return Ok(vec![]),
        };

        if segment.segment_type == "static" {
            // For static segments, join through segment_members table
            return sqlx::query_as::<_, Customer>(
                r#"
                SELECT c.* FROM customers c
                INNER JOIN segment_members sm ON c.id = sm.customer_id
                WHERE sm.segment_id = $1
                "#,
            )
            .bind(segment_id)
            .fetch_all(&self.pool)
            .await;
        }

        // For dynamic segments, parse criteria JSON and build a WHERE clause
        // Supported criteria keys: tags_contain, company_eq, created_after, created_before
        let criteria = &segment.criteria;
        let mut conditions: Vec<String> = vec!["1=1".to_string()];

        // We build the query dynamically based on criteria.
        // Because sqlx does not support truly dynamic bind params easily with query_as,
        // we build the full SQL string with inlined safe values for simple types.
        // For safety, we validate and sanitize values.

        if let Some(tag) = criteria.get("tags_contain").and_then(|v| v.as_str()) {
            let sanitized = tag.replace('\'', "''");
            conditions.push(format!("'{}' = ANY(tags)", sanitized));
        }

        if let Some(company) = criteria.get("company_eq").and_then(|v| v.as_str()) {
            let sanitized = company.replace('\'', "''");
            conditions.push(format!("company = '{}'", sanitized));
        }

        if let Some(after) = criteria.get("created_after").and_then(|v| v.as_str()) {
            let sanitized = after.replace('\'', "''");
            conditions.push(format!("created_at > '{}'", sanitized));
        }

        if let Some(before) = criteria.get("created_before").and_then(|v| v.as_str()) {
            let sanitized = before.replace('\'', "''");
            conditions.push(format!("created_at < '{}'", sanitized));
        }

        let where_clause = conditions.join(" AND ");
        let query = format!("SELECT * FROM customers WHERE {}", where_clause);

        sqlx::query_as::<_, Customer>(&query)
            .fetch_all(&self.pool)
            .await
    }
}
