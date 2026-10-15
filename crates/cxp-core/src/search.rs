//! What customer and knowledge-base search share (#17): the capped result,
//! the bounds every search stays within, and the backlog of rows written
//! before the search index existed.
//!
//! Under row-level security no index can serve `ILIKE`, pg_trgm's `%`, or
//! full-text `@@` (none is leakproof), so search looks candidates up in its
//! own index tables (`customer_search_grams`, `knowledge_search_terms`,
//! migration 0004) with leakproof btree conditions, and judges each
//! candidate with the original predicate. Every step reads a bounded number
//! of rows, so a broad query costs no more than a narrow one: matches are
//! counted only up to [`TOTAL_CAP`], and when a bound cuts the search short
//! the result says so ([`SearchPage::total_capped`]).
//!
//! Design and numbers: docs/specs/2026-10-14-search-under-rls-design.md.
//! What proves the statements equal to the pre-#17 scans: the equivalence
//! and random-corpus tests in `crates/cxp-core/tests/customers.rs` and
//! `tests/support.rs`, against the oracle in `tests/common/mod.rs`
//! (`customers_both_ways`, `articles_both_ways`).

use otto_tenant::Tx;
use serde::Serialize;

use crate::error::{Error, Result};

/// Longest search query, in characters. Probing costs grow with a query's
/// windows and trigrams, so a long one would hold a pooled connection for
/// longer than any search needs; 256 covers any real email address.
pub const MAX_QUERY_CHARS: usize = 256;

/// The longest a search statement may run: a backstop that turns a search
/// gone wrong into an error instead of a connection held from the small
/// shared pool. Every search here runs in tens of milliseconds.
const STATEMENT_TIMEOUT: &str = "2s";

/// Matches are counted up to this many; past it `total` is this and
/// `total_capped` is set, and a page starting at or past it is empty.
pub const TOTAL_CAP: i64 = 1000;

/// How many index rows search counts for one gram, lexeme, or window before
/// calling it common: enough to tell a rare one from a common one, cheap for
/// every gram of a long query.
pub(crate) const DF_PROBE: i64 = 256;

/// Most index rows read for substring (`ILIKE '%q%'`) candidates: the rarest
/// window's rows, or a short query's prefix range.
pub(crate) const SUBSTRING_READ: i64 = 2000;

/// Most customers read newest first, each checked with `ILIKE`, when a
/// substring query's rarest window has more than [`SUBSTRING_READ`] rows.
pub(crate) const NEWEST_READ: i64 = 20_000;

/// Most index rows read for fuzzy (`name % q`) candidates, rarest trigram
/// first.
pub(crate) const FUZZY_READ: i64 = 10_000;

/// Most fuzzy candidates checked against the customers themselves, most
/// shared trigrams first; fewer once the read stopped at [`FUZZY_READ`],
/// when the count is not exact anyway.
pub(crate) const FUZZY_CHECKED: i64 = 1500;
pub(crate) const FUZZY_CHECKED_PAST_READ: i64 = 500;

/// Most index rows read for full-text candidates (the rarest lexeme's).
pub(crate) const FULL_TEXT_READ: i64 = 2000;

/// Most articles ranked with `ts_rank` (which reads each one's stored
/// `search_tsv`) when a several-word query has more than [`TOTAL_CAP`]
/// matches; up to the cap, every match is ranked.
pub(crate) const RANKED_PAST_CAP: i64 = 200;

/// Most not-yet-indexed rows a search reads directly (see
/// [`index_backlog`]).
pub(crate) const BACKLOG_READ: i64 = 200;

/// One page of search results. The contract:
///
/// - `total_capped` false: `items`, their order, and `total` are identical
///   to the pre-#17 search (every match counted; the page in the usual
///   order).
/// - `total_capped` true: `total` is a lower bound (the cap itself when more
///   than [`TOTAL_CAP`] match), and `items` are the best-ranked of the
///   matches the search found before it stopped counting.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub total_capped: bool,
}

/// What the search statements return: the capped count, whether it is
/// capped, and the page's ids in order.
#[derive(sqlx::FromRow)]
pub(crate) struct Hits {
    pub total: i64,
    pub capped: bool,
    pub ids: Vec<uuid::Uuid>,
}

/// The table whose backlog [`index_backlog`] works through. Public only for
/// the backlog tests (`tests/search_backlog.rs`); searches call it through
/// [`prepare`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indexed {
    Customers,
    KnowledgeArticles,
}

