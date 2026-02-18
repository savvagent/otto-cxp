use sqlx::PgPool;
use uuid::Uuid;

use crate::models::page::Page;

#[derive(Debug, Clone)]
pub struct PageDb {
    pool: PgPool,
}

impl PageDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        application_id: Option<Uuid>,
        name: &str,
        path: Option<&str>,
        description: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<Page, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Page>(
            r#"
            INSERT INTO pages (id, application_id, name, path, description, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(application_id)
        .bind(name)
        .bind(path)
        .bind(description)
        .bind(&metadata)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Page>, sqlx::Error> {
        sqlx::query_as::<_, Page>("SELECT * FROM pages WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Page>, sqlx::Error> {
        sqlx::query_as::<_, Page>(
            "SELECT * FROM pages ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_by_application(
        &self,
        application_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Page>, sqlx::Error> {
        sqlx::query_as::<_, Page>(
            "SELECT * FROM pages WHERE application_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(application_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count(&self, application_id: Option<Uuid>) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pages WHERE ($1::uuid IS NULL OR application_id = $1)",
        )
        .bind(application_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        path: Option<&str>,
        description: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<Page>, sqlx::Error> {
        sqlx::query_as::<_, Page>(
            r#"
            UPDATE pages
            SET name = COALESCE($2, name),
                path = COALESCE($3, path),
                description = COALESCE($4, description),
                metadata = COALESCE($5, metadata),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(path)
        .bind(description)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM pages WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
