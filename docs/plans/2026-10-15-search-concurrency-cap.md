# Search concurrency cap Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Cap concurrent searches at half the database pool, process-wide, with a permit taken
before the search's transaction opens and a 500 ms wait that ends in a retriable `unavailable`.

**Architecture:** A `SearchLimiter` (an `Arc<Semaphore>`) in `cxp_core::search`; the search trait
methods require a `&SearchPermit`; `cxp-server` builds one limiter from `CXP_DB_MAX_CONNECTIONS`
and hands it to `cxp-api`'s `AppState` and `cxp-mcp`'s `Cxp`, whose search entry points acquire
before `begin_live`. No migration, no config key.

**Tech Stack:** Rust (tokio semaphore, axum, rmcp, sqlx 0.8), Postgres 16.

**Ticket:** savvagent/otto-cxp#42

**Spec:** `docs/specs/2026-10-15-search-concurrency-cap-design.md`

## Global Constraints

- Worktree only: `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-42-search-concurrency-cap`;
  trunk changes only through the PR. Every Bash call starts a fresh shell: run commands as
  `cd <worktree> && ...`.
- Tests use this run's own container: `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp`
  (never :15435, never a Fly database).
- Gates:
  - `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - `podman build -t otto-cxp:check-issue41 .` (orchestrator, once before the PR)
- Evidence: report pass counts per test binary; a `cargo test` panic about `DATABASE_URL` is a
  missing variable, not a pass. Skipped tests are not a pass.
- Invariants: no migration; no SQL outside `cxp-core`, runtime sqlx only; every route and tool
  keeps its scope check first (an unauthorized caller never takes a permit); request paths keep
  `begin_live`; log lines never carry the search query (customer data); no new config key (the
  permit count derives from `CXP_DB_MAX_CONNECTIONS`).
- Commit format: `<type>(<scope>): <imperative, lower-case>`, body explains why and ends
  `Refs #42`. No attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers).
- Out-of-band: none.
- The record-as-shipped commit (spec `**Status:**`, these boxes) is the orchestrator's.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-core/src/search.rs` | `SearchLimiter`, `SearchPermit`, `SEARCH_WAIT`, unit tests |
| **Modify.** `crates/cxp-core/Cargo.toml` | tokio dev-dependency gains `test-util` |
| **Modify.** `crates/cxp-core/src/error.rs` | `Error::Unavailable` doc names the new cause |
| **Modify.** `crates/cxp-core/src/customers.rs`, `knowledge.rs` | search methods take `&SearchPermit` |
| **Modify.** `crates/cxp-core/tests/common/mod.rs`, `customers.rs`, `support.rs`, `search_backlog.rs` | pass a permit |
| **Modify.** `crates/cxp-api/src/lib.rs`, `routes/customers.rs`, `error.rs` (doc table) | `AppState.search`, acquire before `begin` |
| **Modify.** `crates/cxp-api/tests/api.rs`, `tests/console.rs` | harness passes a limiter; flood, control and refusal tests |
| **Modify.** `crates/cxp-mcp/src/server.rs`, `lib.rs`, `tools/cdp.rs`, `tools/support.rs` | `Cxp.search`, acquire before `tx` |
| **Modify.** `crates/cxp-mcp/tests/mcp.rs` | harness passes a limiter; refusal test |
| **Modify.** `crates/cxp-server/src/lib.rs` | one limiter for both surfaces |
| **Modify.** `docs/deploy/fly.md`, `CLAUDE.md` | the cap |

## Task Order & Rationale

Task 1 adds the limiter on its own (unit-tested, nothing uses it yet). Task 2 threads it through
every crate at once, because the signature changes break every caller together; its tests are
written first against the new API and fail until the wiring lands. Task 3 is docs.

---

### Task 1: `SearchLimiter` in `cxp_core::search`

**Files:**
- Modify: `crates/cxp-core/src/search.rs`
- Modify: `crates/cxp-core/Cargo.toml` (dev-dependency `tokio = { workspace = true, features = ["full", "test-util"] }`)
- Modify: `crates/cxp-core/src/error.rs` (doc comment on `Unavailable` only)

**Interfaces:** produces `cxp_core::search::{SearchLimiter, SearchPermit, SEARCH_WAIT}` used by
Task 2.

**Out-of-band / invariants:** none; the timeout log names no query.

**Model:** sonnet

- [ ] **Step 1: Write the failing unit tests** at the end of `search.rs` (add a
  `#[cfg(test)] mod tests` if there is none; if there is, add to it):

