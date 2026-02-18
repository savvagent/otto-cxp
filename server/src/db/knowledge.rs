use sqlx::PgPool;
use uuid::Uuid;

use crate::models::knowledge::KnowledgeArticle;

#[derive(Debug, Clone)]
pub struct KnowledgeDb {
    pool: PgPool,
}

impl KnowledgeDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_article(
        &self,
        title: &str,
        content: &str,
        category: Option<&str>,
        tags: &[String],
    ) -> Result<KnowledgeArticle, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, KnowledgeArticle>(
            r#"
            INSERT INTO knowledge_articles (id, title, content, category, tags, metadata, published, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, '{}', false, $6, $7)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(content)
        .bind(category)
        .bind(tags)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn search(&self, query: &str) -> Result<Vec<KnowledgeArticle>, sqlx::Error> {
        sqlx::query_as::<_, KnowledgeArticle>(
            r#"
            SELECT * FROM knowledge_articles
            WHERE published = true
              AND (
                  to_tsvector('english', title || ' ' || content) @@ plainto_tsquery('english', $1)
                  OR title ILIKE '%' || $1 || '%'
              )
            ORDER BY
                ts_rank(to_tsvector('english', title || ' ' || content), plainto_tsquery('english', $1)) DESC
            LIMIT 20
            "#,
        )
        .bind(query)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_article(&self, id: Uuid) -> Result<Option<KnowledgeArticle>, sqlx::Error> {
        sqlx::query_as::<_, KnowledgeArticle>(
            r#"SELECT * FROM knowledge_articles WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<KnowledgeArticle>, sqlx::Error> {
        sqlx::query_as::<_, KnowledgeArticle>(
            "SELECT * FROM knowledge_articles ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn update(
        &self,
        id: Uuid,
        title: Option<&str>,
        content: Option<&str>,
        category: Option<&str>,
        tags: Option<&[String]>,
        published: Option<bool>,
    ) -> Result<Option<KnowledgeArticle>, sqlx::Error> {
        sqlx::query_as::<_, KnowledgeArticle>(
            r#"
            UPDATE knowledge_articles
            SET title = COALESCE($2, title),
                content = COALESCE($3, content),
                category = COALESCE($4, category),
                tags = COALESCE($5, tags),
                published = COALESCE($6, published),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(content)
        .bind(category)
        .bind(tags)
        .bind(published)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM knowledge_articles WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
