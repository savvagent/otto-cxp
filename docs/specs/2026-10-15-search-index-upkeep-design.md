# Search index upkeep — design

**Status:** DRAFT
**Issue:** #34 "Watch and slim search index storage; retire lazy backlog indexing"
**Plan:** `docs/plans/2026-10-15-search-index-upkeep.md`
**Builds on:** `docs/specs/2026-10-14-search-under-rls-design.md` (#17, PR #33)

## Brief

> #17's search index tables (`customer_search_grams`, `knowledge_search_terms`) cost about 7 kB
> per customer and 25 kB per published article at scale. That is roughly 2–3 GB per
> 300k-customer org on the small shared `otto-db`. Inserts get slower: about 3 ms per customer and
> 36 ms per article at test scale. Search also indexes legacy rows lazily, a small batch per
> search call. Found by the #17 reviews (PR #33).

Acceptance criteria (#34):

- [ ] A size threshold is documented in `docs/deploy/fly.md` for when to act (dedicated
  instance, or a slimmer index), with the SELECT-only `pg_stat_user_tables` /
  `pg_stat_user_indexes` query from the PR #33 database review to watch it.
- [ ] Once production has no `search_indexed = false` rows (confirmed SELECT-only), a release
  removes the per-search backlog step and the backlog CTEs; the partial indexes stay, because
  migrations are additive.
- [ ] The LEAKPROOF `similarity_op` + GIN route is re-evaluated if fuzzy recall or storage
  becomes a problem, with a boot check on `proleakproof`.

Risk (Owner: Rob): the LEAKPROOF route is a superuser change on the shared instance, and is lost
on `ALTER EXTENSION pg_trgm UPDATE` or dump/restore.

The issue's three comments are in scope (PR #33 re-reviews, all Minor or defence in depth):

1. Security re-audit: a short `statement_timeout` for the backlog batch and no retrying of a
   failing batch; consider lowering `NEWEST_READ` or per-caller search limits; add
   `MAX_CONTENT_CHARS` for article content before any route or tool exposes article writes.
   Production on 2026-10-15: 0 customers and 0 articles in every org.
2. Database re-review: `coalesce(current_setting('pg_trgm.similarity_threshold', true)::float8,
   0.3)` instead of relying on `show_trgm` running first in `fp`; measure a cold-cache search or
   document a first-search `statement_timeout` as an acceptable retriable error; skip the
   20,001-row count when `sub_new` already reached 1,001 matches.
3. Rust re-review: a backlog row that always fails stalls the org's backlog (batches in
   `ORDER BY id`); the property tests accept `capped: true` with no items, so assert most of the
   250 corpus queries are uncapped; map `statement_timeout` (SQLSTATE 57014) on search to a
   distinct error code instead of `internal`.

## Premise corrections

- **No watch query is recorded in PR #33.** Its comments name the need but not the SQL. This
  change writes the query (below) and runs it once on production for a baseline.
- **`otto-db`'s volume is 1 GB** (`fly volumes list -a otto-db`: `pg_data`, 1GB), not "small"
  in the abstract. Every threshold below is a fraction of that volume, shared with three other
  services' databases. 2–3 GB per large org cannot fit at all; the threshold must trigger far
  earlier.
- **Rows with `search_indexed = false` cannot be written any more.** Migration 0004's
  `BEFORE INSERT OR UPDATE` trigger `search_mark_indexed` sets the flag on every write, whatever
  the statement says. The only false rows are those that existed when 0004 ran; production had
  none then (0 customers, 0 articles) and has none now. Removing the backlog path therefore
  needs no migration and no backfill. A row loaded with triggers disabled (a superuser restore
  with `session_replication_role = replica` or `--disable-triggers`, or a data-only `COPY`) can
  lack index rows; it usually reads `search_indexed = true` (the default since 0004), so the
  runbook detects such rows by their missing index rows, not by the flag, and rebuilds them in
  two steps (final review, #34).
- **Comments 1 (backlog timeout, batch retry) and 3 (stalled backlog) are moot** once the
  backlog path is gone: there is no batch left to time out or stall.

## Assumptions

- **Production confirmation (SELECT-only), 2026-10-15, `fly postgres connect -a otto-db -d
  otto_cxp` in a `READ ONLY` transaction:** `customers` 0 rows, 0 unindexed; `knowledge_articles`
  0 rows, 0 unindexed; `_sqlx_migrations` max version 4. The search index tables total 56 kB;
  `otto_cxp` is 9.5 MB.
- **No migration.** `search_indexed`, its default, `search_mark_indexed`, the `NOT OLD.search_indexed`
  arms of the sync triggers' `WHEN` clauses, and the two partial backlog indexes all stay
  (additive migrations; the previous release still reads `search_indexed` and must boot against
  this schema). They are now dead weight of a few kB; dropping them is a later, separate change
  if ever.
- **`NEWEST_READ` stays 20,000.** It is what makes substring search exact in orgs of up to 20k
  customers (an equivalence test pins it), and a broad query's cost is already bounded by it and
  by the 2 s `statement_timeout`. Per-caller search limits are rate limiting, which belongs to a
  separate change across every tool, not to search. Recorded here as the re-audit's "consider".
- **`MAX_CONTENT_CHARS` = 100,000** characters for article content (create and update), chosen
  as roughly 30 printed pages: far above any real help article, while bounding an article's
  index rows (one `l` row per distinct lexeme) and its `to_tsvector` cost. No route or tool
  writes articles today, so the limit rejects nothing live.
- **The timeout code is `query_too_broad`, NOT retriable (revised in PR review).** The PR #40
  reviewers (security, silent-failure, Rust, architect) pointed out that a retriable 503 invites
  generic retry layers and agents to repeat a 2 s broad query on the 5-connection shared pool.
  It is now REST 422 with no `Retry-After`, MCP `INVALID_PARAMS` with `retriable: false`, and the
  message says to narrow the query; a cold-cache timeout is rare and the caller's next, narrower
  search runs warm. The original reasoning follows. A search can hit its 2 s timeout either
  because it is broad in a large org or because the first search after a resume reads a cold
  cache (Fly suspends the machine; the shared instance's cache is not ours). Either a narrower
  query or a plain retry can succeed, so it is retriable and the message says both. REST answers
  503 with `Retry-After` (the existing rule: 503 iff retriable, and only 503 carries
  `Retry-After`); MCP answers `INVALID_PARAMS` (the default for non-internal codes) with
  `data.code = "query_too_broad"`, `retriable: true`. It is not logged as internal; core logs a
  `warn` naming the search, never the query text.
- **Cold-cache measurement is documented, not run.** Measuring on Fly's volume would mean seeding
  production (a write; human-only). With 0 rows today a cold search reads a handful of pages; the
  fly.md section records the retriable error as acceptable and says to measure once an org nears
  the threshold.
- **Run Profile:** every value comes from CLAUDE.md "## Development Workflow"; none assumed.
  Tests run against a dedicated container on port 15462 (parallel worktrees share :15435).

## Goal & success criteria

Retire the lazy backlog path that production never needed, tighten the search statements per the
re-reviews, and give Rob a threshold and a query to know when the index's storage needs action.

- `cxp_core::search` has no `index_backlog`, `Indexed`, `BACKLOG_READ`, or savepoint; neither
  search statement has a `backlog` CTE; searches no longer write.
- A search that hits `statement_timeout` returns `query_too_broad` (not retriable), proven by a test
  that blocks search on a lock past the timeout.
- `fp` reads the threshold with `current_setting(..., true)` and a 0.3 fallback; the 20,001-row
  count runs only when the match count has not already capped the result.
- Article content over `MAX_CONTENT_CHARS` is `invalid_argument` on create and update.
- Both random-corpus tests assert at least 95% of the queries they run are uncapped (measured:
  100%; the corpora are fixed-seed).
- `docs/deploy/fly.md` holds the threshold, the watch query, the production baseline, and the
  LEAKPROOF re-evaluation trigger with its boot-check design; CLAUDE.md's "Search index (#17)"
  matches the code.

## Scope

**In:** `crates/cxp-core/src/{search,customers,knowledge,error}.rs`; `crates/cxp-api/src/error.rs`
(status for the new variant, doc table); `crates/cxp-mcp/src/error.rs` (test case); tests in
`crates/cxp-core/tests/{customers,support,search_backlog}.rs`; `docs/deploy/fly.md`; CLAUDE.md;
this spec and its plan.

**Out:** any migration; dropping `search_indexed` or the partial indexes; LEAKPROOF, GIN, or any
boot check (documented only: superuser change, Rob); slimming the index rows (surrogate keys,
prefix-only grams); rate limiting; article write routes or tools; the `Paginated` envelope for
search (#24); lowering `NEWEST_READ`.

## Architecture

### Retire the backlog (`search.rs`, `customers.rs`, `knowledge.rs`)

- `search.rs`: delete `BACKLOG_READ`, `Indexed`, `index_backlog`, and the savepoint block.
  `prepare(tx)` keeps only `SET LOCAL jit = off` and `SET LOCAL statement_timeout = '2s'`; the
  module doc stops describing a backlog and says every row is indexed by its triggers
  (`search_mark_indexed` forces the flag on every write).
- `SEARCH_CUSTOMERS_SQL`: delete the `backlog` CTE, its `UNION ALL` arm in `matches`, and its
  `capped` disjunct; `fz_all` drops `AND search_indexed` (it existed only to keep backlog rows
  out of `fz_all`, and with no false rows it filters nothing). The doc comment loses the
  `backlog` bullet.
- `SEARCH_ARTICLES_SQL`: delete the `backlog` CTE, the third `UNION ALL` arm of `matches` (the one
  reading `FROM backlog b, tsq, knowledge_articles a`), and its `capped` disjunct; doc bullet
  removed.
- Searches are again read-only statements (apart from `SET LOCAL`).

### Statement tweaks

- `fp`: `coalesce(nullif(current_setting('pg_trgm.similarity_threshold', true), '')::float8, 0.3)
  AS th`. `current_setting(name, true)` returns NULL when pg_trgm is not yet loaded in the
  session and no placeholder is set; pg_trgm's default is then 0.3, which is what `%` will use.
  A placeholder set before pg_trgm loads (`SET pg_trgm.similarity_threshold = 0.5`) is returned as
  set, and is what pg_trgm adopts on load. `nullif(…, '')` guards a placeholder reset to empty.
  The comment no longer relies on `b` being evaluated first.
- `capped` (customers): `CASE WHEN (SELECT count(*) FROM matches) > cap THEN true ELSE (<the other
  disjuncts>) END`. CASE evaluates its branches in order and the uncorrelated subqueries are
  init-plans run only on first use, so the `LIMIT 20001` count of the org's customers runs only
  when the result is not already capped by its match count (a `sub_new` that found 1,001 matches
  makes `matches` exceed the cap). The `OR` chain already short-circuits in practice; CASE
  guarantees it rather than speeding anything up. A placeholder threshold outside [0, 1] set
  before pg_trgm loads would be read raw by `fp` while pg_trgm rejects it on load; no surface can
  set one. The articles statement has no such count and is unchanged.

### Timeout error (`error.rs`, `search.rs`)

- New variant `Error::QueryTooBroad`, `Display`: "this search ran longer than otto-cxp allows;
  narrow the query (more characters or more specific words), or retry". `code()` →
  `"query_too_broad"`; `retriable()` → false (revised; see Assumptions); `is_internal()` → false.
- `search::on_timeout(kind: &'static str) -> impl FnOnce(sqlx::Error) -> Error`: SQLSTATE `57014`
  becomes `QueryTooBroad` after `tracing::warn!(search = kind, "search hit its statement_timeout")`;
  anything else is `Error::Database(e)`. Applied with `map_err` to both statements of each search
  (the hits statement and the rows-by-id read), since both run under the timeout.
- `cxp-api` `status_for`: `QueryTooBroad` → 422, no `Retry-After` (revised from 503); the module's
  code table gains the row. `cxp-mcp` `from_core` needs no change (non-internal, falls to
  `INVALID_PARAMS`); its test table gains the case.
- Public interface change: one new, additive error code on `search_customers`,
  `search_knowledge_base`, and `GET /api/v1/customers/search`. Clients branch on `code`; the
  TypeScript SDK passes codes through unchanged.

### Content limit (`knowledge.rs`)

`pub const MAX_CONTENT_CHARS: usize = 100_000;` and `checked_content`, applied in
`create_article` and in `update_article` when `content` is set, like `checked_title`.

### Docs

- `docs/deploy/fly.md`, Infrastructure bullet: drop the "searches also index … backlog" sentence;
  link a new section **"Search index storage (#34)"** that holds:
  - the threshold: act when the two index tables together pass **100 MB** (10% of the 1 GB
    `otto-db` volume; about 10k customers or 3k published articles at the measured per-row cost,
    less once grown a row at a time), or `otto_cxp` passes 250 MB, or the volume passes 60% used (lowered from 70% in PR review: rebuild and WAL headroom).
    The actions, in order of preference: split otto-cxp onto its own instance (recurring cost:
    Rob), or slim the index via the LEAKPROOF route;
  - the SELECT-only watch query (tables via `pg_stat_user_tables`, indexes via
    `pg_stat_user_indexes`, totals via `pg_total_relation_size` / `pg_database_size`), run in a
    `READ ONLY` transaction through `fly postgres connect -a otto-db -d otto_cxp`, and the
    2026-10-15 baseline;
  - the backlog note: no row can be written unindexed; a restore with triggers disabled must be
    followed by an index rebuild, with the exact SQL (a human step: `UPDATE … SET search_indexed =
    true` per table, run as a superuser, which fires the triggers);
  - the first-search timeout: `query_too_broad` from a cold cache is acceptable;
    measure one cold search once an org nears the threshold;
  - **LEAKPROOF re-evaluation**: when to revisit (index past the threshold, or long fuzzy queries
    reported capped/incomplete), what it is (Rob, superuser, `ALTER FUNCTION
    public.similarity_op(text, text) LEAKPROOF` in `otto_cxp` only, then a GIN index in a later
    migration), what loses it (`ALTER EXTENSION pg_trgm UPDATE`, dump/restore), and the boot
    check that change must add: refuse to use the GIN path (or refuse to boot) unless
    `SELECT proleakproof FROM pg_proc WHERE oid = 'public.similarity_op(text,text)'::regprocedure`
    is true, so a lost flag degrades to a loud failure instead of silent sequential scans.
- CLAUDE.md "Search index (#17)": drop the "Searches are `cxp:read` yet write" sentence; say every
  row is indexed by triggers (no backlog), a search times out at 2 s as `query_too_broad`, and
  point at fly.md for the storage watch.

## Invariants impact

- **Append-only migrations / additive:** no migration; nothing dropped. The previous release
  (which still runs `index_backlog` and the backlog CTEs) works against this schema unchanged, and
  this release works against it too. Rolling deploy is safe in both directions.
- **Tenant isolation:** unchanged; fewer statements. No SQL leaves `cxp-core`.
- **`begin_live` on request paths:** unchanged.
- **Untrusted text / no database text in responses:** the new error's `Display` is fixed text; the
  log line names the search kind only.
- **Small shared database:** search no longer writes; the count is skipped when not needed.
- **Metering, auth, scopes, webhooks, licensing, pin:** untouched.

## Out-of-band

None. No config key, secret, scope, migration, or role change. The watch query is documented for
Rob to run; it is SELECT-only.

## Error handling & edge cases

- A 57014 raised for another reason (an operator's `pg_cancel_backend`) also reads as
  `query_too_broad`; telling the caller to narrow the query is harmless.
- `statement_timeout` is `SET LOCAL`, so it ends with the transaction; nothing else changes.
- A threshold placeholder set to an unparsable string still errors as before (`22P02` →
  `invalid_argument`); not reachable through any surface.

## Testing approach

Gates: `DATABASE_URL=postgres://cxp:cxp@localhost:15462/otto_cxp cargo test --workspace --locked`,
clippy, fmt, the migrations check, `podman build -t otto-cxp:check .` (crates changed). No pnpm
path changes.

- `tests/customers.rs`: delete `a_backlog_larger_than_its_read_is_capped_until_indexed`,
  `a_failing_backlog_batch_does_not_fail_search` (and its `Logs`), and
  `indexing_the_backlog_does_not_block_references_to_it`. Add
  `rows_are_indexed_whatever_the_insert_says` (an insert and an update naming
  `search_indexed = false` read back true and have grams) and
  `a_search_past_its_timeout_is_query_too_broad` (a superuser holds `ACCESS EXCLUSIVE` on
  `customer_search_grams`; the search fails after 2 s with `query_too_broad`, not retriable, no
  database text). The random corpus test counts uncapped queries and asserts >= 95%.
- `tests/support.rs`: the article corpus test asserts >= 95% uncapped;
  `article_content_is_length_limited` (create and update, at and past the limit); an article
  timeout test as for customers (lock `knowledge_search_terms`).
- `tests/search_backlog.rs`: the least-privilege test no longer calls `index_backlog`. After 0004
  the predating rows are unindexed and absent from search; an ordinary `UPDATE … SET
  search_indexed = true` in each org's own pinned transaction indexes that org's rows (org B's
  stay untouched by A's), and search is then exact. The file's doc says this is the rebuild the
  runbook describes.
- `crates/cxp-core/src/error.rs` unit tests: the new code, not retriable, not internal, fixed text.
- `crates/cxp-api/src/error.rs`: status 503 with `Retry-After` for the new variant.
- `crates/cxp-mcp/src/error.rs`: the new code in its table.
- Existing equivalence and threshold tests (`indexed_search_follows_the_similarity_threshold`)
  cover the `fp` and `capped` rewrites.

## Risks & open questions

- **A restore with triggers off** would leave rows invisible to search until rebuilt. Mitigated by
  the runbook step; a boot check could detect `search_indexed = false` rows, but it would need
  cross-org reads (maintenance) on every boot for a case that needs a human anyway. Not added.
- **Removing the `fz_all` filter** makes a non-positive threshold read all customers, as it did
  for indexed ones; bounded by `FUZZY_CHECKED` as before.
- **The 95% bound** is calibrated on the fixed-seed corpora; it is deterministic, so it cannot
  flake, but a future change to the corpora must re-check it.
