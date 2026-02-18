use sqlx::PgPool;
use uuid::Uuid;

use crate::models::component::Component;

#[derive(Debug, Clone)]
pub struct ComponentDb {
    pool: PgPool,
}

impl ComponentDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        page_id: Option<Uuid>,
        name: &str,
        component_type: Option<&str>,
        description: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<Component, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Component>(
            r#"
            INSERT INTO components (id, page_id, name, component_type, description, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(page_id)
        .bind(name)
        .bind(component_type)
        .bind(description)
        .bind(&metadata)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Component>, sqlx::Error> {
        sqlx::query_as::<_, Component>("SELECT * FROM components WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Component>, sqlx::Error> {
        sqlx::query_as::<_, Component>(
            "SELECT * FROM components ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_by_page(
        &self,
        page_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Component>, sqlx::Error> {
        sqlx::query_as::<_, Component>(
            "SELECT * FROM components WHERE page_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(page_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count(&self, page_id: Option<Uuid>) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM components WHERE ($1::uuid IS NULL OR page_id = $1)",
        )
        .bind(page_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        component_type: Option<&str>,
        description: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Option<Component>, sqlx::Error> {
        sqlx::query_as::<_, Component>(
            r#"
            UPDATE components
            SET name = COALESCE($2, name),
                component_type = COALESCE($3, component_type),
                description = COALESCE($4, description),
                metadata = COALESCE($5, metadata),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(name)
        .bind(component_type)
        .bind(description)
        .bind(metadata)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM components WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
