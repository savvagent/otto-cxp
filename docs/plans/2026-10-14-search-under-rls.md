# Search under RLS — plan

Spec: `docs/specs/2026-10-14-search-under-rls-design.md`. Issue #17.

All commands run in the worktree; `DATABASE_URL` points at a local Postgres only.

- [x] **1. Reproduce.** `scripts/bench-search/seed.sql` into a scratch database, then
  `scripts/bench-search/run.sh <url>`; capture `EXPLAIN (ANALYZE, BUFFERS)` of both scans as
  `otto_app` pinned to the big org. Record the premise corrections in the spec.
- [x] **2. Failing tests first.**
  - `crates/cxp-core/tests/customers.rs`: `indexed_search_returns_exactly_what_the_scan_returns`,
    `indexed_search_follows_the_similarity_threshold`,
    `indexed_search_matches_the_scan_on_near_threshold_names` (each compares
    `search_customers` with the pre-#17 SQL, `SEARCH_BY_SCAN`), and
    `the_search_index_follows_customer_writes`, `raw_sql_writes_are_indexed`.
  - `crates/cxp-core/tests/support.rs`: `indexed_article_search_returns_exactly_what_the_scan_returns`
    (against `ARTICLES_BY_SCAN`) and `the_search_index_follows_article_writes`.
  - `crates/cxp-core/tests/db.rs`: `SEARCH_INDEX_TABLES` in the isolation report, the cross-org,
    unpinned, maintenance-marker, composite-foreign-key, and grant checks.
  - `crates/cxp-core/tests/lifecycle.rs`: the purge covers both tables, before their parents.
- [x] **3. Migration** `crates/cxp-core/migrations/0004_search_index_tables.sql`: `search_windows`,
  `customer_search_grams_of`, `knowledge_search_terms_of`; the two tenant tables with forced RLS,
  `<table>_tenant_isolation` policies, composite foreign keys, and grants; the sync triggers; the
  RLS-aware backfill.
- [x] **4. Queries.** `SEARCH_CUSTOMERS_SQL` in `crates/cxp-core/src/customers.rs` and
  `SEARCH_ARTICLES_SQL` in `crates/cxp-core/src/knowledge.rs`; `without_jit` in
  `crates/cxp-core/src/lib.rs`; both tables in `PURGED_TABLES`
  (`crates/cxp-core/src/platform_events.rs`), children first.
- [x] **5. Measure after**, same scripts, and record the numbers and storage in the spec.
- [x] **6. Gates.**
  - `DATABASE_URL=postgres://cxp:cxp@localhost:<port>/otto_cxp cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'` prints nothing
- [ ] **7. Final commit before merge:** set the spec's `**Status:**` to `IMPLEMENTED (#<PR>)` and tick
  this box. Renumber the migration if `0003` has not landed on `main` by then (the orchestrator
  owns this).
