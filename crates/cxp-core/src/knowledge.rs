//! The knowledge base: `KnowledgeExt` on `otto_tenant::Tx` —
//! articles and their full-text search — with its model structs.

use std::sync::LazyLock;

use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contains_pattern;
use crate::customers::DF_PROBE;
use crate::error::{Error, Result};
use crate::paging::Page;

/// Most articles a search returns.
pub const SEARCH_LIMIT: i64 = 20;
/// Longest search query, in characters.
pub const MAX_QUERY_CHARS: usize = 200;

const ARTICLE_COLS: &str =
    "id, title, content, category, tags, metadata, published, created_at, updated_at";

/// `search_articles`: `$1` org, `$2` the trimmed query, `$3` the title
/// `ILIKE` pattern, `$4` the limit.
///
/// The final `WHERE` is the whole match predicate, exactly as before #17; the
/// CTEs only choose which rows it is evaluated on, through
/// `knowledge_search_terms` (migration 0004), whose lookups are leakproof
/// btree conditions and so index conditions under RLS:
///
/// - `fts_hits`: `plainto_tsquery('english', q)` is the AND of the lexemes
///   `to_tsvector('english', q)` yields (the `english` configuration maps
///   each token to at most one lexeme), so a match holds all of them as `l`
///   terms: the candidates are the articles holding every one (the
///   lexemes' postings, counted per article). A query of stop words alone
///   has no lexemes and matches nothing, as `@@` with an empty query does.
/// - `title_hits`: the title windows, exactly as `search_customers` treats
///   `ILIKE` (see `customers::SEARCH_CUSTOMERS_SQL`).
pub(crate) static SEARCH_ARTICLES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "WITH lexemes AS ( \
           SELECT DISTINCT 'l' || l AS term \
           FROM unnest(tsvector_to_array(to_tsvector('english', $2))) AS l \
         ), windows AS ( \
           SELECT DISTINCT 's' || g AS term FROM search_windows(lower($2)) AS g \
         ), ranked AS ( \
           SELECT term, row_number() OVER (ORDER BY n, term) AS rk FROM ( \
             SELECT w.term, (SELECT count(*) FROM (SELECT 1 FROM knowledge_search_terms t \
                              WHERE t.org_id = $1 AND t.term = w.term LIMIT {DF_PROBE}) AS p) AS n \
             FROM windows AS w) AS c \
         ), fts_hits AS ( \
           SELECT t.article_id FROM knowledge_search_terms t \
           WHERE t.org_id = $1 AND t.term = ANY (ARRAY(SELECT term FROM lexemes)) \
           GROUP BY t.article_id \
           HAVING count(*) = (SELECT count(*) FROM lexemes) \
         ), title_hits AS ( \
           SELECT t.article_id FROM knowledge_search_terms t \
           WHERE t.org_id = $1 \
             AND t.term = (SELECT term FROM ranked WHERE rk = 1) \
             AND NOT EXISTS (SELECT 1 FROM ranked r WHERE r.rk IN (2, 3) \
                   AND NOT EXISTS (SELECT 1 FROM knowledge_search_terms t2 \
                     WHERE t2.org_id = $1 AND t2.term = r.term AND t2.article_id = t.article_id)) \
           UNION \
           SELECT t.article_id FROM knowledge_search_terms t \
           WHERE length(lower($2)) < 4 AND t.org_id = $1 \
             AND t.term >= 's' || lower($2) \
             AND t.term <= 's' || lower($2) || U&'\\+10FFFF\\+10FFFF\\+10FFFF' \
         ) \
         SELECT {ARTICLE_COLS} FROM knowledge_articles \
         WHERE org_id = $1 AND published = true \
           AND id = ANY (ARRAY(SELECT article_id FROM fts_hits \
                               UNION SELECT article_id FROM title_hits)) \
           AND (search_tsv @@ plainto_tsquery('english', $2) OR title ILIKE $3) \
         ORDER BY ts_rank(search_tsv, plainto_tsquery('english', $2)) DESC, \
                  created_at DESC, id \
         LIMIT $4"
    )
});

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
    /// `Invalid`. Matches and ranks the stored `search_tsv`, or a title
    /// containing the query. Neither `@@` nor `ILIKE` is leakproof, so under
    /// row-level security no index can serve them; candidates come from
    /// `knowledge_search_terms` by leakproof btree lookups instead
    /// (`SEARCH_ARTICLES_SQL`).
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
        crate::without_jit(self).await?;
        Ok(sqlx::query_as(&SEARCH_ARTICLES_SQL)
            .bind(org)
            .bind(query)
            .bind(contains_pattern(query))
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
