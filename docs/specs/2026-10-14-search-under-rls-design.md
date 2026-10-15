# Search under row-level security — design

**Status:** DRAFT (#17)
**Issue:** #17 "Make customer and knowledge-base search fast under row-level security"
**Plan:** `docs/plans/2026-10-14-search-under-rls.md`
**Decision:** Rob chose option (c) on 2026-10-14: accept a search-semantics change above a cap so
that broad queries are fast too.

## Brief

> Under forced row-level security, Postgres applies only leakproof operators as index conditions
> ahead of the RLS predicate. `ILIKE`, the trigram `%` operator and full-text `@@` are not
> leakproof, so customer search and knowledge-base search scan every row of the caller's org.

Acceptance criteria (#17): search stays under ~50 ms at 300k rows per org as `otto_app` under
forced RLS, and the approach is recorded with its security reasoning.

The first version of this change kept results exactly as before for every query. Selective
queries became fast, but broad ones (thousands of matches) stayed at 130 to 650 ms, because every
match had to be fetched and ranked. LEAKPROOF plus GIN indexes would have been no better on broad
queries. Rob chose option (c). Its binding requirements:

1. p50 under ~50 ms for every benchmark query at 300k rows per org, broad ones included (`ab`,
   `navy`, `account`, an exact email), run as `otto_app` under forced RLS.
2. Semantics:
   - The page is ranked by today's ordering, chosen from the best-ranked candidates.
   - Below the cap (1000 matches), rows, order, and total are identical to today.
   - `total` is exact up to the cap. Above it, `total` is 1000 and a new additive
     `totalCapped: true` is set.
   - Offsets past the cap return an empty page.
   - No query scans or ranks an unbounded number of rows.

   Review of #33 restated the second and third points as the contract below (Semantics), since no
   bounded design can be identical below the cap for every query. Rob is asked to confirm it; this
   change implements it as proposed.
3. The migration needs neither BYPASSRLS nor superuser. A backfill runs per org (maintenance or
   pinned transactions) in bounded batches. A test proves this under a NOSUPERUSER NOBYPASSRLS
   owner.
4. Storage proportionate to a small shared database, well below the first version's 6.8 kB per
   customer and 22 kB per article if possible.
5. No superuser change unless it is genuinely needed. If it is, it becomes a hand-off.
6. The usual tenant-table invariants.

## Premise corrections

Measured as `otto_app` with forced RLS. These are unchanged from the first version:

- **The RLS predicate itself is not the cost.** `current_org()` is inlined. The planner merges
  `org_id = current_org()` with the query's `org_id = $1` and evaluates the setting once.
  Counting an org's 300k customers is an index-only scan (about 25 ms).
- **The cost is evaluating the text operators on every org row.** On 300k customers:
  - `email ILIKE` alone takes about 290 ms.
  - `name % q` alone takes about 480 ms.
  - All three operators together take about 1 s.
- **Knowledge-base search was worse than the ticket said:** 1 to 1.7 s at 300k articles. Most of
  that is de-TOASTing every `search_tsv`.
- **LEAKPROOF is per database.** `ALTER FUNCTION … LEAKPROOF` in `otto_cxp` changes only that
  database's `pg_proc`. The other services' databases are not affected.

New with option (c):

- **"Identical below the cap" and "bounded work" cannot both hold for every query, under any
  index.** Two kinds of query need unbounded reads to be exact:
  - A fuzzy (`%`) query with many trigrams. A long query needs many shared trigrams, but it can
    be built only from common ones.
  - A several-word full-text query whose words are each common while their AND is rare.

  For both, proving that fewer than 1000 rows match means reading posting lists whose length
  grows with the org. GIN does the same, only faster: a GIN `%` lookup for `ab` read 53k
  candidates and took 106 ms. This design is exact below the cap whenever its reads finish
  within their bounds, which covers every sub-cap query in the benchmark except one (the exact
  email; see Semantics). When a read bound cuts a search short, the response says so with
  `totalCapped: true`.
- **`search_customers` and `search_knowledge_base` returned no total before.** The REST route
  `GET /api/v1/customers/search` returns a bare JSON array. A JSON field cannot be added to it
  without breaking it, so the count rides in headers (Assumptions).

## Assumptions

- "300k rows per org" applies to customers and articles alike. A real knowledge base holds 10² to
  10⁴ articles, so 300k articles is a stress case.
- Benchmark host:
  - A laptop running Postgres 16.13 in podman, `shared_buffers = 4GB`, warm cache.
  - One client, prepared statements, no parallel workers.
  - Other agents' test runs shared the host, at load average 3 to 11. That inflates the "before"
    figures most.
  - Production (`otto-db`, a shared small machine) will be slower in absolute terms.
- `pg_trgm.similarity_threshold` stays at its default of 0.3 in production. Other thresholds are
  supported and tested.
- **When a read bound is hit below the cap, `totalCapped` is true and `total` is a lower bound**
  (the matches found). Rob's rule "past the cap, total is 1000 and totalCapped is true" is kept
  exactly. A capped total below 1000 means that the search stopped before counting every match,
  and the tool descriptions say so.
- Production runs Postgres 18. The numbers below were measured on 18.6, and again on 16.13.
- REST: `GET /api/v1/customers/search` keeps its bare array body. It gains `X-Total-Count` and
  `X-Total-Capped` headers. The web console's command palette reads the array and only shows the
  first five, so neither its types nor `crates/cxp-api/tests/fixtures/wire.json` change.
  `@cxp/sdk`'s `callList` returns `items` and ignores the new fields.
- There is no REST knowledge-base search, so only the MCP tool gains `total` and `totalCapped`.

## Goal & success criteria

| Requirement | Result |
|---|---|
| 1. p50 < 50 ms, every benchmark query | Met. On Postgres 18 the worst p50 is 31.7 ms and the worst p95 32.6 ms (`grace`); on 16, 31.3 / 32.3 ms (Measurements) |
| 2. Exact below the cap | Met as the contract states it: `totalCapped: false` always means identical. Tested on fixed and random corpora (Testing). In the benchmark, `zzzz`, `antiprimer`, and `zebrafishy` are exact; the exact-email query is not, and says so (Semantics) |
| 2. Total / `totalCapped` / offsets / bounds | Met, and tested (`search_counts_up_to_the_cap`, `article_search_counts_up_to_the_cap`) |
| 3. No superuser, per-org batches, proven | Met. The migration reads no tenant rows; `the_index_is_built_per_org_under_a_least_privilege_owner`; the e2e least-privilege test passes |
| 4. Storage well below 6.8 / 22 kB | **Not met.** 7.1 kB per customer, 25 kB per published article (Storage). Reaching it needs GIN, and so LEAKPROOF |
| 5. No superuser change | Met. LEAKPROOF is an optional hand-off, not needed for the latency target |
| 6. Invariants | Met (Invariants impact) |

## Semantics

Customers (`search_customers`, MCP and REST) and articles (`search_knowledge_base`):

- **Predicate and order are today's.**
  - Customers: `email ILIKE '%q%' OR name ILIKE '%q%' OR name % q`. Email matches first, then
    by `similarity` descending, then newest first, then by id.
  - Articles: published, and `search_tsv @@ plainto_tsquery('english', q) OR title ILIKE '%q%'`.
    By `ts_rank` descending, then newest first, then by id.
- **The contract** (from the review of #33; it replaces "identical for any query below the cap"):
  - **`totalCapped: false`** means items, order, and total are identical to today's search.
  - **`totalCapped: true`** means `total` is a lower bound, and the items are the best-ranked of
    what was found.
- **`total`** counts matches up to 1000. `totalCapped` is true when more than 1000 rows match
  (`total` is then 1000), or when the search stopped before counting every match (`total` is then
  the matches it found).
- **The page** is the requested page of the matches found, in today's order. When the search
  stopped early, the matches found are the first 1001 in candidate order:
  - customers: the newest substring matches (when the substring read was cut; see below), then
    the substring candidates from the index (in index order, so effectively arbitrary), then
    fuzzy candidates with the most shared rare trigrams, then not-yet-indexed rows;
  - for one-word full-text queries, the most occurrences first;
  - for several words, the rarest word's articles with the most occurrences first, then title
    matches.
- **Offsets:** a customer page starting at or past 1000 is empty, with `total` 1000 and
  `totalCapped` true. A page crossing 1000 is cut there. Article search has no offset (20 rows).
- **Bounds, per query** (`crates/cxp-core/src/search.rs`):

| Step | Bound |
|---|---|
| Query length | 256 characters (`MAX_QUERY_CHARS`, both searches); longer is `invalid_argument` |
| Statement time | `statement_timeout = 2s` for the rest of a search's transaction, a backstop |
| Rarity probes | 256 index rows per query window, trigram, or lexeme |
| Substring candidates (customers; article titles) | 2,000 index rows (the rarest window's, or a short query's prefix range), each checked against at most two more windows |
| Newest customers, when the substring read is cut | 20,000 customers newest first, each checked with `ILIKE`, until 1,001 match |
| Fuzzy candidates | 10,000 index rows of the rarest needed trigrams; 1,500 candidates checked (500 once the read is cut) |
| Full-text candidates | 2,000 index rows of the rarest lexeme, probed for the others |
| Not-yet-indexed rows | 200 read directly; 50 customers or 10 articles indexed per search |
| Rows checked against the table | until 1,001 match, at most the sum of the candidate bounds |
| Searchable text | email ≤ 320, name ≤ 255, title ≤ 500 characters, checked in `cxp-core` on create and update |
| Rows ranked | at most 1,001 (customers); for articles, the matches up to 1,001, ranked by `tf` with no `ts_rank`, or up to 1,000 (200 past the cap) by `ts_rank` for several words |

Matches against today on the benchmark (Postgres 18; the page compared is the first, by ids and
order):

| Query | Matches | `total` | `totalCapped` | Page overlap with today |
|---|---:|---:|---|---:|
| `navy` | 17,861 | 1000 | true | 4 / 50 |
| `grace` | 2,438 | 1000 | true | 21 / 50 |
| `hopper` | 1,567 | 1000 | true | 33 / 50 |
| `grace hopper` | 3,041 | 512 | true | 17 / 50 |
| `Grace Hoper` | 1,849 | 485 | true | 10 / 50 |
| `catherine.straski1@initech.example` | **92** | **7** | true | 6 / 50 (the email itself is first) |
| `zzzz` | 0 | 0 | false | identical |
| `lee` | 4,038 | 1000 | true | 29 / 50 |
| `ab` | 23,049 | 1000 | true | 9 / 50 |
| `password reset` | 4,577 | 115 | true | 6 / 20 |
| `account` | 42,148 | 1000 | true | **identical** |
| `aulicism`, `arbuscula`, `arrhythmic`, `acc` | 1,226 to 12,907 | 1000 | true | **identical** |
| `antiprimer` | 539 | 539 | false | identical |
| `Angiasthenia Aleurones` | 5,787 | 316 | true | 9 / 20 |
| `zebrafishy` | 0 | 0 | false | identical |

**Broad customer pages.** Today's order for a broad customer query is email matches first, then
name similarity, then newest first. The review settled on not adding `created_at` to the gram
table key: it costs 16 to 24 B on every index row and still would not reproduce that order. It
asked instead for a bounded newest-first fallback, kept only if every benchmark query stays under
50 ms p50 and it cannot set `totalCapped: false` on an incomplete answer. Both hold, so it is kept:

- When the substring read hits its 2,000-row bound, `sub_new` reads the org's customers newest
  first through `(org_id, created_at DESC)`, checks each with `ILIKE`, and stops at 1,001 matches
  or 20,000 rows. Its matches come before the index candidates.
- `totalCapped` stays true unless that read covered the whole org (an index-only count of at most
  20,001 rows says so). In an org of up to 20,000 customers, a broad substring query below the
  cap is therefore exact; before, it was capped with an arbitrary page.
- Cost at 300k customers: 10 to 20 ms on the queries that trigger it (`navy` 6.6 → 24.7 ms,
  `grace` 10.1 → 31.7, `lee` 7.6 → 27.4, `ab` 8.5 → 20.8 on Postgres 18 vs the reviewed
  version on 16).
- Page agreement at 300k improves modestly (`navy` 1 → 4 of 50, `ab` 0 → 9) and moves a little
  either way elsewhere (`grace` 23 → 21, `lee` 31 → 29, `hopper` 28 → 33). Where similarity
  decides the order, the best matches are spread across all of them, and no bounded read finds
  them.

One-word article queries keep today's page because `ts_rank` is a function of the word's
occurrences, which the index holds, and a partial index serves those occurrences in descending
order.

**The one sub-cap benchmark query that is not exact is the exact email.** Its 91 fuzzy name
matches ("Catherine Stra…", similarity ≥ 0.3 to the whole email) share trigrams with the query
only through common lists. Finding all of them means reading about 82k index rows, which no
bounded read reaches. The customer with that email is still found exactly and listed first.
`totalCapped: true` and `total: 7` say that the count is incomplete. The LEAKPROOF hand-off
below is the way to make this class exact (33 ms measured).

## Scope

In:
- migration 0004, rewritten;
- `cxp_core::search` (new), `SEARCH_CUSTOMERS_SQL`, and `SEARCH_ARTICLES_SQL`;
- `SearchPage` returned by `search_customers` and `search_articles`;
- the MCP `List.totalCapped` field and tool descriptions, and the REST headers;
- tests, `scripts/bench-search/`, this spec, and the plan.

Out:
- `resolve_customer`'s substring fallback (it stops at 2 rows);
- `list_*` queries (already index-served);
- LEAKPROOF and GIN (a hand-off);
- a creation-time column in the customer index (decided against in review; see Semantics);
- storage reduction and retiring the backlog path: [#34](https://github.com/savvagent/otto-cxp/issues/34);
- moving search responses into the `Paginated` envelope with `totalCapped`: #24.

## Architecture

### Why not the obvious designs

| Design | Result |
|---|---|
| GIN trigram / full-text index | Its operators are non-leakproof, so RLS never lets it be an index condition. With LEAKPROOF it is exact and fast on long fuzzy queries, but unbounded on short ones (`ab` fuzzy: 106 ms) and on broad ones |
| A capped scan of the base table, no index tables | Selective queries need the whole org scanned to be exact: about 1 s |
| Bloom signatures per row | Still a scan of every org row: 160 ms to 4.6 s |
| A table without RLS, `SECURITY DEFINER`, or the search inside a policy | Bypasses or bends RLS. Rejected |
| Name-side prefix filtering (ranked trigrams per name) for exact long fuzzy queries | Prototyped: the email query took 65 to 90 ms, and storage went up. Rejected |
| **Inverted index in tenant tables, read with leakproof btree conditions, bounded reads, counted to a cap** | Chosen |

### Schema (`crates/cxp-core/migrations/0004_search_index_tables.sql`)

- **`customer_search_grams (org_id, customer_id, gram text COLLATE "C", name_grams smallint)`**
  - PK `(org_id, gram, customer_id) INCLUDE (name_grams)`, plus an `(org_id, customer_id)` index.
  - Composite FK to `customers` with `ON DELETE CASCADE`.
- **`knowledge_search_terms (org_id, article_id, term text COLLATE "C", tf smallint)`**
  - PK `(org_id, term, article_id) INCLUDE (tf)`, plus an `(org_id, article_id)` index.
  - Partial index `(org_id, term, tf DESC, article_id) WHERE tf >= 2`, which is about one lexeme
    row in nine.
  - Composite FK to `knowledge_articles` with cascade.
  - Holds published articles only, so counts need no table read.
- **Grams by kind:**
  - `s` + every 4-character window of a lowercased string, padded with three U+0001 so that every
    position starts one. These come from email and name, or from the article title.
  - `t` + each `show_trgm(name)` trigram.
  - `l` + each `search_tsv` lexeme, with `tf` = its number of positions.
- **The `search_indexed` flag on `customers` and `knowledge_articles`.**
  - `ADD COLUMN … DEFAULT false` gives every existing row `false` without reading or rewriting
    one (a catalog default).
  - `SET DEFAULT true` and a BEFORE trigger make every later write `true`.
  - Partial indexes `WHERE NOT search_indexed` hold the backlog: `(org_id, id)` on customers,
    `(org_id, published, id)` on articles, so search reads only the published backlog. They are
    empty once it is done.
- **AFTER triggers (`SECURITY INVOKER`) rebuild a row's grams** on insert, on a change of
  email/name or title/content/published, and when a row is first indexed. They run as the
  writer under RLS.
- Tenant-table rules apply to both tables: forced RLS, `<table>_tenant_isolation`, grants to
  `otto_app`, and `PURGED_TABLES`.
- Both tables vacuum after 2% new rows (`autovacuum_vacuum_insert_scale_factor = 0.02`), which
  keeps their visibility maps current and the lookups index-only.
- `name_grams` is clamped to 32767 in the trigger. Names are capped at 255 characters in
  `cxp-core`, so only a row written some other way could reach it, and a clamped count only
  loosens that row's fuzzy bound.

### No backfill in the migration; the backlog per org

Under #16 the migrating role (`cxp_server`) is bound by RLS and sees no tenant rows, so 0004
reads none. It only adds structure.

Rows that predate 0004 are indexed in each org's own pinned transaction, through
`cxp_core::search::index_backlog`:
- It flips `search_indexed` for up to N rows chosen in a CTE (`FOR NO KEY UPDATE SKIP LOCKED`),
  and the triggers do the indexing. That lock does not conflict with the `FOR KEY SHARE` a
  foreign-key insert takes, so `create_ticket` or `record_interaction` on those customers does
  not wait for the search.
- Every search first indexes a batch of its org's backlog (50 customers or 10 articles; published
  articles first) under a savepoint. If the batch fails, it is rolled back to the savepoint and
  logged at `warn` with its SQLSTATE only, and the search goes on.
- It then reads the rest directly (up to 200 rows; for articles, published ones only) and checks
  each against the full predicate, so search is complete during the transition.
- A `cxp:read` request therefore writes: only its own org's index rows and flags, which no caller
  sees, so the tools keep `read_only_hint`.

Production holds tens of customers per org, so each org's first search indexes everything. An
org's backlog is never touched by another org (RLS). No maintenance policy is added.
`the_index_is_built_per_org_under_a_least_privilege_owner` proves the path end to end.

### Queries

`SEARCH_CUSTOMERS_SQL` (`crates/cxp-core/src/customers.rs`) returns one row
`(total, capped, ids)`. A second statement fetches the page's rows by id.

1. **Substring.** The query's windows are counted (capped at 256) to find the rarest. Up to
   2,000 of its index rows are read; a query under 4 characters reads a prefix range instead.
   Candidates are those holding the next two rarest windows too.
2. **Fuzzy.** The rarest `|B| − kmin + 1` trigrams are read, rarest first, up to 10,000 index
   rows:
   - `kmin` is the larger of `t|B|` and `k(2)`, because a matching name has at least two
     trigrams.
   - When the read finishes, the count bound `p ≥ k(|A|) − (kmin − 1)` selects candidates, with
     0.001 slack for float4.
   - Up to 1,500 candidates are checked, most shared trigrams first (500 when the read was cut).
   - A threshold of 0 or below makes the org's customers the candidates.
3. **Newest first.** Only when step 1's read hit its bound: `sub_new` (Semantics, "Broad
   customer pages").
4. **Backlog.** Unindexed rows, up to 200.
5. **`matches`.** Candidates are checked against the original predicate, substring first, until
   1,001 match.
   - This is a `UNION ALL` under a `LIMIT`: Postgres runs its branches in order and stops when
     the limit is reached. A broad substring query never computes its fuzzy candidates.
   - The capped flags are an `OR` chain that short-circuits the same way.
6. **The page** is `matches` in today's order, cut at 1000.

`SEARCH_ARTICLES_SQL` (`crates/cxp-core/src/knowledge.rs`):

- **Full text.** The rarest lexeme's rows are read, `tf >= 2` first in `tf` order (the partial
  index), then the rest, up to 2,000 (`fts_read`). Probing the other lexemes makes a candidate an
  exact `@@` match (`fts_hits`).
- **Title.** Windows are read as for customers and checked with `title ILIKE` on the row. Only
  `fts_hits` is excluded here, not all of `fts_read`: an article with the rarest lexeme that lacks
  another is no full-text match, yet may match by title (review of #33: `set password` against
  "Reset password").
- **Backlog.** Read directly and checked with the whole predicate.
- **Ranking.**
  - One lexeme (`numnode = 1`): ranking needs no `ts_rank`. For a single lexeme with default
    weights it is `Σ_{j<tf} 0.1/(j+1)² / 1.6449`, strictly increasing in `tf`. An article without
    the lexeme ranks 0, below all with it. Every row read whose `tf` ties the page's last is
    ranked with its creation time.
  - Several lexemes: `ts_rank` on every match up to the cap, or on the 200 with the highest
    `tf` of the rarest lexeme past it. Title-only matches are ranked with `ts_rank` too, as the
    scan ranks them: it scores an article holding some of the lexemes above zero.

Both start with `SET LOCAL jit = off`, because the generic plans' cost estimates pass
`jit_above_cost` and compiling took about 300 ms, and `SET LOCAL statement_timeout = '2s'`.
`fp` reads `pg_trgm.similarity_threshold` with `current_setting`, which knows the setting only
once pg_trgm is loaded; `fp` evaluates `show_trgm` (through `fq`) first, which loads it.

Two plan pitfalls are fixed and recorded in comments:
- Prefix-range bounds must be expressions on the bind parameter, not a CTE column. Otherwise
  the generic plan scans the org's whole index (6.7 s for `lee`).
- The tf of a title candidate is a scalar subquery, not a lateral join. Otherwise it is
  hash-joined against the whole lexeme list.

## Measurements

Data: `scripts/bench-search/seed.sql`, deterministic.
- Org A has 300,000 customers and 300,000 articles (270,000 published).
- Orgs B and C have 50,000 customers and 20,000 articles each; org D has 20,000 and 5,000.
- In total: 420,000 customers and 345,000 articles.
- The search index is bulk-loaded. Its btrees are about 25 to 30% denser than indexes grown a
  row at a time, as production's will be, so production's index is that much larger per row.

Method: `scripts/bench-search/run.sh`. pgbench runs each search as a request does: one
transaction as `otto_app` pinned to org A, the settings, the backlog statement under its
savepoint, the search, and the row fetch. Each search runs on its generic plan after warm-up. The
figures are p50 / p95 in ms: 60 runs after, 10 runs before (each run of the old scan takes about a
second).

**Postgres 18.6** (production's major version), in podman, `shared_buffers = 4GB`, warm cache,
host load average 1 to 2. pgbench 18 runs inside the container over the unix socket
(`CONTAINER=… run.sh`): host pgbench 16 through podman's pasta port forward added a steady
200 ms to every multi-kilobyte result, which the server's own statement log showed was not
server time.

| Customers (org A, 300k) | Matches | Before p50 / p95 | After p50 / p95 |
|---|---:|---:|---:|
| `navy` | 17,861 | 668 / 680 | **24.7 / 25.6** |
| `grace` | 2,438 | 657 / 666 | **31.7 / 32.6** |
| `hopper` | 1,567 | 981 / 1457 | **13.7 / 14.9** |
| `grace hopper` | 3,041 | 805 / 818 | **14.9 / 15.3** |
| `Grace Hoper` (fuzzy only) | 1,849 | 787 / 803 | **13.3 / 14.2** |
| `catherine.straski1@initech.example` | 92 | 1350 / 1351 | **16.0 / 16.6** |
| `zzzz` | 0 | 634 / 652 | **1.8 / 1.9** |
| `lee` (3 chars) | 4,038 | 692 / 700 | **27.4 / 28.4** |
| `ab` (2 chars) | 23,049 | 698 / 704 | **20.8 / 23.4** |

| Knowledge base (org A, 270k published) | Matches | Before p50 / p95 | After p50 / p95 |
|---|---:|---:|---:|
| `password reset` | 4,577 | 966 / 1049 | **8.4 / 10.4** |
| `account` | 42,148 | 1020 / 1123 | **4.7 / 5.8** |
| `aulicism` | 12,412 | 991 / 994 | **3.9 / 4.5** |
| `arbuscula` | 3,793 | 1062 / 1938 | **3.8 / 4.2** |
| `arrhythmic` | 1,226 | 957 / 960 | **3.9 / 5.2** |
| `antiprimer` | 539 | 983 / 988 | **2.8 / 4.2** |
| `Angiasthenia Aleurones` | 5,787 | 1484 / 1981 | **18.6 / 19.5** |
| `acc` (3 chars) | 12,907 | 984 / 987 | **4.4 / 5.6** |
| `zebrafishy` | 0 | 975 / 1282 | **0.7 / 1.0** |

**Postgres 16.13**, same data and method, host pgbench 16 over TCP (no forwarding delay on this
container). "Before" is from the first measurement, at host load average 3 to 11; "after (review)"
is the version reviewed in #33, "after (final)" this one, at load average about 1.5.

| Customers (org A) | Before p50 / p95 | After (review) p50 / p95 | After (final) p50 / p95 |
|---|---:|---:|---:|
| `navy` | 754 / 844 | 6.6 / 8.5 | **24.4 / 25.4** |
| `grace` | 723 / 884 | 10.1 / 11.2 | **31.3 / 32.3** |
| `hopper` | 1135 / 1809 | 13.0 / 13.8 | **12.1 / 13.0** |
| `grace hopper` | 996 / 1515 | 16.2 / 17.8 | **15.2 / 17.8** |
| `Grace Hoper` | 841 / 972 | 14.1 / 14.7 | **13.3 / 14.0** |
| exact email | 2242 / 3477 | 17.8 / 19.1 | **16.8 / 17.6** |
| `zzzz` | 913 / 1186 | 0.9 / 1.0 | **0.9 / 1.1** |
| `lee` | 945 / 1421 | 7.6 / 8.7 | **30.2 / 31.0** |
| `ab` | 1016 / 1423 | 8.5 / 9.4 | **21.7 / 23.2** |

| Knowledge base (org A) | Before p50 / p95 | After (review) p50 / p95 | After (final) p50 / p95 |
|---|---:|---:|---:|
| `password reset` | 1369 / 1889 | 10.0 / 12.5 | **9.1 / 10.5** |
| `account` | 1203 / 1435 | 4.4 / 5.1 | **4.3 / 5.2** |
| `aulicism` | 1089 / 1182 | 4.4 / 5.1 | **4.4 / 5.4** |
| `arbuscula` | 1727 / 1935 | 5.0 / 6.7 | **4.4 / 5.2** |
| `arrhythmic` | 1104 / 1146 | 5.4 / 6.4 | **4.6 / 5.5** |
| `antiprimer` | 1044 / 1065 | 3.1 / 3.7 | **3.1 / 3.7** |
| `Angiasthenia Aleurones` | 1574 / 1887 | 20.6 / 21.5 | **19.1 / 20.1** |
| `acc` | 1037 / 1131 | 5.2 / 6.1 | **5.0 / 6.3** |
| `zebrafishy` | 1009 / 1069 | 0.9 / 1.2 | **1.1 / 1.2** |

For comparison, the first version (exact for every query, unbounded) took 85 to 316 ms on
broad customer queries and 133 to 652 ms on broad article queries.

### Storage and write cost (420k customers / 310.5k published articles; identical on 16 and 18)

| | Rows | Heap | PK | Other indexes | Total | Per row | Per entity |
|---|---:|---:|---:|---:|---:|---:|---:|
| `customer_search_grams` | 22.2M (52.8 per customer) | 1444 MB | 1249 MB | 161 MB | **2.86 GB** | 135 B | **7.1 kB** per customer |
| `knowledge_search_terms` | 51.4M (165 per article) | 3615 MB | 3293 MB | 350 + 278 MB | **7.54 GB** | 154 B | **25 kB** per published article |
| for comparison: `customers` / `knowledge_articles` | | | | | 178 MB / 1290 MB | | 0.4 kB / 3.7 kB |

**This misses the storage aim.** The cost is per row:
- a heap tuple header and two uuids, about 68 B;
- a unique btree entry, about 56 B;
- a deduplicated FK index entry, about 8 B.

Row counts are close to the minimum for exactness: one row per character position for substring
search, one per trigram, one per distinct lexeme. Fewer rows would mean prefix-only matching,
which is not identical below the cap. Smaller rows would mean surrogate integer keys, saving
about 18%. Neither reaches "well below".

GIN stores the same postings compressed, at about 1 to 2 B each: about 49 MB for customers and
200 MB for articles. GIN is usable only with LEAKPROOF (Human hand-off).

These are bulk-loaded sizes; grown a row at a time the btrees run 25 to 30% larger. At today's
production volumes (tens of customers), the cost is negligible. At 10k customers it is about
70 to 90 MB; at 2k articles, about 50 to 65 MB. Watching this, and a size at which to act, is
[#34](https://github.com/savvagent/otto-cxp/issues/34).

Writes, measured on the 420k / 345k database:
- Inserting a customer goes from 0.24 ms to 3 ms.
- Inserting a published article goes from 0.3 ms to about 36 ms. The cold-cache btree inserts
  into multi-GB indexes dominate this; on small tables it is a few ms.
- An update that changes no searchable text costs nothing extra (0.1 ms per row).

## Invariants impact

- **Append-only:** 0004 is new and still unmerged. Nothing on `main` changes.
- **Additive; the previous release keeps working:**
  - It never reads the new tables or columns.
  - Its writes fire the triggers, so the index stays correct (`raw_sql_writes_are_indexed`).
  - Its inserts take the new column's default `true`.
- **Tenant isolation:** both tables have `org_id`, ENABLE and FORCE RLS, a `_tenant_isolation`
  policy, and grants to `otto_app`. Composite `(org_id, x_id)` FKs cover them. They are in every
  check in `tests/db.rs` and in `PURGED_TABLES` (children first, `tests/lifecycle.rs`).
- **No bypass:** the triggers are `SECURITY INVOKER`; nothing is `SECURITY DEFINER` or LEAKPROOF.
  The backlog runs in the org's pinned transaction.
- **Request paths use `begin_live`:** unchanged. A search's backlog batch runs inside it.
  Searches now write (index rows of their own org) as well as read. The MCP tools stay
  `read_only_hint`, since nothing a caller can see changes.
- **No SQL outside cxp-core:** unchanged. Only runtime `query`/`query_as` is used.
- **Small shared database:** every read is bounded (Semantics). The pool is unchanged. Storage
  is discussed above.
- **Untrusted text:** queries are bind parameters. Grams never reach a response. A failed
  backlog batch is logged by SQLSTATE only, never with the database's text, which can quote a
  customer's data.
- **Small shared database:** queries are capped at 256 characters and searchable fields at
  320 / 255 / 500, so neither a query's probes nor a row's index rows can grow without bound, and
  a 2 s statement timeout backstops every search.

## Migration

`0004_search_index_tables.sql` contains:
- the helper functions;
- two tables with FORCE RLS and policies;
- `search_indexed` columns and partial indexes;
- triggers and grants.

It needs no superuser and no BYPASSRLS, and reads no tenant rows. `ALTER TABLE … ADD COLUMN …
DEFAULT false` takes a brief ACCESS EXCLUSIVE lock and does not rewrite the table. The migration
sets `lock_timeout = '5s'` first: behind a long transaction, a waiting ACCESS EXCLUSIVE request
would queue every request on `customers` and `knowledge_articles`, so it fails instead and the
next boot retries. `search_windows` schema-qualifies the built-ins it calls; a `SET search_path`
would stop the planner inlining it. Building the
backlog partial index reads both tables once (milliseconds at production volumes). The
migration runs at boot under sqlx's advisory lock.

## Out-of-band

None. There is no new config, secret, or scope. Deploy order with #16 no longer matters: this
works under `cxp_server` as it is today.

## Error handling & edge cases

- An empty query is `Invalid`, and so is one over 256 characters, in both searches (the knowledge
  base's cap was 200).
- An email over 320, a name over 255, or a title over 500 characters is `Invalid` on create and
  update. The columns are unbounded `varchar`, and the console's forms set no limit, so these
  reject only text no real customer or article has.
- A backlog row that cannot be indexed is read directly by every search, and its batch is retried
  (and logged) by each.
- A search over its 2 s statement timeout fails with `internal` (retriable) and holds no pooled
  connection longer than that.
- Queries under four characters take the prefix path, bounded like the rest.
- `pg_trgm.similarity_threshold` is honored. Thresholds of 0 and below use the org's customers as
  candidates (bounded, so a large org is capped).
- A NULL name has no trigrams and never fuzzy-matches, as `%` returns NULL.
- Unpublished articles have no index rows. Publishing or unpublishing re-indexes.
- A row locked by a writer is skipped by the backlog and indexed by a later search. Meanwhile it
  is read directly.
- A `pg_trgm` upgrade that changed `show_trgm` would leave stored trigrams stale. The equivalence
  tests catch that, and the fix is a migration that rebuilds the table.

## Testing approach

- **Equivalence below the cap** (ids, order, total, `totalCapped == false`) against the pre-#17
  SQL, in `tests/common`'s `customers_both_ways` / `articles_both_ways`:
  - 60 customer queries over a mixed corpus, five thresholds, and 300 near-threshold names at
    three thresholds;
  - 54 article queries.
- **Review regressions**, each seen to fail on the reviewed SQL: `set password` (corpus where
  `set` is commoner than `password`) and `reset account passw` (a title-only match `ts_rank`
  puts above two full matches) in `KB_QUERIES`.
- **Random corpora** against the scan: `random_corpora_search_like_the_scan` (400 customers,
  250 queries, two pages each) and `random_article_corpora_search_like_the_scan` (150 articles over
  a small vocabulary, 250 queries). Uncapped answers must be identical; capped ones must be within
  the contract. About 6 s together in the gate.
- **Every read bound hit:** `a_cut_substring_read_is_exact_in_an_org_search_can_read_whole`,
  `a_cut_substring_read_in_a_large_org_is_capped`, `a_cut_fuzzy_read_is_capped`, and
  `a_backlog_larger_than_its_read_is_capped_until_indexed`: capped, `total < 1000`, a lower bound,
  and every item a match.
- **Backlog robustness:** `a_failing_backlog_batch_does_not_fail_search` (a trigger refuses one
  row; the search still finds it, and the log line has the SQLSTATE and no customer text) and
  `indexing_the_backlog_does_not_block_references_to_it`.
- **Limits:** `search_and_searchable_fields_are_length_limited`,
  `article_titles_are_length_limited`, and the article query cap.
- **Cap behavior:**
  - `search_counts_up_to_the_cap`: 1,100 matches → 1000, capped, ordered page, an edge page of
    10, an empty page past the cap; exactly 1000 matches → exact and uncapped; sub-cap queries
    among them stay exact.
  - `article_search_counts_up_to_the_cap`: one word is still the scan's page; several words give
    matches in `ts_rank` order; sub-cap queries stay exact.
- **Least privilege:** `the_index_is_built_per_org_under_a_least_privilege_owner`:
  - migrations 0001–0003 and seed rows in two orgs, all as a NOSUPERUSER NOBYPASSRLS owner;
  - 0004 as that owner indexes nothing;
  - searches are exact while the backlog is read directly and indexed a batch at a time;
  - batches are bounded and per org;
  - at the end, every row is indexed.
- **Writes, isolation, purge, grants:** as in the first version (`tests/db.rs`,
  `tests/lifecycle.rs`).
- **Wire:** the MCP tests assert `total` and `totalCapped` on both search tools and their absence
  on other lists. The REST test asserts the headers.

## Risks & open questions

- **Long fuzzy queries in large orgs are not exact below the cap.** The exact-email query is the
  example: `totalCapped` says so, and the exact email is still first. The fix is LEAKPROOF (below).
- **Broad customer pages in large orgs are not today's.** The newest-first fallback helps where
  recency decides the order, not where similarity does (Semantics). Adding `created_at` to the
  index key was decided against.
- **The contract restatement** awaits Rob's confirmation (Brief).
- **Storage** is about 30 to 100 times that of GIN. The LEAKPROOF route would let a follow-up drop
  the `t` and `l` rows: most of the article table and a quarter of the customer table.
- **Searches write.** The first search of an org that predates 0004 pays for one backlog batch.
  At production volumes this is a few ms, once. Retiring the backlog path once production has
  none left is [#34](https://github.com/savvagent/otto-cxp/issues/34).

## Human hand-off (Rob)

**Not needed for the latency target, and nothing is needed before merge.** One optional
decision remains: whether to make long fuzzy customer queries exact, and later shrink storage,
by marking pg_trgm's `similarity_op` LEAKPROOF in the `otto_cxp` database. A follow-up change
would then add a GIN `(name gin_trgm_ops)` index and use it for queries with many trigrams. This
change does neither.

```sql
-- as a superuser, connected to otto_cxp only:  fly postgres connect -a otto-db -d otto_cxp
ALTER FUNCTION public.similarity_op(text, text) LEAKPROOF;   -- pg_trgm's %
```

Security reasoning:

- **What "leakproof" promises.** The function reveals nothing about its arguments except through
  its return value. Under RLS it may then run on rows that the policy has not yet filtered out,
  here other orgs' names returned by a GIN index.
- **`similarity_op`.** It builds the trigrams of both strings and compares the counts. It raises
  no error that depends on the data.
- **Timing.** Timing varies with the row's content, but leakproofness never covered timing. An
  index scan's own timing already varies with other orgs' data.
- **Residual risks.**
  - Core Postgres has not audited this function as leakproof.
  - A future pg_trgm release could add a data-dependent error.
  - The flag lives only in this database's `pg_proc`. Dump/restore, `pg_upgrade`, or
    `ALTER EXTENSION pg_trgm UPDATE` would lose it, and search would then fall back to this
    design (never wrong). A boot check could warn when that happens.
  - Nothing outside `otto_cxp` is affected.

Measured on this benchmark with the flag set and a GIN name index (then reverted): the
exact-email fuzzy lookup took 33 ms and was exact, but `ab` took 106 ms (GIN reads every name
starting with "a"). Any follow-up must therefore keep this design for short queries.
