# Search per-candidate plans — design

**Status:** IMPLEMENTED (#44)
**Issue:** #41 "Keep search per-candidate in mid-size orgs (generic plans flatten into whole-org scans)"
**Plan:** `docs/plans/2026-10-15-search-per-candidate-plans.md`
**Builds on:** `docs/specs/2026-10-14-search-under-rls-design.md` (#17),
`docs/specs/2026-10-15-search-index-upkeep-design.md` (#34)

## Brief

> In orgs of a few thousand to roughly 100k rows, both search statements can fall back to reading
> the whole org. #17 was built to avoid exactly that, and its 300k-row benchmark did not catch it:
> the cost model flips back to nested loops only above about 100k rows per org.
>
> The PR #40 database review (#34) found this with EXPLAIN ANALYZE as `otto_app` under forced
> RLS, using generic plans:
>
> - **`search_customers`**: the planner flattens the `matches` lateral
>   (`crates/cxp-core/src/customers.rs`, the `CROSS JOIN LATERAL (SELECT id, email ILIKE $2 ...)`)
>   into a Hash Join over a bitmap heap scan of all of the org's customers. Every row then gets
>   `ILIKE`/`%`. At 30k customers (warm cache) `grace` took 91 ms, `hoppr` 96 ms and `user1234@`
>   86 ms. Adding `LIMIT 1` inside the lateral (the lookup is by primary key, so results are
>   identical) brought these to 25, 18 and 2.1 ms. A 200-customer org was unaffected.
> - **`search_articles`**: the generic plan picks hashed subplans for `title_hits` and for the
>   `EXISTS (... a.title ILIKE $3)` in `matches`. When `title_read` is not empty, these seq-scan
>   all of the org's `knowledge_search_terms` and its published articles: 41 to 70 ms at 3,000
>   articles.
>
> This was already on `main` before #34 and is not a regression. Near the crossover, a cold-cache
> search could reach the 2 s `statement_timeout` (`query_too_broad`) even for a narrow query.

Acceptance criteria (#41):

- [ ] `scripts/bench-search` gains a mid-size benchmark, about 10k to 50k customers and 1k to 5k
  articles per org.
- [ ] The customer statement's lateral cannot be flattened (for example `LIMIT 1` inside it), and
  the mid-size benchmark shows the per-candidate plan.
- [ ] The article statement avoids the whole-org hashed subplans in generic plans.
- [ ] The equivalence and random-corpus tests still pass unchanged (results identical below the
  cap).

Out of scope (#41): changing the search semantics or the caps.

## Premise corrections

- **Reproduced, and worse at a mid-size seed.** On a local Postgres 16.13 seeded with
  `seed.sql -v size=mid` (below: org under test 30k customers / 3k articles, 50,200 customers and
  5,050 articles in the table), forced generic plans took 190 ms for `grace`, 200 ms for `hoppr`
  and 310 ms for `catherine.straski1@initech.example`; `user1234@` matches nothing in this data
  and was fast (2 ms) before and after. The `matches` node was a Hash Join over a Bitmap Heap Scan
  of all 30,000 org customers, each tested with `ILIKE`/`%`.
- **The article seq scan is over the whole table, not just the org.** The hashed `EXISTS` subplan
  ran `Seq Scan on knowledge_articles` with `org_id = $1` only as a filter: it reads every org's
  articles. It executes only when some title-window candidate is not already a full-text match
  (`brou` in the mid-size data: 758 rows kept of 5,050 read). Its cost grows with the whole table,
  so the shared production table makes it worse than any single-org benchmark shows.
- **The `title_hits` hashed subplan did not appear in the mid-size data**, but its shape allows it:
  the inner `NOT EXISTS (... t.term = tw.gram AND t.article_id = r.article_id)` is correlated on
  two outer columns by plain equality, which is exactly what the planner may turn into
  `NOT (hashed SubPlan)` over all of the org's index rows. `fts_hits`, the `matches` lateral's
  `fts` check, and the customer statement's `sub_hits` have the same shape. This change closes all
  of them, not only the two observed.
- **`sub_new` also reads the whole org in generic plans** (the newest-first read for broad
  substring queries: a Bitmap Heap Scan and an on-disk sort of all 30k customers instead of the
  `(org_id, created_at DESC)` index; `ab`: 111 ms generic against 58 ms custom). Its output is
  bounded (`NEWEST_READ` rows checked, 1,001 matches kept), but its read is not: the generic plan
  sizes the read by the average org, below `NEWEST_READ`, so it reads and sorts the whole org, and
  the read grows with the org beyond `NEWEST_READ`. It runs only for a broad query whose rarest
  window has more than `SUBSTRING_READ` rows. No fence forces the index order, and a rewrite that
  changes which rows it reads is a semantics change (#41 rules those out); listed as a follow-up.
  Two joins are also unfenced and per candidate in every plan seen: `fz_read`'s lateral (a nested
  loop over a few trigrams; a hash join would read all of the org's grams) and `ranked`'s join to
  `knowledge_articles` (a nested loop on about 160 estimated rows). Neither is a fence target
  here.

## Assumptions

- **No Run Profile key was missing** from the Development Workflow section; nothing is assumed
  there. Deploy is `ci-auto`; the Dockerfile and CI workflow exist, so the CI merge gate applies.
- **`OFFSET 0` is the fence for correlated `EXISTS`/`NOT EXISTS`.** Postgres turns a correlated
  `EXISTS` into a semi/anti join or a hashed `ANY` subplan only after `simplify_EXISTS_query`,
  which refuses a subquery with an `OFFSET` clause (`LIMIT` alone is stripped, so `LIMIT 1` would
  not fence an `EXISTS`). With `OFFSET 0` the check stays a per-row subplan: one btree probe per
  candidate. `OFFSET 0` changes no result. This holds in Postgres 16 (CI and local), and the same
  function in the 17 and 18 sources (checked 2026-10-15).
- **`LIMIT 1` is the fence for the `matches` lateral**, as the issue proposes: a subquery with a
  `LIMIT` is never pulled up into the outer join tree. The lookup is `org_id = $1 AND id = c.id`
  and `id` is the table's primary key, so it returns at most one row and `LIMIT 1` changes no
  result.
- **Generic plans are what production runs.** sqlx prepares each statement once per connection
  and Postgres moves to the generic plan after five executions when it looks no costlier; the
  benchmark and the plan checks force `plan_cache_mode = force_generic_plan` to see that plan
  directly.
- **The mid-size seed is a variant of `seed.sql`, not a second file**, chosen by a psql variable,
  so both sizes share one data generator.

## Goal & Success Criteria

Every per-candidate step of both search statements stays a per-candidate probe in generic plans,
whatever the org size, so a narrow search costs what its candidates cost, with results unchanged.

- In forced generic plans on the mid-size seed, no `matches` node reads `customers` other than by
  index, and no hashed subplan reads `customers`, `customer_search_grams`, `knowledge_articles` or
  `knowledge_search_terms`; `scripts/bench-search/plans.py` reports this per query.
- `grace`, `hoppr` and the email query run in generic plans within 1.5x or 5 ms of their custom
  plans, whichever is looser, on the mid-size seed (measured generic/custom: 22/26, 5/3,
  66/53 ms).
- A new test fails on the current statements and passes on the new ones, by checking generic plan
  shape with the planner pushed toward the flattened/hashed alternatives.
- The equivalence and random-corpus tests in `crates/cxp-core/tests/customers.rs` and
  `tests/support.rs` pass unchanged.

## Scope

In:

- `SEARCH_CUSTOMERS_SQL`: `LIMIT 1` inside the `matches` lateral; `OFFSET 0` inside `sub_hits`'s
  inner `NOT EXISTS`.
- `SEARCH_ARTICLES_SQL`: `OFFSET 0` inside the inner `NOT EXISTS` of `title_hits`, of `fts_hits`,
  and of the `matches` lateral's `fts` check, and inside the `EXISTS (... a.title ILIKE $3)`.
- Doc comments on both statements naming the fences and why.
- Two `#[doc(hidden)] pub fn`s exposing the statement text for the plan-shape test.
- `crates/cxp-core/tests/search_plans.rs`: the plan-shape test.
- `scripts/bench-search`: `seed.sql -v size=mid`, `plans.py` (the generic-plan check), regenerated
  `after-*.sql`, `run.sh` notes.

Out:

- Search semantics, ranking, caps and bounds (`search.rs` constants) — unchanged.
- `sub_new`'s whole-org read in generic plans (Premise corrections; follow-up).
- Any migration, index, config key, secret, scope or role change.
- Forcing custom plans (`plan_cache_mode` per search): rejected, since it re-plans these large
  statements on every call and fixes nothing for a plan the planner gets wrong with real values.

## Architecture

Both statements are `LazyLock<String>` constants in `cxp-core` (`SEARCH_CUSTOMERS_SQL` in
`customers.rs`, `SEARCH_ARTICLES_SQL` in `knowledge.rs`), run by `CustomersExt::search_customers`
and `KnowledgeExt::search_articles`. The change is text inside those constants:

1. `customers.rs`, `matches`:
   ```sql
   CROSS JOIN LATERAL (
     SELECT id, email ILIKE $2 AS em, similarity(name, $3) AS sim, created_at
     FROM customers
     WHERE org_id = $1 AND id = c.customer_id
       AND (email ILIKE $2 OR name ILIKE $2 OR name % $3)
     LIMIT 1) AS m
   ```
2. `customers.rs`, `sub_hits`: `... AND c.customer_id = r.customer_id OFFSET 0)) `.
3. `knowledge.rs`: the same `OFFSET 0` before the closing parenthesis of each of the four
   subqueries listed under Scope.

Test hook (additive public API, hidden from docs, the `db_role::for_test` precedent):

```rust
// customers.rs
#[doc(hidden)]
pub fn search_customers_sql() -> &'static str { &SEARCH_CUSTOMERS_SQL }
// knowledge.rs
#[doc(hidden)]
pub fn search_articles_sql() -> &'static str { &SEARCH_ARTICLES_SQL }
```

Plan-shape test (`tests/search_plans.rs`): seeds one org (a few hundred customers, a few hundred
published articles) and a second org, runs `ANALYZE`, opens a tenant transaction (`otto_app`, org
pinned, as every search runs), `PREPARE`s the statement with explicit parameter types, sets
`plan_cache_mode = force_generic_plan`, and reads `EXPLAIN (FORMAT JSON) EXECUTE ...`. To make
the bad alternatives the cheap ones, the customer check sets `enable_nestloop = off` (a flattened
lateral would then be a hash or merge join) and the article and `sub_hits` checks set
`enable_indexscan`, `enable_indexonlyscan` and `enable_bitmapscan = off` (a per-row probe is then
a seq scan per candidate, so a hashed subplan wins whenever the planner may choose one). It
asserts:

- customers: inside the `CTE matches` subtree, every plan node that reads `customers` is an
  `Index Scan` or `Index Only Scan`, and no `Hash Join` or `Merge Join` appears there;
- both statements: no subplan the plan marks `hashed SubPlan N` (in any node's filter or
  condition text) reads one of the four search tables, where "reads" means a scan node in that
  subplan's own subtree whose `Relation Name` is the table (a `CTE Scan` has none, so the
  expected hashed `NOT IN (SELECT ... FROM sub_all)` / `fts_hits` subplans over CTEs pass).

On the unfenced statements, with the test's own seed, the hashed-subplan check failed on
`sub_hits`' probe (a hashed subplan seq-scanning `customer_search_grams`) and, with only the
article fences removed, on a `knowledge_search_terms` probe; the mid-size benchmark data hashed
the article title `EXISTS` instead. Which probe the planner hashes depends on the statistics, which
is why every probe is fenced. The PR records the failures.

`plans.py` applies the same two rules to `EXPLAIN (ANALYZE, FORMAT JSON)` of forced generic plans
against a seeded benchmark database, with no planner knobs, and prints per query the execution
time and `per-candidate` or the offending node. It reads the generated `after-*.sql` (so it runs
exactly the statements `gen.py` extracted): the search statement is the one line starting
`WITH ` (the page-rows `SELECT` after it is ignored), and its pgbench variables are turned back
into `$n`.

No public interface (route, tool, wire field, config key) changes. The two `pub fn`s are additive.

## Invariants impact

- **No SQL outside `cxp-core`; runtime `sqlx::query` only:** holds. The statements stay in
  `cxp-core`; the test is a `cxp-core` integration test and uses runtime queries.
- **Tenant isolation is RLS:** unchanged. The plan test runs inside a tenant transaction as
  `otto_app`, so it checks the plans RLS produces (the policies' `One-Time Filter`s included).
- **Untrusted text / no database text in responses:** unchanged; error mapping is untouched.
- **The database is small and shared; no unbounded list queries:** strengthened. Each candidate
  step reads by index per candidate again instead of the whole org or table.
- **Append-only migrations:** no migration.
- **Search index (#17) contract:** `totalCapped: false` results identical to a full scan. Holds:
  `OFFSET 0` and `LIMIT 1` on a primary-key lookup change no row; the equivalence and
  random-corpus tests run unchanged.

## Migration

None.

## Out-of-band

None: no config key, secret, scope, migration, role change, or recurring cost. A merge deploys
through CI (`ci-auto`, when `FLY_DEPLOY` is `enabled`).

## Error Handling & Edge Cases

- The statements' error handling (`search::on_timeout`, `query_too_broad`) is unchanged.
- `LIMIT 1` in the lateral: the subquery matches on the primary key, so it yields zero or one row
  either way.
- `OFFSET 0` inside `EXISTS`: no change to which rows exist.
- A planner that one day pulls up subqueries with `OFFSET 0` would undo the fence; the plan test
  would then fail in CI on that Postgres version rather than silently regress.
- `EXPLAIN` in the test needs the statement to plan with the same types the app binds: `uuid`,
  `text`, `text`, `int8`, `int8` (customers) and `uuid`, `text`, `text`, `int8` (articles).

## Testing Approach

- Gate `DATABASE_URL=postgres://cxp:cxp@localhost:15466/otto_cxp cargo test --workspace --locked`
  (this run's own container; CLAUDE.md's gate names :15435):
  - new `crates/cxp-core/tests/search_plans.rs`: `customer_matches_probe_each_candidate`,
    `search_index_probes_are_never_hashed` (customers' `sub_hits` and all article probes). Each is
    first run against the current statements to see it fail, then passes with the fences.
  - existing equivalence and random-corpus tests (`customers.rs`, `support.rs`) unchanged.
- clippy, fmt, the append-only migration check, and the `podman build` gate (crates changed).
- Benchmark evidence, recorded in the PR: `plans.py` and `run.sh` on the mid-size seed, before
  and after.
- Review: CLAUDE.md's extra reviewer `database-optimizer` runs on the PR (SQL changes).

## Risks & Open Questions

- **The plan test leans on planner knobs and statistics.** Postgres keeps either the hashed or the
  per-row form of an unfenced probe by estimated cost, so a plan shows a missing fence only when
  the statistics favour the bad form: with the test's seed every fence but the article `matches`
  `fts` check's fails a plan test when removed. `every_probe_keeps_its_fence` therefore checks all
  six fences by their text too. If a future Postgres chose differently under the knobs, a plan
  test could fail without a real regression; the failure message prints the offending node.
- **`sub_new`** stays a whole-org read in generic plans for broad queries (follow-up).
- **Production's Postgres major version** is not checked here; the fences rely on planner rules
  that are the same from 16 through 18.
