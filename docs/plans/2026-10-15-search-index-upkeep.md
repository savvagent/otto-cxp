# Search index upkeep Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [x]`) syntax for tracking.

**Goal:** Retire the lazy search backlog, give a search timeout its own error code, tighten the
search statements, cap article content, and document when the index's storage needs action.

**Architecture:** All SQL stays in `cxp-core` (`search.rs`, `customers.rs`, `knowledge.rs`); a new
`Error::QueryTooBroad` variant flows to REST and MCP through the existing mappings. No migration.

**Tech Stack:** Rust (sqlx runtime queries, axum, rmcp), Postgres 16 with pg_trgm.

**Ticket:** savvagent/otto-cxp#34

**Spec:** `docs/specs/2026-10-15-search-index-upkeep-design.md`

## Global Constraints

- Worktree only: `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-34-search-index-upkeep`;
  trunk changes only through the PR.
- Tests use the dedicated container: `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp`
  (never :15435, never a Fly database).
- Gates:
  - `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - `podman build -t otto-cxp:check .` (crates changed; run once before the PR)
- Evidence: report pass counts per test binary; a `cargo test` panic about `DATABASE_URL` is a
  missing variable, not a pass. Skipped tests are not a pass.
- Invariants: no migration and no edit to any migration; no SQL outside `cxp-core`, runtime
  `sqlx::query`/`query_as` only; database text never in a response or log line (log SQLSTATE or
  fixed text only); every read bounded; request paths keep `begin_live`.
- Commit format: `<type>(<scope>): <imperative, lower-case>`, body explains why and ends
  `Refs #34`. No attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers).
- Out-of-band: none (no config, secret, scope, migration, or role change).

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-core/src/error.rs` | `Error::QueryTooBroad`, code/retriable/internal, unit tests |
| **Modify.** `crates/cxp-core/src/search.rs` | drop backlog; `prepare(tx)`; `on_timeout` |
| **Modify.** `crates/cxp-core/src/customers.rs` | statement without backlog; `fp` coalesce; CASE `capped`; timeout mapping |
| **Modify.** `crates/cxp-core/src/knowledge.rs` | statement without backlog; timeout mapping; `MAX_CONTENT_CHARS` |
| **Modify.** `crates/cxp-api/src/error.rs` | 503 for `QueryTooBroad`; doc table; test |
| **Modify.** `crates/cxp-mcp/src/error.rs` | test case for the new code |
| **Modify.** `crates/cxp-core/tests/customers.rs` | drop backlog tests; trigger and timeout tests; uncapped ratio |
| **Modify.** `crates/cxp-core/tests/support.rs` | uncapped ratio; content limit; article timeout test |
| **Modify.** `crates/cxp-core/tests/search_backlog.rs` | least-privilege test without `index_backlog` |
| **Modify.** `docs/deploy/fly.md`, `CLAUDE.md` | storage watch section; search section |

## Task Order & Rationale

Task 1 adds the error variant (other tasks depend on it). Task 2 is the core change (backlog
removal, statement tweaks, timeout mapping) with its tests. Task 3 is the independent content
limit. Task 4 is docs. Each task leaves the workspace green.

---

### Task 1: `Error::QueryTooBroad` on every surface

**Model:** sonnet

**Files:** `crates/cxp-core/src/error.rs`, `crates/cxp-api/src/error.rs`, `crates/cxp-mcp/src/error.rs`

- [x] **Step 1: failing tests.**
  - `crates/cxp-core/src/error.rs` `mod tests`: add `(Error::QueryTooBroad, "query_too_broad")`
    to `every_variant_has_its_wire_code`; add `Error::QueryTooBroad` to the `yes` list of
    `retriable_marks_what_an_agent_may_simply_retry`; add a test
    `a_search_past_its_timeout_reads_as_too_broad` asserting `!is_internal()`, and that
    `to_string()` contains "narrow the query" and "retry".
  - `crates/cxp-api/src/error.rs` tests: add `(Error::QueryTooBroad,
    StatusCode::SERVICE_UNAVAILABLE)` to `every_domain_code_has_its_status`; in
    `unavailable_tells_the_client_when_to_retry` assert `ApiError::from(Error::QueryTooBroad)
    .into_response()` carries `Retry-After` and its JSON `code` is `query_too_broad` (status
    503 is enough if reading the body is awkward).
  - `crates/cxp-mcp/src/error.rs` tests: add `Error::QueryTooBroad` to the table that asserts
    `data.code == e.code()` (around line 140), expecting rpc `INVALID_PARAMS` and
    `retriable: true` if that table checks them.
- [x] **Step 2:** `cargo test -p cxp-core --lib error && cargo test -p cxp-api --lib error &&
  cargo test -p cxp-mcp --lib error` — fails to compile (no variant).
- [x] **Step 3: implement.** In `cxp_core::Error`, after `Unavailable`:
  ```rust
  /// A search ran past its `statement_timeout` (SQLSTATE 57014): too broad
  /// for this org, or the first search on a cold cache. A narrower query or
  /// a plain retry can succeed.
  #[error(
      "this search ran longer than otto-cxp allows; narrow the query (more characters \
       or more specific words), or retry"
  )]
  QueryTooBroad,
  ```
  `code()`: `Error::QueryTooBroad => "query_too_broad"`; `retriable()`: add
  `Error::QueryTooBroad => true`; `is_internal()` already false via `_`.
  `cxp-api` `status_for`: `Error::QueryTooBroad => StatusCode::SERVICE_UNAVAILABLE` with a
  comment (retriable, so 503 and `Retry-After`); add the row
  `| \`query_too_broad\` | 503, with \`Retry-After\`: a search ran past its time limit; narrow
  the query or retry |` to the module doc table. `cxp-mcp` `from_core` needs no code change.
