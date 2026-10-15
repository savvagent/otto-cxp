# Search under row-level security — design

**Status:** DRAFT (#17)
**Issue:** #17 "Make customer and knowledge-base search fast under row-level security"
**Plan:** `docs/plans/2026-10-14-search-under-rls.md`

## Brief

> Under forced row-level security, Postgres applies only leakproof operators as index conditions
> ahead of the RLS predicate. `ILIKE`, the trigram `%` operator and full-text `@@` are not
> leakproof, so customer search and knowledge-base search scan every row of the caller's org.

Acceptance criteria (verbatim from #17):

1. Customer and knowledge-base search stay under ~50 ms at 300k rows per org when run as
   `otto_app` under forced RLS.
2. The chosen approach (e.g. marking `texticlike`, `similarity_op`, `ts_match_vq` LEAKPROOF on
   `otto-db`, or a search design that doesn't depend on them) is recorded with its security
   reasoning.

Risk (verbatim): marking operators LEAKPROOF is a superuser change on the shared instance and a
security decision; owner Rob.

**Outcome in one paragraph.** This change ships a search design that needs no superuser: each
customer and article gets rows in an org-scoped inverted index (`customer_search_grams`,
`knowledge_search_terms`), looked up with text equality and prefix ranges, which are leakproof and
so serve as index conditions under RLS. The original predicate still decides every result, so
results are identical to before. Selective queries now take 3 to 70 ms instead of 1 to 3 s.
Criterion 1 is **met for selective queries and missed for broad ones**: a query matching thousands
of rows still takes 85 to 650 ms, because every match has to be fetched and ranked. Indexing costs
about 7 kB per customer and 22 kB per article. The LEAKPROOF option needs a fraction of the
storage and is about twice as fast on broad queries. It is written up below as a decision for Rob
(see Human hand-off). Nothing in this change marks anything LEAKPROOF.

## Premise corrections

Measured as `otto_app` with forced RLS, `max_parallel_workers_per_gather = 0`, on the benchmark
below. `EXPLAIN (ANALYZE, BUFFERS)` of the pre-#17 `search_customers` for `navy`:

```
Limit (actual time=1013.920..1013.927 rows=50)
  -> Sort  (top-N heapsort)
     -> Result  One-Time Filter: ((NULLIF(current_setting('app.org_id', true), ''))::uuid = '…0a'::uuid)
        -> Seq Scan on customers (actual time=0.904..957.917 rows=17861)
             Filter: ((org_id = '…0a') AND ((email ~~* '%navy%') OR (name ~~* '%navy%') OR (name % 'navy')))
             Rows Removed by Filter: 402139
```

- **The RLS predicate itself is not the cost.** `current_org()` is an inlined SQL function. The
  planner merges `org_id = current_org()` with the query's own `org_id = $1` into one equivalence
  class, evaluates `current_setting` once (the one-time filter), and could use `(org_id, …)`
  indexes for the org. Counting the org's 300k customers as `otto_app` is an index-only scan
  (about 25 ms). Here it chose a sequential scan because the org is 70% of the table.
- **The cost is evaluating the text operators on every org row.** `email ILIKE` alone takes 290 ms
  across 300k rows, `name % q` alone takes 480 ms (it builds the trigrams of every name), and all
  three together take about 1 s. A GIN trigram index can't help because those operators aren't
  leakproof. That part of the ticket is right.
- **Knowledge-base search is worse than the ticket implies:** 1.1 to 3 s at 300k articles, not
  ~1 s. Most of the time goes to de-TOASTing every article's stored `search_tsv` (1M buffer hits
  per query) before `@@` can run on it.
- **Marking the operators LEAKPROOF would not touch the other services.** `pg_proc` is
  per-database: `ALTER FUNCTION texticlike(text, text) LEAKPROOF` run in `otto_cxp` changes only
  `otto_cxp`'s catalog. `otto_platform`, `otto_factory`, and `otto_flags` would not see it. It
  still needs a superuser, and it is still a security decision (see below).
- **Two smaller corrections.** `search_customers` returns no total (`List::uncounted`), so there
  is no count query to speed up. `resolve_customer`'s substring fallback scans the org the same
  way, but it stops at 2 rows and is not search (out of scope).

## Assumptions

- "300k rows per org" applies to both customers and articles. A real knowledge base is closer to
  10² to 10⁴ articles. The 300k-article numbers below are a stress case.
- Benchmark host: a laptop running Postgres 16.13 in podman, `shared_buffers = 4GB`, warm cache,
  JIT on (the default), no parallel workers (production runs on a shared-cpu-1x machine).
  Production `otto-db` is smaller and shared, so its absolute numbers will be worse. Other agents'
  test runs shared the host during the final run, so the "before" figures are noisy (an earlier,
  quieter run gave 0.65 to 1.3 s for the same customer queries).
- Search semantics must not change at all: the same rows in the same order, for every query,
  page, and `pg_trgm.similarity_threshold`. The tests check exactly this against the old SQL.
- Migration number `0004`: `0003` is reserved for #14's `0003_console_sessions.sql`. On this
  branch alone, `migrate::tests::versions_are_contiguous_from_one` therefore fails until #14
  lands (expected; the orchestrator rebases or renumbers).

## Goal & success criteria

- `search_customers` and `search_articles` return exactly what the pre-#17 SQL returns (ids and
  order, any page) for every query, including one- to three-character queries, LIKE wildcards,
  non-ASCII text, and any similarity threshold.
- No index-assisted path depends on a non-leakproof operator, a `SECURITY DEFINER` function, or
  any other way around RLS. Every new table is a tenant table under the usual invariants.
- Measured before/after numbers and storage costs are recorded here, and the LEAKPROOF
  alternative is written up with its exact SQL for Rob.

## Scope

In: migration 0004 (two tables, their triggers and helper functions, and a backfill),
`SEARCH_CUSTOMERS_SQL` and `SEARCH_ARTICLES_SQL`, the purge list, tests, the benchmark
(`scripts/bench-search/`), this spec, and the plan.

Out: `resolve_customer`'s fallback scan; changing search semantics (prefix-only search, a total,
different ranking); any LEAKPROOF change (a hand-off); `list_*` queries (already index-served).

