//! What customer and knowledge-base search share (#17): the capped result,
//! the bounds every search stays within, and the timeout that backs them.
//!
//! There is no backlog of unindexed rows (#34): every row is indexed by its
//! table's triggers. `search_mark_indexed` (migration 0004) sets
//! `search_indexed` on every insert and update, and the sync triggers write
//! the row's index rows with it, so no row can be written unindexed. The
//! column and its partial indexes remain only because migrations are
//! additive.
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

/// Before every search, for the rest of the transaction: JIT off (the
/// search statements' generic plans carry cost estimates far above
/// `jit_above_cost`, since the planner cannot see how few index rows a
/// lookup reads, and compiling them takes longer than running them) and
/// [`STATEMENT_TIMEOUT`]. A search statement past the timeout is
/// [`Error::QueryTooBroad`] ([`on_timeout`]).
pub(crate) async fn prepare(tx: &mut Tx<'_>) -> Result<()> {
    sqlx::query("SET LOCAL jit = off")
        .execute(tx.conn())
        .await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
    ))
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A search statement's failure: past [`STATEMENT_TIMEOUT`] (SQLSTATE
/// 57014) it is [`Error::QueryTooBroad`], logged at `warn` with the search
/// it was (never the query, which may be customer data); anything else is
/// a database error. An operator's `pg_cancel_backend` raises 57014 too and
/// reads the same way, which is harmless: retrying it is right.
pub(crate) fn on_timeout(search: &'static str) -> impl FnOnce(sqlx::Error) -> Error {
    move |e| {
        let timed_out =
            matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("57014"));
        if timed_out {
            tracing::warn!(search, "search hit its statement_timeout");
            Error::QueryTooBroad
        } else {
            Error::Database(e)
        }
    }
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
