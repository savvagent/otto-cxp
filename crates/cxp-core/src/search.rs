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

use otto_tenant::Tx;
use serde::Serialize;

use crate::error::Result;

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

/// One page of search results. `total` counts matches up to [`TOTAL_CAP`].
/// `total_capped` is false when `total` is exact; true when there are more
/// matches than the cap (`total` is then the cap) or, rarely, when a bound
/// stopped the search before it had seen every match (`total` then counts
/// the matches it found). The items are the requested page of the matches
/// found, in the search's usual order.
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

/// The table whose backlog [`index_backlog`] works through.
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
}

/// Index up to `limit` of this org's rows that predate the search index
/// (`search_indexed` is false), returning how many it indexed. Setting the
/// flag is the whole job: the tables' triggers write the index rows.
///
/// Migration 0004 cannot do this itself: the migrating role is bound by
/// row-level security (#16) and sees no org's rows. So each org's backlog is
/// indexed in its own pinned transaction, in bounded batches: every search
/// indexes one small batch of its org's first, and reads what is left
/// directly (up to [`BACKLOG_READ`] rows), so search is complete meanwhile.
/// Rows locked by a concurrent writer are skipped, not waited for. Once an
/// org's backlog is empty this is one probe of an empty partial index.
///
/// The batch is chosen once, in a CTE: as an `IN (...)` subquery it can be
/// run again as the update proceeds, and `FOR UPDATE` then skips the rows
/// this statement already updated and hands out more, past the limit.
pub async fn index_backlog(tx: &mut Tx<'_>, kind: Indexed, limit: i64) -> Result<u64> {
    let org = tx.org();
    let table = kind.table();
    Ok(sqlx::query(&format!(
        "WITH batch AS MATERIALIZED ( \
           SELECT id FROM {table} WHERE org_id = $1 AND NOT search_indexed \
           ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED) \
         UPDATE {table} t SET search_indexed = true FROM batch \
         WHERE t.org_id = $1 AND t.id = batch.id"
    ))
    .bind(org)
    .bind(limit)
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Before every search: JIT off for the rest of the transaction (the search
/// statements' generic plans carry cost estimates far above
/// `jit_above_cost`, since the planner cannot see how few index rows a
/// lookup reads, and compiling them takes longer than running them), and
/// one batch of the org's backlog indexed.
pub(crate) async fn prepare(tx: &mut Tx<'_>, kind: Indexed) -> Result<()> {
    sqlx::query("SET LOCAL jit = off")
        .execute(tx.conn())
        .await?;
    index_backlog(tx, kind, kind.batch()).await?;
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