## Architecture

### Why not the obvious designs

| Design | Result |
|---|---|
| GIN trigram / full-text index | Its operators (`%`, `~~*`, `@@`, `@>` …) are all non-leakproof (checked in `pg_amop`), so RLS keeps it from ever being an index condition. |
| Bloom-filter signature per row (bit columns, `(sig & m) = m`) | Prototyped. It is still a scan of every org row (the filter can't be indexed): 160 ms to 4.6 s. Rejected. |
| A table, view, or materialized view without RLS, or a `SECURITY DEFINER` function | Bypasses RLS. Forbidden. |
| Search predicate inside a restrictive policy (policies run at security level 0, so non-leakproof is allowed) | Puts query text in a GUC and makes every customer query pay for it. Too clever and fragile. Rejected. |
| **Inverted index in tenant tables, looked up with leakproof btree operators** | Chosen. |

Leakproof operators that a btree can use (checked in `pg_proc` on PG16): `texteq`, `text_lt/le/gt/ge`,
`starts_with`, `uuid_eq`, and the integer comparisons. An index on `(org_id, gram, id)` queried
with `org_id = $1 AND gram = $2` is therefore an index condition under RLS. Its rows are filtered
by the tables' own `org_id = current_org()` policy like any other tenant row.

### Schema (`crates/cxp-core/migrations/0004_search_index_tables.sql`)

- `customer_search_grams (org_id, customer_id, gram text COLLATE "C", name_grams smallint)`, with
  PK `(org_id, gram, customer_id) INCLUDE (name_grams)` and an `(org_id, customer_id)` index for
  the trigger, the cascade, and the purge. It has a composite FK to `customers (org_id, id)` with
  `ON DELETE CASCADE`.
- `knowledge_search_terms (org_id, article_id, term text COLLATE "C")`, with PK
  `(org_id, term, article_id)` and an `(org_id, article_id)` index. It has a composite FK to
  `knowledge_articles` with cascade.
- Grams and terms are tagged by kind:
  - `s` + every 4-character window of a lowercased string, padded with three U+0001 so that
    every position starts a window (email and name; article title).
  - `t` + each `show_trgm(name)` trigram: exactly the set `similarity()` compares.
  - `l` + each lexeme of `search_tsv`.
- `name_grams = cardinality(show_trgm(name))`, which the fuzzy bound needs for each candidate.
- AFTER INSERT and AFTER UPDATE OF email/name (title/content) triggers, `SECURITY INVOKER`,
  rebuild a row's grams. The `WHEN` clauses skip updates that change nothing, because
  `update_customer` and `update_article` name every column in their SET lists. The triggers run
  as the writer under RLS, so a pinned `otto_app` transaction writes only its own org's grams.
- Helper functions `search_windows`, `customer_search_grams_of`, and `knowledge_search_terms_of`
  are `IMMUTABLE`, with `search_path` pinned. The trigger, the backfill, and (for
  `search_windows`) the queries all share them.
- The backfill runs at migration time and needs a superuser or `BYPASSRLS` role: that is
  production's connecting role today. Under any other role it would see no rows and silently
  leave existing customers unsearchable, so it raises an exception whenever `customers` or
  `knowledge_articles` holds data.

### Queries

`SEARCH_CUSTOMERS_SQL` (`crates/cxp-core/src/customers.rs`) and `SEARCH_ARTICLES_SQL`
(`crates/cxp-core/src/knowledge.rs`) end in the pre-#17 `WHERE … ORDER BY … LIMIT` unchanged. The
only addition is `AND id = ANY (ARRAY(<candidates>))`, served by `UNIQUE (org_id, id)`. Each
candidate set is a superset of the rows its half of the predicate accepts:

- **Substring (`ILIKE '%q%'`).** On a multibyte database Postgres evaluates `x ILIKE p` as
  `lower(x) LIKE lower(p)`. So every match holds every 4-character window of `lower(q)` as an `s`
  gram, and a `q` shorter than four characters is the prefix of one of the match's windows (a
  byte range under `COLLATE "C"`). The query counts each window's postings up to `DF_PROBE` (256),
  takes the rarest window's rows, and keeps those that also hold the next two rarest windows
  (index probes).
