use sqlx::PgPool;
use uuid::Uuid;

use crate::models::ticket::Ticket;

const TICKET_COLS: &str = r#"
    id, customer_id, subject, description,
    status::text as status, priority::text as priority, channel::text as channel,
    assigned_to, tags, metadata,
    experience_id, application_id, page_id, component_id,
    resolved_at, created_at, updated_at
"#;

#[derive(Debug, Clone)]
pub struct TicketDb {
    pool: PgPool,
}

impl TicketDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        customer_id: Uuid,
        subject: &str,
        description: Option<&str>,
        priority: &str,
        channel: Option<&str>,
    ) -> Result<Ticket, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        let sql = format!(
            "INSERT INTO tickets (id, customer_id, subject, description, status, priority, channel, assigned_to, tags, metadata, resolved_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'open', $5::ticket_priority, $6::ticket_channel, NULL, '{{}}', '{{}}', NULL, $7, $8)
             RETURNING {TICKET_COLS}"
        );

        sqlx::query_as::<_, Ticket>(&sql)
            .bind(id)
            .bind(customer_id)
            .bind(subject)
            .bind(description)
            .bind(priority)
            .bind(channel)
            .bind(now)
            .bind(now)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Ticket>, sqlx::Error> {
        let sql = format!("SELECT {TICKET_COLS} FROM tickets WHERE id = $1");
        sqlx::query_as::<_, Ticket>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn update(
        &self,
        id: Uuid,
        status: Option<&str>,
        priority: Option<&str>,
        assigned_to: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<Option<Ticket>, sqlx::Error> {
        let sql = format!(
            "UPDATE tickets
             SET status = COALESCE($2::ticket_status, status),
                 priority = COALESCE($3::ticket_priority, priority),
                 assigned_to = COALESCE($4, assigned_to),
                 tags = COALESCE($5, tags),
                 updated_at = NOW()
             WHERE id = $1
             RETURNING {TICKET_COLS}"
        );

        sqlx::query_as::<_, Ticket>(&sql)
            .bind(id)
            .bind(status)
            .bind(priority)
            .bind(assigned_to)
            .bind(tags)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(
        &self,
        customer_id: Option<Uuid>,
        status: Option<&str>,
        priority: Option<&str>,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Ticket>, sqlx::Error> {
        let sql = format!(
            "SELECT {TICKET_COLS} FROM tickets
             WHERE ($1::uuid IS NULL OR customer_id = $1)
               AND ($2::text IS NULL OR status = $2::ticket_status)
               AND ($3::text IS NULL OR priority = $3::ticket_priority)
               AND ($4::uuid IS NULL OR experience_id = $4)
               AND ($5::uuid IS NULL OR application_id = $5)
               AND ($6::uuid IS NULL OR page_id = $6)
               AND ($7::uuid IS NULL OR component_id = $7)
             ORDER BY created_at DESC
             LIMIT $8 OFFSET $9"
        );

        sqlx::query_as::<_, Ticket>(&sql)
            .bind(customer_id)
            .bind(status)
            .bind(priority)
            .bind(experience_id)
            .bind(application_id)
            .bind(page_id)
            .bind(component_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM tickets WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn count(
        &self,
        status: Option<&str>,
        priority: Option<&str>,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tickets
             WHERE ($1::text IS NULL OR status = $1::ticket_status)
               AND ($2::text IS NULL OR priority = $2::ticket_priority)
               AND ($3::uuid IS NULL OR experience_id = $3)
               AND ($4::uuid IS NULL OR application_id = $4)
               AND ($5::uuid IS NULL OR page_id = $5)
               AND ($6::uuid IS NULL OR component_id = $6)",
        )
        .bind(status)
        .bind(priority)
        .bind(experience_id)
        .bind(application_id)
        .bind(page_id)
        .bind(component_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn close(&self, id: Uuid) -> Result<Option<Ticket>, sqlx::Error> {
        let sql = format!(
            "UPDATE tickets
             SET status = 'closed',
                 resolved_at = NOW(),
                 updated_at = NOW()
             WHERE id = $1
             RETURNING {TICKET_COLS}"
        );

        sqlx::query_as::<_, Ticket>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }
}
