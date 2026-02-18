use sqlx::PgPool;
use uuid::Uuid;

use crate::models::interaction::Interaction;

const INTERACTION_COLS: &str = r#"
    id, customer_id, interaction_type::text as interaction_type, metadata, created_at
"#;

#[derive(Debug, Clone)]
pub struct InteractionDb {
    pool: PgPool,
}

impl InteractionDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn record(
        &self,
        customer_id: Uuid,
        interaction_type: &str,
        metadata: serde_json::Value,
    ) -> Result<Interaction, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        let sql = format!(
            "INSERT INTO interactions (id, customer_id, interaction_type, metadata, created_at)
             VALUES ($1, $2, $3::interaction_type, $4, $5)
             RETURNING {INTERACTION_COLS}"
        );

        sqlx::query_as::<_, Interaction>(&sql)
            .bind(id)
            .bind(customer_id)
            .bind(interaction_type)
            .bind(&metadata)
            .bind(now)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn list(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Interaction>, sqlx::Error> {
        let sql = format!(
            "SELECT {INTERACTION_COLS} FROM interactions ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        );
        sqlx::query_as::<_, Interaction>(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn get_journey(
        &self,
        customer_id: Uuid,
        limit: i64,
    ) -> Result<Vec<Interaction>, sqlx::Error> {
        let sql = format!(
            "SELECT {INTERACTION_COLS} FROM interactions
             WHERE customer_id = $1
             ORDER BY created_at DESC
             LIMIT $2"
        );

        sqlx::query_as::<_, Interaction>(&sql)
            .bind(customer_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
    }
}