- **Fuzzy (`name % q`).** With |A| the name's trigrams, |B| the query's, and c shared:
  `c / (|A| + |B| − c) ≥ t` gives `c ≥ k(|A|) = t(|A| + |B|) / (1 + t)`, and since `c ≤ |A|`,
  also `c ≥ kmin = ⌈t|B|⌉`. Every match therefore shares one of any `|B| − kmin + 1` query
  trigrams, so only the rarest that many are looked up (rarity from the capped counts, ties broken
  word-initial-last). A customer is a candidate when its count among them reaches
  `k(|A|) − (kmin − 1)`. A slack of 0.001 absorbs pg_trgm's float4 rounding. `t` is read from
  `pg_trgm.similarity_threshold` at query time; when `t ≤ 0` every named customer matches and
  the query falls back to the org's customers.
- **Full text (`search_tsv @@ plainto_tsquery('english', q)`).** With the `english` config,
  `plainto_tsquery` is the AND of the lexemes `to_tsvector('english', q)` yields. The candidates
  are the articles holding all of them (postings grouped and counted). A query of stop words
  alone has no lexemes and matches nothing, as `@@` with an empty query does.
- `crate::without_jit` runs `SET LOCAL jit = off` first. The planner can't see how few rows a
  gram lookup reads, so the generic plan's cost passes `jit_above_cost`, and JIT compilation
  alone took about 300 ms.

`PURGED_TABLES` lists both tables before their parents, so `org.deleted` deletes them in batches
like everything else rather than by cascade.

## Measurements

