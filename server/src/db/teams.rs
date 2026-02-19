use sqlx::PgPool;
use uuid::Uuid;

use crate::models::team::Team;

#[derive(Debug, Clone)]
pub struct TeamDb {
    pool: PgPool,
}

impl TeamDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        name: &str,
        description: Option<&str>,
        owner: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<Team, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Team>(
            r#"
            INSERT INTO teams (id, name, description, owner, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(owner)
        .bind(&metadata)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Team>, sqlx::Error> {
        sqlx::query_as::<_, Team>("SELECT * FROM teams WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Team>, sqlx::Error> {
        sqlx::query_as::<_, Team>(
            "SELECT * FROM teams ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM teams")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        owner: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<Team>, sqlx::Error> {
        sqlx::query_as::<_, Team>(
            r#"
            UPDATE teams
            SET name = COALESCE($2, name),
                description = COALESCE($3, description),
                owner = COALESCE($4, owner),
                metadata = COALESCE($5, metadata),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(owner)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM teams WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Team>, sqlx::Error> {
        let pattern = format!("%{}%", query);
        sqlx::query_as::<_, Team>(
            r#"
            SELECT * FROM teams
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
