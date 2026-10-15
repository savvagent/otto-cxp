# One casing convention for the `/api/v1` wire format — design

**Status:** IMPLEMENTED (#39)
**Issue:** #24

## Brief

> `/api/v1` now uses three casing conventions: response models serialize camelCase, request
> bodies and query parameters deserialize snake_case (catalog bodies also accept `teamId` as an
> alias), and `/dashboard/metrics` reshapes a camelCase core model into snake_case. Every new
> client (`@cxp/sdk`, `@cxp/react`, the mobile stubs, the console) has to learn the split, and
> #15 was this exact mismatch.

Acceptance criteria (from #24):

1. One documented wire convention for `/api/v1` (likely camelCase throughout), stated in the
   cxp-api route module docs and the resource-server design doc.
2. The change is additive for existing callers: request DTOs accept the old snake_case spelling
   as a serde `alias`, and `/dashboard/metrics` keeps working for current consumers.
3. The wire fixture (`crates/cxp-api/tests/fixtures/wire.json`) and the console types follow the
   convention.

Comment on #24 (from the #17 review, PR #33): `GET /api/v1/customers/search` returns a bare array
and carries `X-Total-Count` / `X-Total-Capped` headers, a third convention beside the list
envelope. When the wire format is unified, move search into the paginated envelope with
`totalCapped`.

Run brief: keep the change additive for existing callers; update `@cxp/sdk`, `@cxp/react` and the
`apps/web` console in the same PR so nothing in-repo reads the old shapes.

## Premise corrections

- **The REST list envelope is `{data, total, limit, offset}`, not `{items, total}`.** `{items,
  total}` is the MCP list tools' shape (`cxp_core::search::SearchPage`, the tool outputs). The
  REST envelope is `routes::Paginated` (`crates/cxp-api/src/routes/mod.rs:22-27`), which the
  console reads as `PaginatedResponse<T>`. Renaming `data` to `items` would break every list
  reader for no casing gain, so search moves into the *REST* envelope: `{data, total,
  totalCapped, limit, offset}`.
- **`@cxp/sdk` and `@cxp/react` do not call `/api/v1`.** Both speak MCP over `/mcp`
  (`sdk/typescript/src/connect.ts`; `packages/react` wraps the SDK); neither contains an
  `/api/v1` path or reads a REST shape. The only in-repo `/api/v1` client is `apps/web`
  (`apps/web/src/lib/api/*.ts`). Nothing in the SDK or React package changes; the PR says so and
  the plan verifies it by search. MCP tool input fields stay snake_case, as the resource-server
  design fixes (`docs/specs/2026-10-14-otto-resource-server-design.md`, "list tools"); `/mcp` is
  not `/api/v1`.
- **Moving search into the envelope is not additive.** A bare array becomes an object. The
  comment on #24 asks for it explicitly, it shipped one day ago (#33), and the console's command
  palette is its only known consumer; this PR changes that reader. It is the one named breaking
  change (see Architecture).

## Assumptions

- **camelCase is the convention**, as the issue suggests: every response model already is, so it
  changes the fewest bytes on the wire and no response reader.
- **The convention covers JSON keys and query-parameter names, not values.** Enum values stay
  as stored (`in_progress`, `page_view`, `nps`); free-form JSON the caller owns (`metadata`,
  `questions`, `answers`) passes through untouched.
- **Exceptions that stay:** error bodies `{"error", "code"}` (single words); the OAuth-shaped
  `401`/`403` bodies `{"error", "error_description"}` from `cxp-http`, whose names RFC 6750 fixes;
  `POST /platform/webhooks` (not `/api/v1`; the platform defines its payload); the console's own
  browser URL query strings (`/console/customers?experience_id=…`, router state, not the API).
- **Old snake_case request spellings are accepted indefinitely** (a serde `alias` costs nothing).
  Sending both spellings of one field is a `400 invalid_argument` (serde's duplicate-field error),
  as `team_id` + `teamId` already is.
- **`/dashboard/metrics` answers both spellings for now:** the six camelCase keys plus the six
  legacy snake_case keys, the latter deprecated. Removing them is #35, filed by this
  run (it needs a window in which no reader uses them; the console stops in this PR).
- **The `X-Total-Count` / `X-Total-Capped` headers go.** Once the count is in the body they are
  the third convention the comment objects to; they shipped with #33 a day ago, and keeping them
  would not save a reader of the bare array from the body change anyway.
- **`GET /surveys/{id}/responses` stays a plain array in this PR.** It is the same envelope
  question as search but was not asked for, and changing it is a second breaking change. The
  module docs name it as the one list without the envelope; #36 tracks moving it.
- Run Profile values all come from the section; none assumed.

## Goal & Success Criteria

`/api/v1` has one documented convention (camelCase keys in request bodies, query parameters and
responses; lists in the `Paginated` envelope), the server accepts every old request spelling, and
the console sends and reads only the new one.

- Every multi-word request field and query parameter in `crates/cxp-api/src/routes/*.rs` is
  camelCase with its snake_case spelling as an `alias`, proven by unit tests that deserialize
  both spellings of each.
- `GET /customers/search` answers `{data, total, totalCapped, limit, offset}`, and no longer sets
  `X-Total-*`.
- `GET /dashboard/metrics` answers the camelCase keys and the legacy snake_case keys with equal
  values.
- `wire.json` pins the search envelope and the dashboard shape; `apps/web` types, readers and
  writers use only camelCase (its tests and `tsc` prove it against the fixture).
- The convention is stated in `crates/cxp-api/src/routes/mod.rs` and in the resource-server
  design doc's REST section.

## Scope

**In:** request DTOs and query structs in `crates/cxp-api/src/routes/{customers,tickets,
feedback,surveys,catalog}.rs`; the search response; the dashboard response; `routes/mod.rs` docs
and wire fixture; `crates/cxp-api/tests/api.rs`; `apps/web/src/lib/{types.ts,types.test.ts,
api/*.ts}` and every caller of those API functions; the resource-server design doc.

**Out:** MCP tool inputs and outputs; `/auth/*` and `/platform/webhooks`; `cxp-core` models and
SQL (no change needed: responses are already camelCase); renaming `data` to `items`; the survey
responses envelope (follow-up); removing the legacy dashboard keys (follow-up); the console's own
browser URL parameters; `sdk/swift`, `sdk/kotlin` (stubs, no REST client).

## Architecture

### Request side (additive)

Each request struct gets `#[serde(rename_all = "camelCase")]`, and every field whose name has an
underscore gets `#[serde(alias = "<snake_name>")]`. Single-word fields are unaffected. Structs and
fields:

| File | Struct | Multi-word fields |
|---|---|---|
| `customers.rs` | `ListParams` | `experienceId`, `applicationId`, `pageId`, `componentId` |
| `customers.rs` | `CreateBody`, `UpdateBody`, `SearchParams` | none (single words; `rename_all` added for uniformity) |
| `tickets.rs` | `CreateBody` | `customerId` |
| `tickets.rs` | `UpdateBody` | `assignedTo` |
| `tickets.rs` | `ListParams` | `customerId`, `experienceId`, `applicationId`, `pageId`, `componentId` |
| `feedback.rs` | `ListParams` | `customerId`, `feedbackType`, `experienceId`, `applicationId`, `pageId`, `componentId` |
| `surveys.rs` | `ListParams` | `experienceId`, `applicationId`, `pageId`, `componentId` |
| `surveys.rs` | `CreateBody`, `UpdateBody` | none |
| `catalog.rs` | `Create/UpdateExperienceBody` | `teamId` |
| `catalog.rs` | `Create/UpdateApplicationBody` | `experienceId` (create), `teamId` |
| `catalog.rs` | `Create/UpdatePageBody` | `applicationId` (create), `teamId` |
| `catalog.rs` | `Create/UpdateComponentBody` | `pageId` (create), `componentType`, `teamId` |
| `catalog.rs` | `ApplicationParams`, `PageParamsByApplication`, `ComponentParams` | `experienceId`, `applicationId`, `pageId` |
| `mod.rs` | `PageParams` | none |

The catalog team field keeps its semantics (`#[serde(default, …, deserialize_with = "present")]`
on update) with the primary name and alias swapped: `teamId`, alias `team_id`. Field names in
Rust stay snake_case. Query strings are parsed by `serde_urlencoded` through `ApiQuery`, which
honours `rename_all` and `alias` (no `flatten` is involved).

### Response side

- **Search** (`customers::search`, `crates/cxp-api/src/routes/customers.rs:80-110`): returns
  `Json<CappedPaginated<Customer>>`, a new struct in `routes/mod.rs`:
  ```rust
  #[derive(Debug, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct CappedPaginated<T: Serialize> {
      pub data: Vec<T>,
      pub total: i64,
      pub total_capped: bool,
      pub limit: i64,
      pub offset: i64,
  }
  ```
  built from `cxp_core::search::SearchPage` and the served `Page`. `TOTAL_COUNT`/`TOTAL_CAPPED`
  constants and headers are removed. **Breaking** for a reader of the bare array (named above).
  `total` keeps its #17 meaning (capped at `TOTAL_CAP`; `totalCapped: true` makes it a lower
  bound, and the page past the cap is empty).
- **Dashboard** (`routes/dashboard.rs`): `Metrics` serializes the camelCase keys and the legacy
  snake_case keys. Shape: a struct holding `#[serde(flatten)] current: cxp_core::dashboard::
  DashboardMetrics` (camelCase, already) and `#[serde(flatten)] legacy: LegacyMetrics` (the six
  snake_case fields, doc-commented as deprecated, removal tracked by the follow-up). Additive:
  current consumers keep their keys.
- **`Paginated`** gains `#[serde(rename_all = "camelCase")]` (no wire change: all single words) so
  the rule holds for any field added later.
- Everything else is already camelCase (the `cxp-core` models, `Team`, `{deleted}`).

### Console (`apps/web`)

- `lib/types.ts`: `DashboardMetrics` becomes camelCase (`totalCustomers`, …); add
  `CappedPaginatedResponse<T>` (`PaginatedResponse<T>` + `totalCapped: boolean`); header comment
  states the one convention.
- `lib/api/*.ts`: every request-body type and query-parameter name camelCase
  (`customerId`, `assignedTo`, `experienceId`, `teamId`, `componentType`, `feedbackType`, …);
  `customersApi.search` returns `CappedPaginatedResponse<Customer>`.
- Callers updated: the dashboard page reads camelCase metrics; the command palette reads
  `.data` from search; every page/hook that builds request objects (`experience_id:` →
  `experienceId:` etc.) uses the new keys. `CatalogFilterValues` (the filter bar's value) becomes
  camelCase; two helpers in `catalog-filter-bar.tsx` map it to and from the pages' *browser* URL
  params, which keep their snake_case names (`?experience_id=`), so existing console links work.
  The ticket detail page's `updateField` sends `assignedTo`. (These two are spread / computed-key
  callers `tsc` does not check, so the plan names them.)
- `lib/types.test.ts`: key lists for `dashboardMetrics` (camelCase, plus a check that the
  fixture's legacy keys equal the snake_case spelling of each and carry the same values) and the
  new `customerSearch` envelope.

### Fixture

`wire_fixture::payloads()` gains `"customerSearch": CappedPaginated::new(SearchPage{…}, page)`;
`dashboardMetrics` is built from the new `Metrics`. Regenerated with `UPDATE_WIRE_FIXTURES=1`.

### Docs

- `crates/cxp-api/src/routes/mod.rs` module doc: the convention (camelCase keys in bodies, query
  parameters and responses; snake_case accepted as aliases on input; the dashboard's legacy keys;
  enum values and caller JSON untouched; lists are `Paginated`, search adds `totalCapped`, survey
  responses are the one plain array pending the follow-up; errors `{error, code}`).
- `catalog.rs` module doc: `teamId`, alias `team_id`.
- Resource-server design doc REST section: replace the "request bodies are snake_case … #24"
  sentences with the convention; also the line in "Out of scope".

## Invariants impact

- Append-only migrations, RLS, composite FKs, `begin_live`, no SQL outside core, metering,
  teams, webhooks, small DB: untouched (no SQL, no new route, no transaction change; search keeps
  its bounded page and capped count).
- **Auth:** every route keeps its scope check; no route added or removed.
- **Untrusted text / DB error text:** unchanged; serde's duplicate-field and unknown-variant
  errors describe the request only (as today via `ApiError::invalid`).
- **Licensing:** new files are docs under `docs/` (repo-level, AGPL).

## Out-of-band

None: no config key, secret, scope, migration, or role change. The deploy is `ci-auto`; the
console ships in the same image, so server and console change together (no window where a new
console talks to an old server while production runs one machine, as it does today; a rolling
deploy across several machines would let a new console reach an old replica, which ignores the
camelCase keys, except a browser holding an old console bundle: an old console's
snake_case writes still work via the aliases; an old console's search palette would see an object
instead of an array until it reloads, and its dashboard keeps working via the legacy keys).

## Error Handling & Edge Cases

- Both spellings of one field in a body (`{"customerId":…, "customer_id":…}`) or query
  (`?pageId=a&page_id=b`): `400 invalid_argument` (duplicate field). Tested for one body and one
  query.
- Repeated identical query key (`?pageId=a&pageId=b`): unchanged behaviour (serde_urlencoded
  duplicate-field error, `400`).
- Unknown keys stay ignored, as today (no `deny_unknown_fields`), so a mistyped camelCase key is
  silently ignored exactly as a mistyped snake_case key was.
- Search with `totalCapped: true` and an offset past the cap: `data: []`, `total` the lower bound.

## Testing Approach

- **Unit (cxp-api, no DB):** a test module in each route file (or one in `routes/mod.rs`)
  deserializing every request struct from the camelCase spelling and from the snake_case
  spelling of every multi-word field, asserting equal values; one duplicate-spelling rejection
  each for a body and a query string (query via `serde_urlencoded::from_str` — check it is a
  dependency, else via `axum::extract::Query::try_from_uri`). The existing catalog team test is
  updated to the swapped primary name.
- **Wire fixture:** regenerated; `console_payloads_match_the_committed_fixture` pins it.
- **Integration (`crates/cxp-api/tests/api.rs`, real Postgres):** search answers the envelope with
  `totalCapped: false` and no `x-total-*` headers; dashboard answers both key sets; a ticket
  created with `customerId` and one with `customer_id` both succeed; a list filtered with
  `?customerId=` and `?customer_id=` both filter; a catalog create with `teamId` still verifies.
  Existing snake_case calls in the file stay as alias coverage. The cross-tenant e2e suite
  (`crates/cxp-server/tests/e2e.rs`, the `CountsOnlyB` arm) asserts the dashboard body exactly
  and gains the camelCase keys; its search case only checks the body for leaked text, which holds
  for either shape.
- **Web:** `pnpm build` (tsc over `src` and `test`) and `pnpm test` (`types.test.ts` against the
  fixture; any API-client test that asserts a URL or body updated to camelCase).
- **Gates:** test, clippy, fmt, migrations-append-only, docker build, pnpm gate (all from the
  section). Tests run against this worktree's own Postgres on `:15461`.

## Risks & Open Questions

- **An external caller of the bare search array breaks.** Accepted per the #24 comment; the PR
  body calls it out. Mitigated by the shape being one day old.
- **#34 is changing `cxp-core` search internals in parallel.** If `SearchPage` changes, the
  `CappedPaginated` constructor adapts on rebase; the wire shape here does not depend on core's.
- **A cached old console bundle.** The console is a PWA (`vite-plugin-pwa`, `registerType:
  "autoUpdate"`, precached app shell), so a browser can run the previous bundle for one session
  after the deploy. In it, writes still work (aliases) and the dashboard still works (legacy
  keys); only the command palette's customer results break: `customerResults.value.slice` throws
  inside its `try`/`finally`, so the palette shows no customers (tickets still show) until the
  new worker takes over. Accepted: no guard is added for a one-session, read-only degradation.