impl Indexed {
    fn table(self) -> &'static str {
        match self {
            Indexed::Customers => "customers",
            Indexed::KnowledgeArticles => "knowledge_articles",
        }
    }

    /// How many rows of the backlog a search indexes before it searches. A
    /// customer writes about 50 index rows and an article about 160, so a
    /// batch is a few hundred to a few thousand index rows: tens of
    /// milliseconds once, on the volumes otto-cxp holds today.
    fn batch(self) -> i64 {
        match self {
            Indexed::Customers => 50,
            Indexed::KnowledgeArticles => 10,
        }
    }

    /// The order a batch is taken in: the partial backlog index's. Articles
    /// take published ones first, the only ones search reads.
    fn batch_order(self) -> &'static str {
        match self {
            Indexed::Customers => "id",
            Indexed::KnowledgeArticles => "published DESC, id DESC",
        }
    }
}

/// Index up to `limit` of this org's rows that predate the search index
/// (`search_indexed` is false), returning how many it indexed. Setting the
/// flag is the whole job: the tables' triggers write the index rows.
///
/// This is why a search, a `cxp:read` request, may write: it writes only
/// its own org's index rows (and the flag), which no caller can see, so
/// the search tools keep `read_only_hint`. Public only for the backlog
/// tests; searches call it through [`prepare`].
///
/// Migration 0004 cannot do this itself: the migrating role is bound by
/// row-level security (#16) and sees no org's rows. So each org's backlog is
/// indexed in its own pinned transaction, in bounded batches: every search
/// indexes one small batch of its org's first, and reads what is left
/// directly (up to [`BACKLOG_READ`] rows), so search is complete meanwhile.
/// Rows locked by a concurrent writer are skipped, not waited for, and the
/// lock taken is `FOR NO KEY UPDATE`, which does not block a concurrent
/// insert that references the row (a ticket for that customer, say, takes
/// `FOR KEY SHARE`). Once an org's backlog is empty this is one probe of an
/// empty partial index.
///
/// The batch is chosen once, in a CTE: as an `IN (...)` subquery it can be
/// run again as the update proceeds, and the lock then skips the rows this
/// statement already updated and hands out more, past the limit.
#[doc(hidden)]
pub async fn index_backlog(tx: &mut Tx<'_>, kind: Indexed, limit: i64) -> Result<u64> {
    let org = tx.org();
    let table = kind.table();
    let order = kind.batch_order();
    Ok(sqlx::query(&format!(
        "WITH batch AS MATERIALIZED ( \
           SELECT id FROM {table} WHERE org_id = $1 AND NOT search_indexed \
           ORDER BY {order} LIMIT $2 FOR NO KEY UPDATE SKIP LOCKED) \
         UPDATE {table} t SET search_indexed = true FROM batch \
         WHERE t.org_id = $1 AND t.id = batch.id"
    ))
    .bind(org)
    .bind(limit)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Before every search, for the rest of the transaction: JIT off (the
/// search statements' generic plans carry cost estimates far above
/// `jit_above_cost`, since the planner cannot see how few index rows a
/// lookup reads, and compiling them takes longer than running them) and
/// [`STATEMENT_TIMEOUT`]; then one batch of the org's backlog indexed.
///
/// The batch runs under a savepoint. If it fails (a row whose index rows
/// cannot be written, say), the search must not: the batch is rolled back,
/// logged with its SQLSTATE only (never the database's text, which may
/// quote customer data), and the search reads that row directly with the
/// rest of the backlog. A failure of the savepoint commands themselves
/// fails the search.
pub(crate) async fn prepare(tx: &mut Tx<'_>, kind: Indexed) -> Result<()> {
    sqlx::query("SET LOCAL jit = off")
        .execute(tx.conn())
        .await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
    ))
    .execute(tx.conn())
    .await?;
    sqlx::query("SAVEPOINT search_backlog")
        .execute(tx.conn())
        .await?;
    if let Err(e) = index_backlog(tx, kind, kind.batch()).await {
        let code = match &e {
            Error::Database(db) => db
                .as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.into_owned()),
            _ => None,
        };
        tracing::warn!(
            table = kind.table(),
            code = code.as_deref().unwrap_or("none"),
            "indexing a batch of the search backlog failed; searching without it"
        );
        sqlx::query("ROLLBACK TO SAVEPOINT search_backlog")
            .execute(tx.conn())
            .await?;
    }
    sqlx::query("RELEASE SAVEPOINT search_backlog")
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// `rows` in the order of `ids` (the order search ranked them in). A row
/// whose id is missing is dropped, which cannot happen inside the one
/// transaction that found the ids.
pub(crate) fn in_order<T>(
    ids: &[uuid::Uuid],
    mut rows: Vec<T>,
    id: impl Fn(&T) -> uuid::Uuid,
) -> Vec<T> {
    let mut out = Vec::with_capacity(ids.len());
    for want in ids {
        if let Some(i) = rows.iter().position(|r| id(r) == *want) {
            out.push(rows.swap_remove(i));
        }
    }
    out
}
