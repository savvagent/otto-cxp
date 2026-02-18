use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::customer::Customer;
use crate::models::feedback::Feedback;
use crate::models::interaction::Interaction;
use crate::models::ticket::Ticket;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Customer360 {
    pub customer: Customer,
    pub recent_tickets: Vec<Ticket>,
    pub recent_interactions: Vec<Interaction>,
    pub recent_feedback: Vec<Feedback>,
}

#[derive(Debug, Clone)]
pub struct CustomerDb {
    pool: PgPool,
}

impl CustomerDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        email: &str,
        name: Option<&str>,
        phone: Option<&str>,
        company: Option<&str>,
        metadata: serde_json::Value,
        tags: &[String],
    ) -> Result<Customer, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Customer>(
            r#"
            INSERT INTO customers (id, email, name, phone, company, metadata, tags, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(email)
        .bind(name)
        .bind(phone)
        .bind(company)
        .bind(&metadata)
        .bind(tags)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Customer>, sqlx::Error> {
        sqlx::query_as::<_, Customer>(
            r#"SELECT * FROM customers WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn update(
        &self,
        id: Uuid,
        email: Option<&str>,
        name: Option<&str>,
        phone: Option<&str>,
        company: Option<&str>,
        metadata: Option<serde_json::Value>,
        tags: Option<&[String]>,
    ) -> Result<Option<Customer>, sqlx::Error> {
        sqlx::query_as::<_, Customer>(
            r#"
            UPDATE customers
            SET email = COALESCE($2, email),
                name = COALESCE($3, name),
                phone = COALESCE($4, phone),
                company = COALESCE($5, company),
                metadata = COALESCE($6, metadata),
                tags = COALESCE($7, tags),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(email)
        .bind(name)
        .bind(phone)
        .bind(company)
        .bind(metadata)
        .bind(tags)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Customer>, sqlx::Error> {
        sqlx::query_as::<_, Customer>(
            r#"SELECT * FROM customers ORDER BY created_at DESC LIMIT $1 OFFSET $2"#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_by_email(&self, email: &str) -> Result<Option<Customer>, sqlx::Error> {
        sqlx::query_as::<_, Customer>(
            r#"SELECT * FROM customers WHERE email = $1"#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM customers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM customers")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Customer>, sqlx::Error> {
        let pattern = format!("%{}%", query);

        sqlx::query_as::<_, Customer>(
            r#"
            SELECT * FROM customers
            WHERE email ILIKE $1
               OR name ILIKE $1
               OR similarity(name, $2) > 0.3
            ORDER BY
                CASE WHEN email ILIKE $1 THEN 0 ELSE 1 END,
                similarity(name, $2) DESC
            LIMIT 50
            "#,
        )
        .bind(&pattern)
        .bind(query)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_360(&self, id: Uuid) -> Result<Option<Customer360>, sqlx::Error> {
        let customer = sqlx::query_as::<_, Customer>(
            r#"SELECT * FROM customers WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        let customer = match customer {
            Some(c) => c,
            None => return Ok(None),
        };

        let recent_tickets = sqlx::query_as::<_, Ticket>(
            r#"
            SELECT id, customer_id, subject, description,
                   status::text as status, priority::text as priority, channel::text as channel,
                   assigned_to, tags, metadata, resolved_at, created_at, updated_at
            FROM tickets
            WHERE customer_id = $1
            ORDER BY created_at DESC
            LIMIT 10
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        let recent_interactions = sqlx::query_as::<_, Interaction>(
            r#"
            SELECT id, customer_id, interaction_type::text as interaction_type, metadata, created_at
            FROM interactions
            WHERE customer_id = $1
            ORDER BY created_at DESC
            LIMIT 10
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        let recent_feedback = sqlx::query_as::<_, Feedback>(
            r#"
            SELECT id, customer_id, feedback_type::text as feedback_type,
                   subject, body, rating, metadata,
                   experience_id, application_id, page_id, component_id,
                   created_at
            FROM feedback
            WHERE customer_id = $1
            ORDER BY created_at DESC
            LIMIT 10
            "#,
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        Ok(Some(Customer360 {
            customer,
            recent_tickets,
            recent_interactions,
            recent_feedback,
        }))
    }
}