Data (`scripts/bench-search/seed.sql`, deterministic): org A has 300,000 customers and 300,000
articles (270,000 published, 5-word titles, 150-word bodies drawn Zipf-like from
`/usr/share/dict/words`). Orgs B and C each have 50,000 customers and 20,000 articles; org D has
20,000 customers and 5,000 articles. In total that is 420,000 customers and 345,000 articles.
Names come from 125 first names and 110 surnames (60%) or invented surnames (40%); emails are
`first.lastN@domain`, across 17 domains.

Method (`scripts/bench-search/run.sh`): pgbench, one client, `-M prepared` (as sqlx runs it, so
the plan cache goes generic after five runs), each transaction `SET LOCAL ROLE otto_app` +
`set_config('app.org_id', A)` + the search, `LIMIT 50` (customers) / `20` (articles). The figures
are whole-transaction latency after 6 warm-up runs: 40 runs after, 10 runs before (each run of
the old scan takes 1 to 3 s).

### Customers (org A, 300k)

| query | matches | before p50 / p95 | after p50 / p95 | LEAKPROOF+GIN p50 / p95 † |
|---|---:|---:|---:|---:|
| `zzzz` | 0 | 1974 / 2133 | **4.3 / 5.2** | 0.9 / 1.1 |
| `catherine.straski1@initech.example` | 92 | 2076 / 2377 | 190 / 242 | 28 / 29 |
| `hopper` | 1,567 | 1989 / 2199 | **49 / 64** | 16 / 18 |
| `grace` | 2,438 | 1893 / 2042 | **50 / 53** | 43 / 49 |
| `Grace Hoper` (typo; fuzzy only) | 1,849 | 2182 / 2326 | 85 / 92 | 39 / 44 |
| `grace hopper` | 3,041 | 1610 / 2242 | 86 / 98 | 46 / 49 |
| `lee` (3 chars: prefix path) | 4,038 | 1658 / 1945 | 101 / 110 | 52 / 66 |
| `navy` | 17,861 | 856 / 1736 | 133 / 155 | 85 / 88 |
| `ab` (2 chars) | 23,049 | 1174 / 1839 | 316 / 327 | 1199 / 2165 |

### Knowledge base (org A, 270k published)

| query | matches | before p50 / p95 | after p50 / p95 | LEAKPROOF+GIN p50 / p95 † |
|---|---:|---:|---:|---:|
| `zebrafishy` | 0 | 1202 / 2121 | **3.2 / 4.6** | 1.3 / 1.6 |
| `antiprimer` | 539 | 1586 / 2991 | **16 / 22** | ≈ 8 ‡ |
| `arrhythmic` | 1,226 | 2863 / 3041 | **32 / 39** | ≈ 10 ‡ |
| `arbuscula` | 3,793 | 1220 / 1441 | 69 / 90 | ≈ 39 ‡ |
| `password reset` | 4,577 | 2979 / 3013 | 133 / 142 | 52 / 80 |
| `Angiasthenia Aleurones` (title words) | 5,787 | 1480 / 2016 | 212 / 291 | — |
| `aulicism` | 12,412 | 1895 / 2685 | 204 / 248 | — |
| `acc` (3 chars: title prefix path) | 12,907 | 1117 / 1374 | 210 / 239 | 52 / 62 |
| `account` | 42,148 | 1285 / 1490 | 652 / 756 | 294 / 325 |

† The LEAKPROOF figures come from the same scripts, measured on this container with the three
functions marked LEAKPROOF and GIN indexes `(org_id, email gin_trgm_ops)`,
`(org_id, name gin_trgm_ops)`, `(org_id, search_tsv)`, and `(org_id, title gin_trgm_ops)`
(btree_gin), then reverted. ‡ These were measured on an earlier article corpus with the same
volume and a different vocabulary, on words of similar document frequency, so they are
indicative only.

What remains slow is the work the result size requires, not finding the rows. A query matching
N rows must fetch and rank all N (`ORDER BY similarity` / `ts_rank` over the whole match set),
and so must the GIN plan. GIN fetches in physical order from a bitmap; this design fetches by id,
which costs about twice as much per row. The email query is the outlier: its 35 trigrams fuzzy-
match every "… Straski" customer, so about 18k candidates need a heap recheck.