```rust
#[cfg(test)]
mod limiter_tests {
    use super::*;

    #[test]
    fn for_pool_leaves_at_least_half_the_pool_free() {
        for (pool, permits) in [(2, 1), (3, 1), (5, 2), (20, 10)] {
            assert_eq!(SearchLimiter::for_pool(pool).available(), permits, "pool {pool}");
        }
    }

    #[tokio::test]
    async fn a_permit_is_held_until_dropped() {
        let limiter = SearchLimiter::new(2);
        let a = limiter.acquire().await.unwrap();
        let _b = limiter.acquire().await.unwrap();
        assert_eq!(limiter.available(), 0);
        drop(a);
        assert_eq!(limiter.available(), 1);
        limiter.acquire().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_limiter_turns_a_search_away_after_the_wait() {
        let limiter = SearchLimiter::new(1);
        let _held = limiter.acquire().await.unwrap();
        let started = tokio::time::Instant::now();
        let err = limiter.acquire().await.unwrap_err();
        assert!(matches!(err, Error::Unavailable), "{err:?}");
        assert!(err.retriable());
        let waited = started.elapsed();
        assert!(waited >= SEARCH_WAIT && waited < SEARCH_WAIT * 2, "{waited:?}");
    }

    #[tokio::test]
    async fn clones_share_one_set_of_permits() {
        let limiter = SearchLimiter::new(1);
        let other = limiter.clone();
        let _held = limiter.acquire().await.unwrap();
        assert_eq!(other.available(), 0);
    }
}
```

- [ ] **Step 2: Run, see them fail (do not compile)**

Run: `cargo test -p cxp-core --lib --locked limiter_tests`
Expected: compile errors: `SearchLimiter` not found.

- [ ] **Step 3: Implement** in `search.rs` (imports: `std::sync::Arc`, `std::time::Duration`,
  `tokio::sync::{OwnedSemaphorePermit, Semaphore}`):

```rust
/// How long a search waits for a [`SearchPermit`] before it is
/// [`Error::Unavailable`]. A search takes tens of milliseconds, so two
/// permits serve many in this time; the wait holds no connection.
pub const SEARCH_WAIT: Duration = Duration::from_millis(500);

/// The process-wide cap on concurrent searches (#42). A search holds one of
/// the small shared pool's connections for up to its `statement_timeout`
/// twice over, so without a cap a handful of broad searches from one
/// `cxp:read` token could hold every connection while writes, platform
/// webhooks and the usage shipper wait. `cxp-server` builds one per process
/// ([`SearchLimiter::for_pool`]) and gives clones to `/api` and `/mcp`.
///
/// Acquire the permit **before** opening the search's transaction, and hold
/// it until the transaction ends: opening the transaction takes a pooled
/// connection, so a search waiting for a permit inside its transaction
/// would hold that connection while it waits, and the cap would protect
/// nothing.
#[derive(Clone, Debug)]
pub struct SearchLimiter(Arc<Semaphore>);

/// Leave to run one search: what `search_customers` and `search_articles`
/// require. Only [`SearchLimiter::acquire`] makes one; dropping it frees the
/// permit.
#[derive(Debug)]
pub struct SearchPermit(#[allow(dead_code)] OwnedSemaphorePermit);

impl SearchLimiter {
    /// A limiter with `permits` concurrent searches. Panics on 0.
    pub fn new(permits: usize) -> Self {
        assert!(permits > 0, "a search limiter needs at least one permit");
        Self(Arc::new(Semaphore::new(permits)))
    }

    /// Half the pool, at least one: always fewer than `max_connections`
    /// for a pool of two or more (`CXP_DB_MAX_CONNECTIONS` is 2 to 20), so
    /// searches can never hold every connection.
    pub fn for_pool(max_connections: u32) -> Self {
        Self::new((max_connections as usize / 2).max(1))
    }

    /// A permit, waiting up to [`SEARCH_WAIT`] for one; past that,
    /// [`Error::Unavailable`] (retriable: REST 503 with `Retry-After`).
    pub async fn acquire(&self) -> Result<SearchPermit> {
        match tokio::time::timeout(SEARCH_WAIT, self.0.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(SearchPermit(permit)),
            // The semaphore is never closed; read a closed one as busy.
            Ok(Err(_)) | Err(_) => {
                tracing::warn!("a search found every search permit taken and was turned away");
                Err(Error::Unavailable)
            }
        }
    }

    /// Permits free right now.
    pub fn available(&self) -> usize {
        self.0.available_permits()
    }
}
```

