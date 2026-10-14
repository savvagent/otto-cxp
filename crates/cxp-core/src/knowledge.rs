//! The knowledge base (story S6): `KnowledgeExt` on `otto_tenant::Tx` —
//! articles and their full-text search — with its model structs.

use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::paging::Page;

/// Most articles a search returns.
pub const SEARCH_LIMIT: i64 = 20;
/// Longest search query, in characters.
pub const MAX_QUERY_CHARS: usize = 200;

const ARTICLE_COLS: &str =
    "id, title, content, category, tags, metadata, published, created_at, updated_at";

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeArticle {
    pub id: Uuid,
    pub title: String,
    pub content: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub published: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A new article. It starts unpublished; search only finds published ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewArticle {
    pub title: String,
    pub content: String,
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Fields to change on an article; `None` leaves a field as it is, so a field
/// cannot be cleared once set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArticleUpdate {
    pub title: Option<String>,
    pub content: Option<String>,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    pub published: Option<bool>,
}

/// Escape `\`, `%` and `_` so user text is matched literally by `ILIKE`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub trait KnowledgeExt {
    fn create_article(
        &mut self,
        new: &NewArticle,
    ) -> impl std::future::Future<Output = Result<KnowledgeArticle>> + Send;

    /// Any article of this org, published or not (the
    /// `knowledge://articles/{id}` resource).
    fn get_article(
        &mut self,
        id: Uuid,
    ) -> impl std::future::Future<Output = Result<KnowledgeArticle>> + Send;

    /// Full-text search over published articles, best match first, at most
    /// [`SEARCH_LIMIT`]. An empty query, or one over [`MAX_QUERY_CHARS`], is
    /// `Invalid`.
    fn search_articles(
        &mut self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<KnowledgeArticle>>> + Send;

    /// Newest first, with the total; `published` filters when set.
    fn list_articles(
        &mut self,
        published: Option<bool>,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<KnowledgeArticle>, i64)>> + Send;

    fn update_article(
        &mut self,
        id: Uuid,
        update: &ArticleUpdate,
    ) -> impl std::future::Future<Output = Result<KnowledgeArticle>> + Send;

    fn delete_article(&mut self, id: Uuid) -> impl std::future::Future<Output = Result<()>> + Send;
}

impl KnowledgeExt for Tx<'_> {
    async fn create_article(&mut self, new: &NewArticle) -> Result<KnowledgeArticle> {
        let org = self.org();
        Ok(sqlx::query_as(&format!(
            "INSERT INTO knowledge_articles \
               (org_id, title, content, category, tags, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, clock_timestamp(), clock_timestamp()) RETURNING {ARTICLE_COLS}"
        ))
        .bind(org)
        .bind(&new.title)
        .bind(&new.content)
        .bind(&new.category)
        .bind(&new.tags)
        .fetch_one(self.conn())
        .await?)
    }

    async fn get_article(&mut self, id: Uuid) -> Result<KnowledgeArticle> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {ARTICLE_COLS} FROM knowledge_articles WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "article" })
    }

    async fn search_articles(&mut self, query: &str) -> Result<Vec<KnowledgeArticle>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::Invalid("query must not be empty".into()));
        }
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err(Error::Invalid(format!(
                "query must be at most {MAX_QUERY_CHARS} characters"
            )));
        }
        let org = self.org();
        Ok(sqlx::query_as(&format!(
            "SELECT {ARTICLE_COLS} FROM knowledge_articles \
             WHERE org_id = $1 AND published = true \
               AND (to_tsvector('english', title || ' ' || content) \
                      @@ plainto_tsquery('english', $2) \
                    OR title ILIKE '%' || $3 || '%') \
             ORDER BY ts_rank(to_tsvector('english', title || ' ' || content), \
                              plainto_tsquery('english', $2)) DESC, created_at DESC, id \
             LIMIT $4"
        ))
        .bind(org)
        .bind(query)
        .bind(escape_like(query))
        .bind(SEARCH_LIMIT)
        .fetch_all(self.conn())
        .await?)
    }

    async fn list_articles(
        &mut self,
        published: Option<bool>,
        page: Page,
    ) -> Result<(Vec<KnowledgeArticle>, i64)> {
        let org = self.org();
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM knowledge_articles \
             WHERE org_id = $1 AND ($2::boolean IS NULL OR published = $2)",
        )
        .bind(org)
        .bind(published)
        .fetch_one(self.conn())
        .await?;
        let rows = sqlx::query_as(&format!(
            "SELECT {ARTICLE_COLS} FROM knowledge_articles \
             WHERE org_id = $1 AND ($2::boolean IS NULL OR published = $2) \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(published)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn update_article(
        &mut self,
        id: Uuid,
        update: &ArticleUpdate,
    ) -> Result<KnowledgeArticle> {
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE knowledge_articles SET \
               title = COALESCE($3, title), content = COALESCE($4, content), \
               category = COALESCE($5, category), tags = COALESCE($6, tags), \
               published = COALESCE($7, published), updated_at = clock_timestamp() \
             WHERE org_id = $1 AND id = $2 RETURNING {ARTICLE_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.title)
        .bind(&update.content)
        .bind(&update.category)
        .bind(&update.tags)
        .bind(update.published)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "article" })
    }

    async fn delete_article(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM knowledge_articles WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "article" });
        }
        Ok(())
    }
}