### Storage and write cost (420k customers / 345k articles)

| | rows | heap + PK + FK index | per entity | LEAKPROOF+GIN equivalent |
|---|---:|---:|---:|---:|
| `customer_search_grams` | 22.2M (53 per customer) | 2.86 GB | ≈ 6.8 kB | 49 MB (two trigram GINs) |
| `knowledge_search_terms` | 57.1M (165 per article) | 7.7 GB | ≈ 22 kB | 200 MB (tsvector + title GIN) |
| for comparison: `customers` / `knowledge_articles` | | 200 MB / 1.28 GB | 0.5 kB / 3.7 kB | |

Most of the size is per-row overhead: a heap tuple header and a full btree entry for every
(entity, gram) pair, where GIN stores compressed posting lists. At today's production volumes
(tens of customers) this is negligible. At 10k customers it is about 70 MB; at 2k articles, about
45 MB. Writes: inserting a customer goes from 0.02 ms to 1 ms (53 grams), and inserting an
article from 0.4 ms to 4 ms. Updates that change no searchable text cost nothing extra.

## Invariants impact

- **Append-only migrations:** a new `0004`; nothing applied is edited.
- **Additive, previous release compatible:** the old binary never reads the new tables, but its
  writes fire the triggers, so the index stays correct while it runs (`raw_sql_writes_are_indexed`).
  Its deletes cascade.
- **Tenant isolation:** both tables have `org_id`, enable and force RLS, carry
  `<table>_tenant_isolation`, and are granted to `otto_app`. Composite `(org_id, x_id)` FKs
  cover them. They are added to every check in `tests/db.rs` (isolation report, cross-org,
  unpinned, maintenance marker, composite FKs, grants) and to the purge tests in
  `tests/lifecycle.rs`.
- **No bypass:** the triggers are `SECURITY INVOKER`, nothing is `SECURITY DEFINER`, and nothing
  is marked LEAKPROOF.
- **No SQL outside cxp-core; runtime `query`/`query_as` only:** unchanged.
- **Untrusted text:** queries are bound parameters. Gram text is derived data that never reaches a
  response.
- **Small shared database:** see Storage. No unbounded lists: results are still `LIMIT`ed, and
  candidate arrays are bounded by the org's own rows.
- **No GIN index:** `tests/db.rs` still forbids them, and none is added.

## Migration

`0004_search_index_tables.sql`: functions, tables, policies, triggers, an RLS-role guard, a
backfill, and grants. It runs at boot under sqlx's advisory lock. On production volumes the
backfill takes milliseconds. On large data it is slow, because of row-by-row FK checks
(`seed.sql` loads in bulk instead).

## Out-of-band

- None needed for this design: no new config, secret, or scope.
- **Ordering with #16 (least-privilege role):** the backfill needs RLS-exempt privileges. If #16's
  role switch reaches production before this migration runs, the migration refuses to apply and
  the deploy stops at boot. Either deploy this first, or exempt the role for one boot (Human
  hand-off 2).

## Error handling & edge cases

- An empty or whitespace query is `Invalid`, as before. The knowledge base keeps its
  200-character cap.
- Queries under four characters take the prefix path (a range scan over every window starting
  with them). That is correct, but it is as slow as the match set is large (`ab`: 316 ms).
