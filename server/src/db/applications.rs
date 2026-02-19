use sqlx::PgPool;
use uuid::Uuid;

use crate::models::application::Application;

#[derive(Debug, Clone)]
pub struct ApplicationDb {
    pool: PgPool,
}

impl ApplicationDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        experience_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        platform: Option<&str>,
        team_id: Option<Uuid>,
        metadata: serde_json::Value,
    ) -> Result<Application, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Application>(
            r#"
            INSERT INTO applications (id, experience_id, name, description, platform, team_id, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(experience_id)
        .bind(name)
        .bind(description)
        .bind(platform)
        .bind(team_id)
        .bind(&metadata)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Application>, sqlx::Error> {
        sqlx::query_as::<_, Application>("SELECT * FROM applications WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Application>, sqlx::Error> {
        sqlx::query_as::<_, Application>(
            "SELECT * FROM applications ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_by_experience(
        &self,
        experience_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Application>, sqlx::Error> {
        sqlx::query_as::<_, Application>(
            "SELECT * FROM applications WHERE experience_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(experience_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count(&self, experience_id: Option<Uuid>) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM applications WHERE ($1::uuid IS NULL OR experience_id = $1)",
        )
        .bind(experience_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        platform: Option<&str>,
        team_id: Option<Uuid>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<Application>, sqlx::Error> {
        sqlx::query_as::<_, Application>(
            r#"
            UPDATE applications
            SET name = COALESCE($2, name),
                description = COALESCE($3, description),
                platform = COALESCE($4, platform),
                team_id = COALESCE($5, team_id),
                metadata = COALESCE($6, metadata),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(platform)
        .bind(team_id)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM applications WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
