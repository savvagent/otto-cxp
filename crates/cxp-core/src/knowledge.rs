//! The knowledge base: `KnowledgeExt` on `otto_tenant::Tx` —
//! articles and their full-text search — with its model structs.

use std::sync::LazyLock;

use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contains_pattern;
use crate::error::{Error, Result};
use crate::paging::Page;
use crate::search::{
    self, Hits, Indexed, SearchPage, BACKLOG_READ, DF_PROBE, FULL_TEXT_READ, MAX_QUERY_CHARS,
    RANKED_PAST_CAP, SUBSTRING_READ, TOTAL_CAP,
};

/// Most articles a search returns.
pub const SEARCH_LIMIT: i64 = 20;
/// Longest article title, in characters. Search indexes every 4-character
/// window of a title (migration 0004), so its length is what an article's
/// title index rows grow with.
pub const MAX_TITLE_CHARS: usize = 500;

const ARTICLE_COLS: &str =
    "id, title, content, category, tags, metadata, published, created_at, updated_at";

/// `search_articles`: `$1` org, `$2` the trimmed query, `$3` the title
/// `ILIKE` pattern, `$4` the limit. Returns one row: the capped count,
/// whether it is capped, and the page's ids in order ([`crate::search`]).
///
/// The predicate and the order are exactly those before #17 (published, and
/// `search_tsv @@ plainto_tsquery('english', q) OR title ILIKE '%q%'`; by
/// `ts_rank`, newest first). The candidates come from
/// `knowledge_search_terms` (migration 0004), which holds published
/// articles only, by leakproof btree lookups, each reading a bounded number
/// of index rows:
///
/// - `fts_read`: `plainto_tsquery('english', q)` is the AND of the lexemes
///   `to_tsvector('english', q)` yields (the `english` configuration maps
///   each token to at most one lexeme), so a match holds all of them as `l`
///   terms. Up to [`FULL_TEXT_READ`] rows of the rarest lexeme are read
///   (counts capped at [`DF_PROBE`]); `fts_hits` keeps those that hold the
///   others too, which is exactly whether `@@` holds. A query of stop words
///   alone has no lexemes and matches nothing, as `@@` with an empty query
///   does.
/// - `title_hits`: the title windows, as `search_customers` treats `ILIKE`
///   (see `customers::SEARCH_CUSTOMERS_SQL`); these are checked against the
///   title itself. Only the articles `fts_hits` already holds are left out
///   here: one that has the rarest lexeme but lacks another is not a
///   full-text match, yet may still match by title.
/// - `backlog`: published articles that predate the index, up to
///   [`BACKLOG_READ`], checked with the whole predicate.
///
/// Ranking needs no `ts_rank` for a one-lexeme query: there it is a strictly
/// increasing function of the lexeme's count of positions (`tf`), which the
/// index holds, and an article without the lexeme ranks below every one with
/// it. The lexeme's rows are read most positions first (the partial index on
/// `tf >= 2`), and every row read that ties the page's last `tf` is ranked,
/// newest first, so the page is the scan's whenever the read holds the
/// best. A longer query is ranked with `ts_rank` itself, on every match up to
/// [`TOTAL_CAP`], or on the [`RANKED_PAST_CAP`] with the highest `tf` of the
/// rarest lexeme past it; that includes title-only matches, which `ts_rank`
/// scores above zero when they hold some of the query's lexemes.
pub(crate) static SEARCH_ARTICLES_SQL: LazyLock<String> = LazyLock::new(|| {
    let cap = TOTAL_CAP;
    format!(
        "WITH tsq AS MATERIALIZED ( \
           SELECT plainto_tsquery('english', $2) AS q, \
                  numnode(plainto_tsquery('english', $2)) = 1 AS single \
         ), lxr AS MATERIALIZED ( \
           SELECT term, row_number() OVER (ORDER BY n, term) AS rk FROM ( \
             SELECT lx.term, (SELECT count(*) FROM (SELECT 1 FROM knowledge_search_terms t \
                                WHERE t.org_id = $1 AND t.term = lx.term LIMIT {DF_PROBE}) AS p) AS n \
             FROM (SELECT DISTINCT 'l' || l AS term \
                   FROM unnest(tsvector_to_array(to_tsvector('english', $2))) AS l) AS lx) AS c \
         ), fts_read AS MATERIALIZED ( \
           (SELECT t.article_id, t.tf FROM knowledge_search_terms t \
            WHERE t.org_id = $1 AND t.term = (SELECT term FROM lxr WHERE rk = 1) AND t.tf >= 2 \
            ORDER BY t.tf DESC, t.article_id \
            LIMIT {fts_read}) \
           UNION ALL \
           (SELECT t.article_id, t.tf FROM knowledge_search_terms t \
            WHERE t.org_id = $1 AND t.term = (SELECT term FROM lxr WHERE rk = 1) AND t.tf < 2 \
            LIMIT {fts_read}) \
           LIMIT {fts_read} \
         ), lq AS MATERIALIZED ( \
           SELECT lower($2) AS q \
         ), tw AS ( \
           SELECT gram, row_number() OVER (ORDER BY n, gram) AS rk FROM ( \
             SELECT d.gram, (SELECT count(*) FROM (SELECT 1 FROM knowledge_search_terms t \
                               WHERE t.org_id = $1 AND t.term = d.gram LIMIT {DF_PROBE}) AS p) AS n \
             FROM (SELECT DISTINCT 's' || x AS gram FROM lq, search_windows(lq.q) AS x) AS d) AS c \
         ), title_read AS MATERIALIZED ( \
           SELECT t.article_id FROM knowledge_search_terms t \
           WHERE t.org_id = $1 AND t.term = (SELECT gram FROM tw WHERE rk = 1) \
           UNION ALL \
           SELECT t.article_id FROM knowledge_search_terms t \
           WHERE length(lower($2)) < 4 AND t.org_id = $1 \
             AND t.term >= 's' || lower($2) \
             AND t.term <= 's' || lower($2) || U&'\\+10FFFF\\+10FFFF\\+10FFFF' \
           LIMIT {title_read} \
         ), title_hits AS ( \
           SELECT DISTINCT r.article_id FROM title_read r \
           WHERE NOT EXISTS (SELECT 1 FROM tw WHERE tw.rk IN (2, 3) AND NOT EXISTS ( \
                   SELECT 1 FROM knowledge_search_terms t \
                   WHERE t.org_id = $1 AND t.term = tw.gram AND t.article_id = r.article_id)) \
         ), backlog AS MATERIALIZED ( \
           SELECT id AS article_id FROM knowledge_articles \
           WHERE org_id = $1 AND published AND NOT search_indexed \
           LIMIT {backlog} \
         ), fts_hits AS MATERIALIZED ( \
           SELECT r.article_id, r.tf FROM fts_read r \
           WHERE NOT EXISTS (SELECT 1 FROM lxr WHERE lxr.rk > 1 AND NOT EXISTS ( \
                   SELECT 1 FROM knowledge_search_terms t \
                   WHERE t.org_id = $1 AND t.term = lxr.term \
                     AND t.article_id = r.article_id)) \
           ORDER BY r.tf DESC, r.article_id \
         ), matches AS MATERIALIZED ( \
           SELECT article_id, fts, tf FROM ( \
             SELECT article_id, true AS fts, tf FROM fts_hits \
             UNION ALL \
             SELECT h.article_id, x.fts, x.tf \
             FROM title_hits h \
             CROSS JOIN LATERAL ( \
               SELECT tf, tf IS NOT NULL AND NOT EXISTS ( \
                        SELECT 1 FROM lxr WHERE lxr.rk > 1 AND NOT EXISTS ( \
                          SELECT 1 FROM knowledge_search_terms t \
                          WHERE t.org_id = $1 AND t.term = lxr.term \
                            AND t.article_id = h.article_id)) AS fts \
               FROM (SELECT (SELECT t.tf FROM knowledge_search_terms t \
                             WHERE t.org_id = $1 AND t.term = (SELECT term FROM lxr WHERE rk = 1) \
                               AND t.article_id = h.article_id) AS tf) AS y) AS x \
             WHERE h.article_id NOT IN (SELECT article_id FROM fts_hits) \
               AND (x.fts OR EXISTS ( \
                 SELECT 1 FROM knowledge_articles a \
                 WHERE a.org_id = $1 AND a.id = h.article_id AND a.published \
                   AND a.title ILIKE $3)) \
             UNION ALL \
             SELECT a.id, a.search_tsv @@ tsq.q, \
                    (SELECT coalesce(cardinality(u.positions), 1)::smallint \
                     FROM unnest(a.search_tsv) AS u \
                     WHERE 'l' || u.lexeme = (SELECT term FROM lxr WHERE rk = 1)) \
             FROM backlog b, tsq, knowledge_articles a \
             WHERE a.org_id = $1 AND a.id = b.article_id AND a.published \
               AND (a.search_tsv @@ tsq.q OR a.title ILIKE $3)) AS u \
           LIMIT {cap_read} \
         ), ranked AS ( \
           SELECT m.article_id, a.created_at, \
                  CASE WHEN NOT tsq.single THEN ts_rank(a.search_tsv, tsq.q) \
                       WHEN m.fts THEN m.tf ELSE 0 END AS rank \
           FROM (SELECT article_id, fts, tf FROM matches \
                 WHERE (SELECT single FROM tsq) \
                    OR (SELECT count(*) FROM matches WHERE fts) <= {cap} \
                 UNION \
                 (SELECT article_id, fts, tf FROM matches \
                  WHERE NOT (SELECT single FROM tsq) \
                  ORDER BY fts DESC, tf DESC NULLS LAST, article_id \
                  LIMIT {RANKED_PAST_CAP}) \
                 UNION \
                 SELECT article_id, true, tf FROM fts_read \
                 WHERE (SELECT single FROM tsq) \
                   AND tf >= (SELECT min(tf) FROM (SELECT tf FROM fts_read \
                                                   ORDER BY tf DESC LIMIT $4) AS top)) AS m \
           CROSS JOIN tsq \
           JOIN knowledge_articles a ON a.org_id = $1 AND a.id = m.article_id \
         ) \
         SELECT least((SELECT count(*) FROM matches), {cap}) AS total, \
                (SELECT count(*) FROM matches) > {cap} \
                  OR (SELECT count(*) FROM fts_read) > {FULL_TEXT_READ} \
                  OR (SELECT count(*) FROM title_read) > {SUBSTRING_READ} \
                  OR (SELECT count(*) FROM backlog) > {BACKLOG_READ} AS capped, \
                ARRAY(SELECT article_id FROM ranked \
                      ORDER BY rank DESC, created_at DESC, article_id LIMIT $4) AS ids",
        fts_read = FULL_TEXT_READ + 1,
        title_read = SUBSTRING_READ + 1,
        backlog = BACKLOG_READ + 1,
        cap_read = cap + 1,
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
    /// A title over [`MAX_TITLE_CHARS`] is `Invalid`, here and in
    /// [`KnowledgeExt::update_article`].
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
    /// containing the query. Matches are counted up to [`TOTAL_CAP`]; past
    /// it, or when a bound cut the search short, the articles are the best
    /// of the matches found ([`SearchPage`]). Neither `@@` nor `ILIKE` is
    /// leakproof, so under row-level security no index can serve them;
    /// candidates come from `knowledge_search_terms` by leakproof btree
    /// lookups instead (`SEARCH_ARTICLES_SQL`).
    fn search_articles(
        &mut self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<SearchPage<KnowledgeArticle>>> + Send;

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

/// A title search can index: at most [`MAX_TITLE_CHARS`].
fn checked_title(title: &str) -> Result<&str> {
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(Error::Invalid(format!(
            "an article title must be at most {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(title)
}

impl KnowledgeExt for Tx<'_> {
    async fn create_article(&mut self, new: &NewArticle) -> Result<KnowledgeArticle> {
        checked_title(&new.title)?;
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

    async fn search_articles(&mut self, query: &str) -> Result<SearchPage<KnowledgeArticle>> {
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
        search::prepare(self, Indexed::KnowledgeArticles).await?;
        let hits: Hits = sqlx::query_as(&SEARCH_ARTICLES_SQL)
            .bind(org)
            .bind(query)
            .bind(contains_pattern(query))
            .bind(SEARCH_LIMIT)
            .fetch_one(self.conn())
            .await?;
        let rows: Vec<KnowledgeArticle> = sqlx::query_as(&format!(
            "SELECT {ARTICLE_COLS} FROM knowledge_articles WHERE org_id = $1 AND id = ANY ($2)"
        ))
        .bind(org)
        .bind(&hits.ids)
        .fetch_all(self.conn())
        .await?;
        Ok(SearchPage {
            items: search::in_order(&hits.ids, rows, |a| a.id),
            total: hits.total,
            total_capped: hits.capped,
        })
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
        update.title.as_deref().map(checked_title).transpose()?;
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