- [x] **Step 4:** re-run the Step 2 commands — pass. Then the full gates (test, clippy, fmt).
- [x] **Step 5: commit** `feat(core): give a search timeout its own error code` — body: a
  search past its statement_timeout read as `internal`; agents could not tell a broad query
  from a server fault. `Refs #34`.

### Task 2: retire the backlog; tighten the search statements; map the timeout

**Model:** opus

**Files:** `crates/cxp-core/src/search.rs`, `crates/cxp-core/src/customers.rs`,
`crates/cxp-core/src/knowledge.rs`, `crates/cxp-core/tests/customers.rs`,
`crates/cxp-core/tests/support.rs`, `crates/cxp-core/tests/search_backlog.rs`

- [x] **Step 1: failing tests** (`crates/cxp-core/tests/customers.rs` unless noted).
  - `a_search_past_its_timeout_is_query_too_broad`: create and COMMIT a customer in an org in its
    own transaction first (an uncommitted insert holds a lock on `customer_search_grams` and the
    `LOCK TABLE` below would wait forever); then in a separate
    superuser transaction on the test's raw `pool` run `LOCK TABLE customer_search_grams IN
    ACCESS EXCLUSIVE MODE` and keep it open; in the org's `db.begin(org)` transaction call
    `search_customers("anything", Page::default())`; assert the error's `code()` is
    `query_too_broad`, `retriable()`, and its `to_string()` does not contain "statement" or
    "cancel"; assert the call took at least 1.9 s (so it really waited out the timeout rather
    than failing some other way); then roll back the lock. Take the lock on its own
    connection (`pool.begin()`), in an open transaction, before the search starts. (The search's `SET LOCAL statement_timeout = '2s'` bounds
    the lock wait; the lock is on a table only the search statement reads, so `begin_live`'s
    own statements are unaffected.) Put `SET LOCAL lock_timeout = '5s'` on the lock-holding
    transaction before its `LOCK TABLE`, so a mistake fails fast instead of hanging.
  - `crates/cxp-core/tests/support.rs` `an_article_search_past_its_timeout_is_query_too_broad`:
    same with `knowledge_search_terms` and `search_articles("anything")`.
  - `rows_are_indexed_whatever_the_insert_says`: in an org's transaction, `INSERT INTO customers
    (org_id, email, name, search_indexed) VALUES ($1, 'flag@example.com', 'Flag Person', false)`;
    assert `search_indexed` reads true and `customer_search_grams` has rows for it; then
    `UPDATE customers SET search_indexed = false` and assert it still reads true; and
    `search_customers("flag")` finds it (total 1, not capped). Same for an article insert with
    `published = true, search_indexed = false` and `knowledge_search_terms` rows. The doc
    comment says this is why search needs no backlog (migration 0004's `search_mark_indexed`).
  - Random corpus tests (`random_corpora_search_like_the_scan` in `customers.rs`, and the
    article corpus test in `support.rs` that loops `for i in 0..250` over `articles_both_ways`):
    count `run` (queries that returned `Some`) and `uncapped` (not `found.capped`); after the
    loop `assert!(uncapped * 5 >= run * 4, "{uncapped} of {run} uncapped")`. In
    `customers.rs` count per query on the `Page::default()` page only. Run once first and record
    the actual ratio in a comment; if it is below 80%, stop and report (do not lower the bound).
  - Delete from `customers.rs`: `a_backlog_larger_than_its_read_is_capped_until_indexed`,
    `a_failing_backlog_batch_does_not_fail_search`, `indexing_the_backlog_does_not_block_references_to_it`,
    and the `Logs` struct/impl if nothing else uses it. Keep `insert_untriggered` (still used by
    `a_cut_substring_read_in_a_large_org_is_capped`).
  - `crates/cxp-core/tests/search_backlog.rs`: replace the `index_backlog` import and calls.
    After `cxp_core::migrate`, keep the three zero/180 assertions. Then: org A's search for
    `"grace"` (customers) and `"gadget"` (articles) finds nothing (rows predating 0004 are not
    indexed; the statement no longer reads them); then in `db.begin(a)` run
    `UPDATE customers SET search_indexed = true WHERE org_id = $1` and the same for
    `knowledge_articles`, commit; assert A has 0 unindexed and B still has 30 customers
    unindexed (RLS: A's update cannot reach B); `search_is_exact(&db, a)`; then the same update
    for B and `search_is_exact(&db, b)`; keep the final "everything is indexed" assertions.
    Update the file's module doc: the migrating role sees no rows, so rows predating 0004 stay
    unindexed until an ordinary write (this update, or the superuser rebuild in
    `docs/deploy/fly.md`) indexes them; production had none.
- [x] **Step 2:** `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp cargo test -p cxp-core
  --test customers --test support --test search_backlog` — the timeout tests fail (code is
  `internal`); the search_backlog test fails because search still reads the backlog and finds
  "grace" before the update; the trigger test passes already (it documents existing behaviour).
- [x] **Step 3: implement `search.rs`.** Delete `BACKLOG_READ`, `Indexed` (and its impl),
  `index_backlog`, the savepoint block. `prepare` becomes
  `pub(crate) async fn prepare(tx: &mut Tx<'_>) -> Result<()>` running only `SET LOCAL jit = off`
  and `SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'`; its doc says so and that a search
  past the timeout is `Error::QueryTooBroad` (`on_timeout`). Add:
  ```rust
  /// A search statement's failure: past [`STATEMENT_TIMEOUT`] (SQLSTATE
  /// 57014) it is [`Error::QueryTooBroad`], logged at `warn` with the search
  /// it was (never the query, which may be customer data); anything else is
  /// a database error.
  pub(crate) fn on_timeout(search: &'static str) -> impl FnOnce(sqlx::Error) -> Error {
      move |e| {
          let timed_out = matches!(&e, sqlx::Error::Database(db) if db.code().as_deref() == Some("57014"));
          if timed_out {
              tracing::warn!(search, "search hit its statement_timeout");
              Error::QueryTooBroad
          } else {
              Error::Database(e)
          }
      }
  }
  ```
  Rewrite the module doc's first paragraph: no backlog; every row is indexed by the tables'
  triggers (`search_mark_indexed` sets `search_indexed` on every write, so no row can be
  written unindexed); the column and partial indexes remain only because migrations are
  additive (#34).
- [x] **Step 4: implement `customers.rs`.** In `SEARCH_CUSTOMERS_SQL`: delete the `backlog`
  CTE; delete `UNION ALL SELECT customer_id FROM backlog WHERE customer_id NOT IN (SELECT
  customer_id FROM sub_all)` from `matches`; in `fz_all` delete `AND search_indexed`; replace
  `current_setting('pg_trgm.similarity_threshold')::float8 AS th` with
  `coalesce(nullif(current_setting('pg_trgm.similarity_threshold', true), '')::float8, 0.3) AS th`;
  replace the `capped` expression with
  `CASE WHEN (SELECT count(*) FROM matches) > {cap} THEN true ELSE (<the remaining disjuncts
  except the backlog one>) END AS capped`; remove the `backlog = BACKLOG_READ + 1` format arg and
  the `BACKLOG_READ` import. Doc comment: remove the `backlog` bullet; rewrite the `fp` sentence
  (threshold read with `current_setting(…, true)`, NULL before pg_trgm loads in the session, so
  its 0.3 default; a placeholder set earlier is what pg_trgm adopts); add one sentence that the
  count of the org's customers runs only when the matches have not already capped the result.
  In `search_customers`: `search::prepare(self).await?;` and `.map_err(search::on_timeout
  ("search_customers"))` on both the hits `fetch_one` and the rows `fetch_all` (replace their
  `?` on sqlx errors accordingly).
- [x] **Step 5: implement `knowledge.rs`.** In `SEARCH_ARTICLES_SQL`: delete the `backlog` CTE;
  delete the third `UNION ALL` arm of `matches` (from `UNION ALL SELECT a.id, a.search_tsv @@
  tsq.q,` through `AND (a.search_tsv @@ tsq.q OR a.title ILIKE $3)`), keeping the closing `) AS u`;
  delete `OR (SELECT count(*) FROM backlog) > {BACKLOG_READ}` from `capped`; remove the format
  arg and import. Doc comment: remove the `backlog` bullet. In `search_articles`:
  `search::prepare(self)` and `on_timeout("search_articles")` on both statements.
- [x] **Step 6:** `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp cargo test -p
  cxp-core` — all pass, including the equivalence, threshold, cap, and corpus tests. Then the
  full gates (workspace test, clippy, fmt, migrations check).
- [x] **Step 7: commit** `feat(core): retire lazy search backlog indexing` — body: production had
  no rows predating migration 0004 (SELECT-only check 2026-10-15) and its trigger marks every
  write indexed, so the per-search backlog batch and CTEs only cost time; a timeout now reads
  `query_too_broad`; the threshold falls back to pg_trgm's default instead of relying on CTE
  evaluation order; the 20,001-row count is skipped once matches cap the result. `Refs #34`.

### Task 3: cap article content

**Model:** haiku

**Files:** `crates/cxp-core/src/knowledge.rs`, `crates/cxp-core/tests/support.rs`

- [x] **Step 1: failing test** in `support.rs`, next to `article_titles_are_length_limited` and in
  its style: `article_content_is_length_limited` — create with content of `MAX_CONTENT_CHARS`
  `"é"` characters succeeds; create with one more fails `invalid_argument`; update of that
  article with `content: Some(one more)` fails `invalid_argument` and leaves content unchanged.
- [x] **Step 2:** `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp cargo test -p
  cxp-core --test support article_content` — fails to compile (no constant).
- [x] **Step 3: implement.** In `knowledge.rs` next to `MAX_TITLE_CHARS`:
  ```rust
  /// Longest article content, in characters (about 30 printed pages). Search
  /// indexes one row per distinct lexeme of an article (migration 0004), and
  /// `to_tsvector` runs on every write, so this bounds both.
  pub const MAX_CONTENT_CHARS: usize = 100_000;
  ```
  and `checked_content` mirroring `checked_title` (message "an article's content must be at
  most {MAX_CONTENT_CHARS} characters"); call it in `create_article` and, when `Some`, in
  `update_article`. Update the `KnowledgeExt::create_article` doc to mention both limits.
- [x] **Step 4:** re-run Step 2's command — pass; then the full gates.
- [x] **Step 5: commit** `feat(core): limit knowledge article content length` — body: content
  drives an article's index rows and tsvector cost; bound it before any route or tool exposes
  article writes. `Refs #34`.

### Task 4: docs

**Model:** sonnet

**Files:** `docs/deploy/fly.md`, `CLAUDE.md`

- [x] **Step 1: fly.md.** In the Infrastructure bullet, replace "Searches also index, a batch at a
  time, the rows written before migration 0004. Watching that size, and retiring the backlog path
  once production has none left, is #34." with a pointer to the new section. Add a section
  `## Search index storage (#34)` (after `## Database role` and its runbooks, before the next `##`)
  containing exactly the items in the spec's Docs bullet: threshold (100 MB for the two index
  tables together; `otto_cxp` 250 MB; volume 70%), the actions and who decides; the watch query
  (below) and the 2026-10-15 baseline (search index 56 kB; `otto_cxp` 9.5 MB; 0 customers,
  0 articles); the backlog note with the superuser rebuild SQL; the first-search timeout note;
  the LEAKPROOF re-evaluation trigger, the `ALTER FUNCTION` statement, what loses it, and the
  `proleakproof` boot-check design. Watch query (run via `fly postgres connect -a otto-db -d
  otto_cxp`):
  ```sql
  BEGIN TRANSACTION READ ONLY;
  SELECT s.relname, s.n_live_tup AS live_rows,
         pg_size_pretty(pg_total_relation_size(s.relid)) AS total,
         pg_size_pretty(pg_relation_size(s.relid)) AS heap,
         pg_size_pretty(pg_indexes_size(s.relid)) AS indexes
  FROM pg_stat_user_tables s
  WHERE s.relname IN ('customer_search_grams', 'knowledge_search_terms', 'customers', 'knowledge_articles')
  ORDER BY pg_total_relation_size(s.relid) DESC;
  SELECT i.relname, i.indexrelname, i.idx_scan, pg_size_pretty(pg_relation_size(i.indexrelid)) AS size
  FROM pg_stat_user_indexes i
  WHERE i.relname IN ('customer_search_grams', 'knowledge_search_terms')
     OR i.indexrelname LIKE '%search_backlog%'
  ORDER BY pg_relation_size(i.indexrelid) DESC;
  SELECT pg_size_pretty(sum(pg_total_relation_size(relid))) AS search_index_total
  FROM pg_stat_user_tables WHERE relname IN ('customer_search_grams', 'knowledge_search_terms');
  SELECT datname, pg_size_pretty(pg_database_size(datname)) AS size
  FROM pg_database WHERE NOT datistemplate ORDER BY pg_database_size(datname) DESC;
  SELECT count(*) FILTER (WHERE NOT search_indexed) AS customers_unindexed FROM customers;
  SELECT count(*) FILTER (WHERE NOT search_indexed) AS articles_unindexed FROM knowledge_articles;
  ROLLBACK;
  ```
  Rebuild after a restore with triggers disabled (human, superuser, a write):
  `UPDATE customers SET search_indexed = true WHERE NOT search_indexed;` and the same for
  `knowledge_articles` (the triggers write the index rows).
- [x] **Step 2: CLAUDE.md** "### Search index (#17)": replace "Searches are `cxp:read` yet write:
  each indexes a batch of its org's pre-0004 backlog (`cxp_core::search::index_backlog`)." with:
  every row is indexed by the tables' triggers (`search_mark_indexed` marks every write), so
  there is no backlog and searches only read; a search past its 2 s `statement_timeout` is
  `query_too_broad` (retriable); storage watch and threshold: `docs/deploy/fly.md` ("Search index
  storage"). Also update the `cxp-core` crate bullet's `search` description ("and the per-org
  backlog of rows written before migration 0004, indexed a batch per search") to drop the backlog.
- [x] **Step 3:** `cargo fmt --all -- --check` (unchanged code) and a read-through for broken
  links/anchors.
- [x] **Step 4: commit** `docs: document search index storage watch and retire backlog notes` —
  body: the size threshold and query for the 1 GB shared volume, and the LEAKPROOF
  re-evaluation trigger, for #34. `Refs #34`.

### Task 5: record as shipped (same PR, after the PR number exists)

**Model:** orchestrator

- [x] Set the spec's `**Status:**` to `IMPLEMENTED (#<PR>)` and tick this plan's boxes; commit
  `docs: mark search index upkeep implemented` with `Refs #34`; re-run the merge gate.