- `pg_trgm.similarity_threshold` is honoured, including 0 (fallback to the org's customers).
- A NULL name has no grams and never fuzzy-matches, as `%` returns NULL.
- Non-ASCII text, LIKE wildcards, and the escape character are covered by the oracle tests.
- A `pg_trgm` upgrade that changed `show_trgm` output would leave stored `t` grams stale. The
  oracle tests catch that in CI; the fix is a migration that rebuilds the table.

## Testing approach

Oracle tests compare each indexed search with the pre-#17 SQL, run in the same transaction, and
repeat each set so most comparisons run on the generic plan:

- `tests/customers.rs`: 60 queries over a mixed corpus in two orgs, five thresholds, and 300
  near-identical generated names at three thresholds. Writes and raw-SQL writes are covered too.
- `tests/support.rs`: 54 article queries (stemming, stop words, hyphens, URLs, non-ASCII, title
  substrings of every length), and edits and deletes.

Mutation checks were run by hand: disabling the short-query path, removing the fuzzy slack, or
probing one trigram too few each fail these tests. Isolation and purge are covered as listed
under Invariants.

## Risks & open questions

- **The ~50 ms criterion is not met for broad queries** (thousands of matches), either here or
  with LEAKPROOF+GIN. Reaching it would need a semantic change: a bounded top-k without exact
  global ranking, or prefix-only search. That is Rob's call and is not done here.
- **Storage** at large-org scale is 15 to 40 times the GIN alternative. If Rob prefers LEAKPROOF,
  a follow-up migration can drop both tables and their triggers, and these queries revert to the
  plain predicate plus GIN indexes.
- The backfill's role guard ties deploy order to #16 (see Out-of-band).

## Human hand-off (Rob)

1. **Decide whether to keep this design or switch to LEAKPROOF + GIN.** Neither is needed before
   merge: this change is correct and safe as it stands.

   LEAKPROOF, if chosen, needs a superuser connected to the `otto_cxp` database only:

   ```sql
   -- fly postgres connect -a otto-db -d otto_cxp   (as postgres)
   ALTER FUNCTION pg_catalog.texticlike(text, text) LEAKPROOF;      -- ILIKE
   ALTER FUNCTION public.similarity_op(text, text) LEAKPROOF;       -- pg_trgm %
   ALTER FUNCTION pg_catalog.ts_match_vq(tsvector, tsquery) LEAKPROOF; -- @@
   ```

   It then needs a migration that adds `btree_gin` and four GIN indexes and drops this change's
   tables, plus a change to `tests/db.rs`'s "no GIN index" rule. Security reasoning:
   - **What "leakproof" promises.** The function reveals nothing about its arguments except
     through its return value. Under RLS it may then run on rows the policy has not yet filtered
     out, here other orgs' rows that a GIN index returns. The usual leak is an error, a notice,
     or a timing difference that depends on the row's value.
   - **`texticlike`.** Its errors (invalid escape, pattern ending in the escape character) depend
     only on the pattern, which is the caller's own query. No error depends on the stored text.
   - **`similarity_op`.** It builds trigrams of both strings and compares counts. It raises no
     data-dependent error.
   - **`ts_match_vq`.** It evaluates a parsed query against a stored vector and raises no
     data-dependent error.
   - **Timing.** Timing differs with row content for all three, but leakproofness never covered
     timing, and the bitmap or index scan's own timing already varies with other orgs' data.
   - **Residual risks.**
     - Core Postgres has not audited these functions as leakproof.
     - A future Postgres or pg_trgm release could add a data-dependent error path.
     - The flag lives in this database's `pg_proc` only, so it is lost on dump/restore, a
       `pg_upgrade` to a new major version, or an extension upgrade. Search would then silently
       become slow again (never wrong). A boot check could detect that.
     - It affects only `otto_cxp`, not the other databases on `otto-db`.
2. **Deploy order with #16.** Today the connecting role is a superuser, so 0004 backfills on its
   first boot with nothing to do by hand. If #16 has already switched production to a role that
   RLS binds, 0004 refuses with `migration 0004 must run as a superuser or BYPASSRLS role …` and
   the deploy fails its readiness check (the previous release keeps serving). Then, as a
   superuser on `otto_cxp`, lift the exemption for that one boot:

   ```sql
   ALTER ROLE <cxp connecting role> BYPASSRLS;   -- before redeploying
   -- redeploy; confirm "applying migrations" and the boot lines in fly logs
   ALTER ROLE <cxp connecting role> NOBYPASSRLS; -- right after
   ```

   Simplest: let 0004 deploy before #16 switches the role.
