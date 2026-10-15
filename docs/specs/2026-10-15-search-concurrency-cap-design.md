# Search concurrency cap — design

**Status:** DRAFT
**Issue:** #42 "Cap concurrent searches below the database pool size"
**Plan:** `docs/plans/2026-10-15-search-concurrency-cap.md`
**Builds on:** `docs/specs/2026-10-15-search-index-upkeep-design.md` (#34),
`docs/specs/2026-10-15-search-per-candidate-plans-design.md` (#41)

## Brief

> A single token with only `cxp:read` can occupy every pooled database connection.
> `CXP_DB_MAX_CONNECTIONS` is 5. Five broad searches sent in parallel each run under
> `statement_timeout = 2s`, and each search runs two statements (the hits query, then the row
> fetch). So each holds its connection for up to about 4 s. Nothing limits how many searches run
> at once, per org or across the process. While they run, writes, platform webhooks and the usage
> shipper wait for a connection.
>
> PR #40 (#34) took away retry amplification, because `query_too_broad` is now a non-retriable
> 422. It did nothing about a deliberate flood. Found by the PR #40 security re-audit (OWASP API4,
> Unrestricted Resource Consumption).

Acceptance criteria (#42):

- [ ] A process-wide limit on concurrent searches, below `CXP_DB_MAX_CONNECTIONS`, for example a
  `tokio::sync::Semaphore` with 2 permits. It is held around `search::prepare` and both search
  statements.
- [ ] A search that cannot get a permit within a short wait returns `unavailable` (retriable,
  REST 503 with `Retry-After`) and does not queue indefinitely.
- [ ] A test shows a flood of slow searches cannot starve a concurrent write of a connection.

Out of scope (#42): per-caller rate limiting across all tools.

## Premise corrections

- **Holding the permit only "around `search::prepare` and both statements" would not protect the
  pool.** Every search runs inside a tenant transaction that the route or tool opens first
  (`AppState::begin` in `cxp-api`, `Cxp::tx` in `cxp-mcp`, both `platform_events::begin_live`),
  and opening it takes a pooled connection. A permit taken inside `search_customers` would be
  waited for while holding that connection: five flooding searches would still hold all five
  connections, two running and three waiting. So the permit is taken **before** the transaction
  opens and released after it ends. It still covers `prepare` and both statements (it covers the
  whole transaction), as the criterion asks.
- **The row fetch is not a second `statement_timeout` window in practice.** It reads at most a
  page of rows by primary key. The worst case is still about 2 s per search plus the fetch, which
  doesn't change the design.
- **Only three entry points run a search:** `GET /api/v1/customers/search`
  (`cxp-api/src/routes/customers.rs`, `search`), and the MCP tools `search_customers`
  (`cxp-mcp/src/tools/cdp.rs`) and `search_knowledge_base` (`cxp-mcp/src/tools/support.rs`).
  `resolve_customer` does exact lookups (id, email, name), not a search, and is not limited.

## Assumptions

- **No Development Workflow key was missing**; nothing is assumed there.
- **The permit count is derived from the pool size, with no new config key:**
  `permits = max(1, CXP_DB_MAX_CONNECTIONS / 2)`. That is 2 at the default 5, as the issue
  suggests. It is always below the pool size (the config allows 2 to 20), and at least half the
  pool stays free for everything else. A config key would be an out-of-band step for no clear
  gain.
- **The wait is 500 ms.** A search takes tens of milliseconds, so two permits serve many searches
  in that time, and a caller that is turned away hears so quickly. The wait holds no connection
  (the permit comes before the transaction), so a waiting search costs a parked task and nothing
  else. A flood is bounded by the wait: each request waits at most 500 ms, then gets `unavailable`.
- **`Error::Unavailable` is the right error.** Its code is `unavailable`, it is retriable, REST
  already maps it to 503 with `Retry-After` (`cxp_core::auth::RETRY_AFTER_SECONDS`), and MCP
  already marks it retriable. Its message ("could not complete this right now; nothing was
  changed. Retry shortly") fits. Its doc comment gains the new cause.
- **One limiter per process, shared by `/api` and `/mcp`.** `cxp-server` builds one and hands
  clones (an `Arc` inside) to both surfaces. A static would also be process-wide, but each test
  binary runs tests in parallel in one process, so a static would make unrelated tests compete for
  two permits and flake. An instance lets each test build its own.

## Goal & Success Criteria

No number of concurrent searches can hold more than half of the database pool. A search that
cannot start soon is turned away with a retriable `unavailable`, without holding a connection
while it waits.

- At most `max(1, pool / 2)` searches hold a connection at once, across `/api` and `/mcp`.
- A search waits at most 500 ms for a permit, holding no connection, then fails with
  `unavailable`: REST 503 with `Retry-After`, MCP retriable `unavailable`.
- A test floods searches that block for seconds, at more than the pool size. A write issued
  during the flood gets a connection and succeeds well within the pool's acquire timeout. With the
  limiter bypassed, the same write cannot get a connection.
- The search methods cannot be called without a permit (the type system enforces it).

## Scope

In:

- `cxp_core::search`: `SearchLimiter`, `SearchPermit`, `SEARCH_WAIT`, and `SearchLimiter::for_pool`.
- `CustomersExt::search_customers` and `KnowledgeExt::search_articles` take `&SearchPermit` as
  their first argument after `self`.
- `cxp-api`: `AppState` gains `search: SearchLimiter`. `AppState::new` takes it, and the search
  route acquires a permit before `begin`.
- `cxp-mcp`: `Cxp` and `cxp_mcp::router` take a `SearchLimiter`, and both search tools acquire
  before `tx`.
- `cxp-server`: builds one `SearchLimiter::for_pool(config.db_max_connections)` and passes it to
  both.
- Tests: a unit test of the limiter, a flood test, and updated call sites of the search methods in
  the existing tests.
- Docs: `Error::Unavailable`'s doc comment, the `cxp-api` error table, `docs/deploy/fly.md`'s pool
  note if it mentions search, and one CLAUDE.md line.

Out:

- Per-caller or per-org rate limits (#42 out of scope). A single caller can still use every
  search permit. The limit protects the pool, not fairness between callers.
- Any new config key, migration, scope or secret.
- Changing the search statements, the caps or `statement_timeout`.

## Architecture

```rust
// crates/cxp-core/src/search.rs
/// How long a search waits for a permit before it is `Error::Unavailable`.
pub const SEARCH_WAIT: Duration = Duration::from_millis(500);

/// The process-wide cap on concurrent searches (#42). Clone shares it.
#[derive(Clone, Debug)]
pub struct SearchLimiter(Arc<Semaphore>);

/// Proof that a search may run; held for the search's whole transaction.
#[derive(Debug)]
pub struct SearchPermit(OwnedSemaphorePermit);   // private field: only `acquire` makes one

impl SearchLimiter {
    pub fn new(permits: usize) -> Self;              // panics on 0
    pub fn for_pool(max_connections: u32) -> Self;   // max(1, max_connections / 2) permits
    pub async fn acquire(&self) -> Result<SearchPermit>;  // waits SEARCH_WAIT, else Error::Unavailable
    pub fn available(&self) -> usize;                // for tests and logs
}
```

`acquire` is `tokio::time::timeout(SEARCH_WAIT, sem.clone().acquire_owned())`. A timeout logs at
`warn` with no query text and returns `Error::Unavailable`. The semaphore is never closed, so
`acquire_owned` cannot fail otherwise; if it ever did, that is also `Unavailable`.

Call sites, all in the same order (permit, then transaction):

```rust
// cxp-api routes/customers.rs search
let permit = state.search.acquire().await?;
let mut tx = state.begin(&caller).await?;
let found = tx.search_customers(&permit, &q.q, page).await?;
tx.commit().await?;
drop(permit); // implicit at scope end, after the transaction is done
```

MCP's `search_customers` and `search_knowledge_base` follow the same order, with `.mcp()?` on the
acquire. The scope check (`require_read`) stays first, so an unauthorized caller never takes a
permit.

The trait methods take `permit: &SearchPermit` and don't use it beyond its existence. The
parameter documents and enforces that a caller holds one. The doc on `SearchLimiter::acquire`
says to acquire it before opening the transaction, and why.

Public-interface changes:

- `cxp_core` `CustomersExt::search_customers`, `KnowledgeExt::search_articles`: breaking for
  in-repo callers only (a new parameter). No external crate uses `cxp-core`.
- `cxp_api::AppState::new`, `cxp_mcp::router`, `cxp_mcp::Cxp::new`: a new parameter, in-repo only.
- REST and MCP wire: unchanged. `unavailable` and 503 with `Retry-After` already exist. A search
  can now return them.

## Invariants impact

- **The database is small and shared:** strengthened. Searches can hold at most half the pool.
- **Request paths open transactions with `begin_live`:** unchanged; the permit comes before it.
- **No SQL outside `cxp-core`:** no SQL added.
- **Auth:** unchanged; the scope check precedes the permit.
- **Untrusted text:** the timeout log carries the search name only, never the query.
- **Metering:** unchanged (searches don't meter).

## Migration

None.

## Out-of-band

None. There is no config key (the permit count derives from `CXP_DB_MAX_CONNECTIONS`), no secret,
scope, migration or role change, and no recurring cost.

## Error Handling & Edge Cases

- Permit wait times out: `Error::Unavailable`, which is 503 with `Retry-After: 5` on REST and a
  retriable `unavailable` on MCP. Nothing was read or changed.
- A search fails or times out (`query_too_broad`) while holding a permit: the permit drops with
  the handler's scope (RAII), on every path, panics and client disconnects included (axum drops
  the future).
- `CXP_DB_MAX_CONNECTIONS = 2`: one permit; `3`: one; `5`: two; `20`: ten.
- The REST route rejects an empty or long query only inside `search_customers`, after the permit.
  Validation is cheap, and the permit is released at once, so no reordering is needed.

## Testing Approach

Gate: `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test --workspace --locked`
(this run's own container), plus clippy, fmt, the migrations check, and `podman build`.

- `cxp-core` unit tests (`search.rs`):
  - `for_pool` gives 2, 1, 1, 10 for 5, 2, 3, 20.
  - `acquire` succeeds while permits remain.
  - With every permit held, `acquire` returns `Error::Unavailable` after about `SEARCH_WAIT`, not
    before it and not much after (paused tokio time).
  - Dropping a permit frees it.
- `cxp-api` `tests/api.rs`:
  - `a_search_flood_cannot_starve_a_write`: a harness whose pool has 5 connections and a short
    acquire timeout, and a limiter `for_pool(5)`. One connection holds an `ACCESS EXCLUSIVE` lock
    on `customer_search_grams`, so every customer search blocks until its 2 s
    `statement_timeout`. The test fires 8 concurrent `GET /api/v1/customers/search` requests,
    waits until the 2 permits are taken, then issues a write that doesn't touch the locked table
    and asserts it succeeds while the searches are still blocked. The turned-away searches
    answer 503 with `Retry-After`, and the blocked ones `query_too_broad`.
  - A control in the same test file: the same flood against a limiter as large as the pool shows
    the write's connection acquire timing out. This proves the test can tell the difference.
  - A REST mapping check: the search route answers 503 `unavailable` with `Retry-After` when no
    permit is free.
- `cxp-mcp`: an existing tool test (or a new one) shows `search_customers` returns the retriable
  `unavailable` when no permit is free.
- The existing search tests pass a permit from a per-test limiter (`common::permit()`).

## Risks & Open Questions

- **One flooding caller can still hold every search permit,** so other callers' searches get
  `unavailable` for the duration. That is accepted: the limit protects the pool, not fairness.
  Per-caller limits are out of scope (#42).
- **The 500 ms wait and half-the-pool split are judgement calls.** Both are constants in
  `search.rs` and easy to tune.
- **A future search entry point could open its transaction before acquiring the permit.** The
  `&SearchPermit` parameter forces a permit but not the order. The doc on `acquire` and the flood
  test (for REST) guard it. MCP follows the same order, and its test checks the refusal path.
