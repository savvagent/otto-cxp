# Web console reads camelCase API responses — design

**Status:** DRAFT
**Issue:** #15

## Brief

> The web console's types and readers match the API's JSON, so list and detail pages show
> timestamps and catalog references from real API responses.

Acceptance criteria (from #15):

1. `apps/web/src/lib/types.ts` and every reader match the API's camelCase JSON.
2. List and detail pages show timestamps and catalog references from real API responses.

## Premise corrections

- **Not every response is camelCase.** `GET /api/v1/dashboard/metrics` deliberately keeps
  snake_case keys (`crates/cxp-api/src/routes/dashboard.rs`), so `DashboardMetrics` and the
  dashboard page are already correct and stay as they are.
- **Request bodies and query parameters are snake_case, and the console already sends them that
  way.** The API deserializes `customer_id`, `team_id`, `experience_id`, `assigned_to` and so on
  (`crates/cxp-api/src/routes/*.rs`). Only response *models* are camelCase. No writer changes.
  (The catalog routes also accept `teamId` as an alias; the console does not need it.)
- **Customer 360 is not made of full rows.** `recentTickets` and `recentFeedback` are
  `TicketSummary` / `FeedbackSummary` (`crates/cxp-core/src/customers.rs:44-76`), subsets of
  `Ticket` / `Feedback`. `types.ts` typed them as the full models.
- **The list envelope was already right:** `{data, total, limit, offset}`
  (`crates/cxp-api/src/routes/mod.rs`). `search` and survey `responses` return plain arrays,
  which the API client already expects. `MyTeam` (`id`, `slug`, `name`) is unaffected.

## Assumptions

- Form-field names inside the console's zod schemas (`team_id`, `component_type`, ...) are
  request-side form state, so they stay snake_case; they are read from a response row as
  `editing.teamId` and sent as `team_id`.
- Drift protection: a Rust test in `cxp-api` serializes one value of every model the console
  decodes (and the list envelope) and compares it to a committed fixture; the web tests load the
  same file. This is proportionate: no DB, about 250 lines of Rust, and it makes a renamed field
  on either side fail a test.
- `vitest` is added to `apps/web` (already used in `sdk/typescript`); no DOM test environment, so
  there is no component render test. The readers are plain property accesses checked by `tsc`
  against the fixture-typed models.
- No auth is added to the console (#14).

## Goal & Success Criteria

`pnpm build` (`tsc -b`) passes with camelCase types, and no `apps/web` code reads a snake_case
property off a response. The fixture the Rust side generates type-checks as the TS interfaces
(`satisfies`), has exactly the interfaces' keys, and contains no snake_case key outside
`dashboardMetrics` and caller-defined JSON (`metadata`, `answers`, `questions`).

## Scope

In: `apps/web/src/lib/types.ts`, readers in `apps/web/src/pages/**`, the new test and fixture,
`vitest` in `apps/web`, and a stale comment in `crates/cxp-api/src/routes/catalog.rs`.
Out: auth (#14), request-body casing, server model changes, the SDK and `packages/react`.

## Architecture

- `apps/web/src/lib/types.ts`: response interfaces become camelCase; adds `TicketSummary` and
  `FeedbackSummary`; `Customer360` uses them; `DashboardMetrics` keeps snake_case with a comment.
- Readers: `pages/catalog/{experiences,applications,pages-page,components-page}.tsx`,
  `pages/customers/{index,customer-detail}.tsx`, `pages/tickets/{index,ticket-detail}.tsx`,
  `pages/feedback/index.tsx`, `pages/surveys/{index,survey-detail}.tsx`.
- `crates/cxp-api/src/routes/mod.rs`: `wire_fixture` test module writing/comparing
  `crates/cxp-api/tests/fixtures/wire.json` (`UPDATE_WIRE_FIXTURES=1` regenerates).
- `apps/web/src/lib/types.test.ts`: loads the fixture.

## Invariants impact

None. No SQL, schema, auth, metering or scope change. New files under `crates/` and `apps/` are
repo content under the existing AGPL licensing; the fixture is data.

## Migration / Out-of-band

None.

## Error handling & edge cases

- Nullable columns stay `T | null`; the fixture populates them, and `satisfies` accepts that.
- A server-side rename fails `cargo test` (fixture mismatch) until the fixture is regenerated,
  and then fails `pnpm build`/`pnpm test` until `types.ts` follows.

## Testing approach

Rust: `routes::wire_fixture::console_payloads_match_the_committed_fixture`. Web: `types.test.ts`
(exact key sets, envelope, no stray snake_case, `satisfies` against each interface, and two
fetch-stubbed api calls reading `createdAt`, `teamId`, and the 360 view's rows). vitest does not
type-check; the `satisfies` assertions are enforced by `tsc -b` in `pnpm build`.

## Risks & open questions

- Optional keys that the server omits (none today: models serialize every field) would need
  `?:` types; the exact-key test would flag it.
