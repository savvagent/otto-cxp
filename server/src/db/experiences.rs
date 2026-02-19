use sqlx::PgPool;
use uuid::Uuid;

use crate::models::experience::Experience;

#[derive(Debug, Clone)]
pub struct ExperienceDb {
    pool: PgPool,
}

impl ExperienceDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        name: &str,
        description: Option<&str>,
        team_id: Option<Uuid>,
        metadata: serde_json::Value,
    ) -> Result<Experience, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Experience>(
            r#"
            INSERT INTO experiences (id, name, description, team_id, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(team_id)
        .bind(&metadata)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Experience>, sqlx::Error> {
        sqlx::query_as::<_, Experience>("SELECT * FROM experiences WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Experience>, sqlx::Error> {
        sqlx::query_as::<_, Experience>(
            "SELECT * FROM experiences ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM experiences")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        team_id: Option<Uuid>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<Experience>, sqlx::Error> {
        sqlx::query_as::<_, Experience>(
            r#"
            UPDATE experiences
            SET name = COALESCE($2, name),
                description = COALESCE($3, description),
                team_id = COALESCE($4, team_id),
                metadata = COALESCE($5, metadata),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(team_id)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM experiences WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Experience>, sqlx::Error> {
        let pattern = format!("%{}%", query);
        sqlx::query_as::<_, Experience>(
            r#"
            SELECT * FROM experiences
            WHERE name ILIKE $1 OR description ILIKE $1
            ORDER BY created_at DESC
            LIMIT 50
            "#,
        )
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await
    }
}