If clippy rejects the `#[allow(dead_code)]` field attribute style, name the field `_permit` in a
braced struct instead (`pub struct SearchPermit { _permit: OwnedSemaphorePermit }`).

Then extend the `Error::Unavailable` doc comment in `error.rs` with: "Also raised when a search
finds every search permit taken for longer than `search::SEARCH_WAIT` (#42)."

- [ ] **Step 4: Run, see them pass**

Run: `cargo test -p cxp-core --lib --locked limiter_tests` — expected 4 passed.

- [ ] **Step 5: Gates** (all four from Global Constraints; the workspace test needs
  `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp`).

- [ ] **Step 6: Commit**

```bash
git add crates/cxp-core/src/search.rs crates/cxp-core/src/error.rs crates/cxp-core/Cargo.toml Cargo.lock
git commit -m "feat(core): add a process-wide search limiter" -m "A handful of broad searches from one cxp:read token could hold every
pooled connection for seconds. SearchLimiter caps concurrent searches at
half the pool and turns a search away as unavailable after a short wait
instead of queueing it.

Refs #42"
```

(`Cargo.lock` only if it changed; it should not.)

---

### Task 2: Require a permit for every search, taken before the transaction

**Files:**
- Modify: `crates/cxp-core/src/customers.rs` (`CustomersExt::search_customers` declaration and impl)
- Modify: `crates/cxp-core/src/knowledge.rs` (`KnowledgeExt::search_articles` declaration and impl)
- Modify: `crates/cxp-core/tests/common/mod.rs`, `tests/customers.rs`, `tests/support.rs`, `tests/search_backlog.rs`
- Modify: `crates/cxp-api/src/lib.rs`, `crates/cxp-api/src/routes/customers.rs`, `crates/cxp-api/src/error.rs` (module doc table only)
- Modify: `crates/cxp-api/tests/api.rs`, `crates/cxp-api/tests/console.rs`
- Modify: `crates/cxp-mcp/src/server.rs`, `crates/cxp-mcp/src/lib.rs`, `crates/cxp-mcp/src/tools/cdp.rs`, `crates/cxp-mcp/src/tools/support.rs`
- Modify: `crates/cxp-mcp/tests/mcp.rs`
- Modify: `crates/cxp-server/src/lib.rs` (and `crates/cxp-server/tests/e2e.rs` only if it constructs `AppState`/`cxp_mcp::router` directly)

**Interfaces:** consumes Task 1's `SearchLimiter`/`SearchPermit`. Signatures after this task:

```rust
// cxp-core
fn search_customers(&mut self, permit: &SearchPermit, query: &str, page: Page)
    -> impl Future<Output = Result<SearchPage<Customer>>> + Send;   // match the existing style
fn search_articles(&mut self, permit: &SearchPermit, query: &str)
    -> ...;                                                        // likewise
// cxp-api
pub struct AppState { ..., pub search: SearchLimiter }
pub fn new(db: Db, platform: Arc<PlatformClient>, meter: Arc<Meter>, search: SearchLimiter, config: Config) -> Self
// cxp-mcp
pub fn router(db: Db, platform: Arc<PlatformClient>, meter: Arc<Meter>, search: SearchLimiter, config: Config) -> Result<Router, NoAllowedHosts>
impl Cxp { pub fn new(db: Db, platform: Arc<PlatformClient>, meter: Arc<Meter>, search: SearchLimiter) -> Self }
```

**Out-of-band / invariants:** the scope check (`require_read`) stays the first thing each search
entry point does; the permit is acquired next, then the transaction; `begin_live` unchanged; no
SQL outside `cxp-core`.

**Model:** sonnet

- [ ] **Step 1: Write the failing tests**

(a) `crates/cxp-api/tests/api.rs`: give `harness_opts` a `search: SearchLimiter` parameter (keep
`harness`/`harness_with` passing `SearchLimiter::for_pool(5)`), add `search: SearchLimiter` to
`Harness`, and add:

```rust
/// A pool sized as production's (5) with an acquire timeout well under the
/// searches' 2 s statement_timeout.
async fn pool_of_five(opts: PgPoolOptions, connect: PgConnectOptions) -> PgPool {
    opts.max_connections(5)
        .acquire_timeout(std::time::Duration::from_millis(500))
        .connect_with(connect)
        .await
        .unwrap()
}

/// Fires `n` customer searches that block on `locked`'s lock until their
/// statement_timeout, waits until `limiter` has no permit left (or 1 s),
/// then sends a ticket write for `customer` and returns its status, with
/// the searches' statuses once they finish.
async fn write_during_a_search_flood(
    h: &Harness, token: &str, customer: &str, n: usize,
) -> (StatusCode, Vec<StatusCode>) { /* see below */ }
```

`write_during_a_search_flood`: spawn `n` tasks each calling
`h.call("GET", "/api/v1/customers/search?q=grace", Some(token), None)` (the Harness must be
`Clone` or wrapped in `Arc`; make `Harness` derive `Clone` if all fields allow, else share via
`Arc<Harness>`); poll `h.search.available() == 0` every 10 ms for up to 1 s; then send
`POST /api/v1/tickets` with `{"customerId": customer, "subject": "flood"}` and record its status;
then join the searches.

Tests:

```rust
/// #42: searches can hold at most half the pool, so a write during a flood
/// of searches that each block for seconds still gets a connection. The
/// lock makes every customer search wait until its 2 s statement_timeout.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_search_flood_cannot_starve_a_write(opts: PgPoolOptions, connect: PgConnectOptions) {
    let pool = pool_of_five(opts, connect.clone()).await;
    let h = harness_opts(pool, SECRET, false, SearchLimiter::for_pool(5)).await;
    let (_, _, t) = h.caller(RW);
    let customer = create a customer via POST /api/v1/customers and read its id;
    // A connection outside the app's pool holds the lock.
    let mut locker = sqlx::PgConnection::connect_with(&connect).await.unwrap();
    sqlx::query("BEGIN").execute(&mut locker).await.unwrap();
    sqlx::query("LOCK TABLE customer_search_grams IN ACCESS EXCLUSIVE MODE")
        .execute(&mut locker).await.unwrap();
    let (write, searches) = write_during_a_search_flood(&h, &t, &customer, 8).await;
    assert_eq!(write, StatusCode::CREATED, "the write was starved");
    assert!(searches.iter().all(|s| *s == StatusCode::SERVICE_UNAVAILABLE
                                    || *s == StatusCode::UNPROCESSABLE_ENTITY), "{searches:?}");
    assert!(searches.contains(&StatusCode::SERVICE_UNAVAILABLE), "nothing was turned away");
    sqlx::query("ROLLBACK").execute(&mut locker).await.unwrap();
}

/// The control: with a limiter as large as the pool, the same flood does
/// starve the write, so the test above can tell the difference.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn without_the_cap_a_search_flood_starves_a_write(opts: PgPoolOptions, connect: PgConnectOptions) {
    // same set-up with SearchLimiter::new(5) and 8 searches
    assert!(!write.is_success(), "the write got through without the cap: {write}");
}

/// A search that cannot get a permit answers 503 unavailable with
/// Retry-After, and never reaches the database.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_search_without_a_permit_is_unavailable(pool: PgPool) {
    let limiter = SearchLimiter::new(1);
    let h = harness_opts(pool, SECRET, false, limiter.clone()).await;
    let (_, _, t) = h.caller(RO);
    let _held = limiter.acquire().await.unwrap();
    let r = h.call("GET", "/api/v1/customers/search?q=grace", Some(&t), None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.body["error"], "unavailable", "{}", r.body);
    assert!(r.headers.contains_key("retry-after"));
}
```

Check the customer-create route's JSON field names (`email`, `name`) and the ticket route's
(`customerId`, `subject`) against `crates/cxp-api/src/routes` before writing them; the wire is
camelCase. Adjust the error-body field (`error` vs `code`) to what `ApiError` serializes (read
`crates/cxp-api/src/error.rs`).

(b) `crates/cxp-mcp/tests/mcp.rs`: give the harness a limiter (default `SearchLimiter::for_pool(5)`;
a `harness_search(pool, limiter)` variant) and add:

```rust
/// #42: a search tool that cannot get a permit fails as retriable
/// `unavailable` rather than waiting on the pool.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_search_tool_without_a_permit_is_unavailable(pool: PgPool) {
    let limiter = SearchLimiter::new(1);
    let h = harness_search(pool, limiter.clone()).await;
    let t = token(&h.platform, Uuid::new_v4(), READ);
    let _held = limiter.acquire().await.unwrap();
    for (tool, args) in [("search_customers", json!({"query": "grace"})),
                         ("search_knowledge_base", json!({"query": "reset"}))] {
        let failed = h.tool(&t, tool, args).await.unwrap_err();
        assert_eq!(code(&failed), "unavailable", "{tool}: {failed}");
        assert_eq!(failed["data"]["retriable"], true, "{tool}: {failed}");
    }
}
```

(Check how existing tests read `retriable` from a tool error and match that shape.)

- [ ] **Step 2: Run, see them fail (do not compile)**

Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test -p cxp-api --test api --locked search`
Expected: compile errors (`harness_opts` arity, `SearchLimiter` unused by `AppState`).

- [ ] **Step 3: Implement**

1. `cxp-core`: add `permit: &SearchPermit` as the first parameter after `self` to
   `search_customers` and `search_articles` (trait declarations, doc comments: "`permit`: see
   [`SearchLimiter`]; acquire it before opening this transaction", and impls; the impls do not
   otherwise use it — name it `_permit` in the impl if clippy complains).
2. `cxp-core` tests: add to `tests/common/mod.rs`:
   ```rust
   /// A search permit from a limiter of this test's own (the server shares
   /// one per process; tests run in parallel and must not compete).
   pub async fn permit() -> cxp_core::search::SearchPermit {
       cxp_core::search::SearchLimiter::new(1).acquire().await.unwrap()
   }
   ```
   and pass `&common::permit().await` (or one permit bound per test) at every
   `.search_customers(` / `.search_articles(` call in `common/mod.rs`, `customers.rs`,
   `support.rs`, `search_backlog.rs`. Change no assertion.
3. `cxp-api`: `AppState` gains `pub search: SearchLimiter` and the `new` parameter (after `meter`);
   update the unit test at `lib.rs` (~line 300) and `tests/console.rs`'s `AppState::new` to pass
   `SearchLimiter::for_pool(5)`. In `routes/customers.rs` `search`: after `caller.require_read()?`,
   `let permit = state.search.acquire().await?;` then `begin`, then
   `tx.search_customers(&permit, &q.q, page)`. In `error.rs`'s module doc table, the `unavailable`
   row adds "or a search finding every search permit taken (#42)".
4. `cxp-mcp`: `Cxp` gains `search: SearchLimiter` (and the `new` parameter); `router` gains the
   parameter and passes it on. In `tools/cdp.rs` `search_customers` and `tools/support.rs`
   `search_knowledge_base`: after `require_read`, `let permit = self.search.acquire().await.mcp()?;`
   then `self.tx(...)`, then the search with `&permit`. Add a `pub(crate) fn search(&self) ->
   &SearchLimiter` accessor or access the field directly, matching how tools reach `self.db`.
5. `cxp-server/src/lib.rs` `router`: `let search = cxp_core::search::SearchLimiter::for_pool(config.db_max_connections);`
   pass `search.clone()` to `cxp_mcp::router` and `search` to `AppState::new`, with a comment:
   one limiter for the whole process, so `/api` and `/mcp` together hold at most half the pool.

- [ ] **Step 4: Run, see them pass**

Run:
- `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test -p cxp-api --test api --locked search`
  — the three new tests pass (the flood tests take a few seconds each).
- `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test -p cxp-mcp --test mcp --locked permit`
  — the new test passes.
If `without_the_cap_a_search_flood_starves_a_write` does not fail the write (the write got
through), report DONE_WITH_CONCERNS with what happened: the flood test would not prove anything.

- [ ] **Step 5: Gates** (all four).

- [ ] **Step 6: Commit**

```bash
git add -A crates
git commit -m "feat(api): take a search permit before the search's transaction" -m "Searches now need a SearchPermit, which /api and /mcp take from one
process-wide limiter before opening the transaction, so waiting for a
permit holds no connection and searches together hold at most half the
pool. A flood of blocked searches no longer starves a concurrent write;
a search that cannot get a permit within 500 ms is unavailable (503 with
Retry-After on REST, retriable on MCP).

Refs #42"
```

---

### Task 3: Docs

**Files:**
- Modify: `docs/deploy/fly.md` (the pool sentence in the database bullet, ~line 57)
- Modify: `CLAUDE.md` ("### Search index (#17)" section)

**Model:** haiku

- [ ] **Step 1:** In `docs/deploy/fly.md`, after "The pool is capped at 5 connections
  (`CXP_DB_MAX_CONNECTIONS`) because the instance is small and shared.", add: "Searches hold at
  most half of it (two of five) at once, process-wide; a search that cannot start within 500 ms
  answers `unavailable` (503 with `Retry-After`) ([#42](https://github.com/savvagent/otto-cxp/issues/42))."
- [ ] **Step 2:** In CLAUDE.md's "### Search index (#17)" paragraph, append: "A search needs a
  `SearchPermit` from the process's one `SearchLimiter` (half the pool), taken before its
  transaction opens (#42)."
- [ ] **Step 3: Commit**

```bash
git add docs/deploy/fly.md CLAUDE.md
git commit -m "docs: note the search concurrency cap" -m "Refs #42"
```
