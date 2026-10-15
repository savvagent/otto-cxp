# Survey responses in the `/api/v1` list envelope

**Status:** DRAFT
**Ticket:** #36

## Brief

From #36 (verbatim):

> ## Problem
> After #24, every `/api/v1` list answers the `Paginated` envelope `{data, total, limit, offset}`
> (search adds `totalCapped`), except `GET /api/v1/surveys/{id}/responses`, which still answers a
> plain array and drops the total that `cxp_core` already computes. It is the one remaining
> exception to the list convention.
>
> ## Acceptance criteria
> - [ ] `GET /api/v1/surveys/{id}/responses` answers `Paginated<SurveyResponse>`
>   (`{data, total, limit, offset}`)
> - [ ] The wire fixture, `crates/cxp-api/tests/api.rs`, and the console
>   (`apps/web/src/lib/api/surveys.ts`, `hooks/use-surveys.ts`, the survey detail page) follow it
>   in the same PR
> - [ ] The `/api/v1` convention docs (`crates/cxp-api/src/routes/mod.rs`, the resource-server
>   design doc) drop the exception
>
> ## Out of scope
> Survey response submission (MCP only).
>
> ## Risks & open items
> - Changing a plain array to an object breaks any out-of-repo reader of this route (Owner: Rob).

## Premise corrections

- "The one remaining exception to the list convention" holds for lists. The convention docs list
  a second, unrelated exception: `GET /api/v1/dashboard/metrics` also sends deprecated snake_case
  keys (#35). That one stays; only the survey-responses exception is removed.

## Assumptions

- **Breaking change accepted.** The ticket names the out-of-repo reader risk and gives it an
  owner; the only in-repo reader is the console, updated in this PR. The SDK (`@cxp/sdk`) reads
  survey responses over MCP (`get_survey_responses`), not REST, so it is untouched. The console
  and the server ship in one image, so a fresh console never meets an old server or the reverse.
  The exception is a console bundle already precached by its service worker (`registerType:
  "autoUpdate"`), which runs until the worker updates; see Risks.
- **No compatibility alias.** Content negotiation or a query flag to keep the array was rejected:
  #24 changed other list shapes without one, and the console is the known reader.
- **The console shows the total.** The detail page heading becomes `Responses (<total>)`, or
  `Responses (<shown> of <total>)` when `total` exceeds the rows shown, computed by an exported
  pure function `responsesCount(shown, total)` in `survey-detail.tsx` so it is unit-tested
  (the console has no component-render tests). It still fetches
  the first page only (limit 50, as today); paging through responses is out of scope.
- **`getResponses` takes optional `{limit, offset}`** with the same defaults (50, 0) as
  `surveysApi.list`, so a later pager needs no API change; the hook keys on them.
- **The shipped wire-casing spec** (`docs/specs/2026-10-15-api-wire-casing-design.md`) stays as
  written, since it records #24; the living docs are the ones the ticket names.
- No Run Profile value was assumed; the section supplies every key this run uses.

## Goal & Success Criteria

Make the survey responses list answer like every other `/api/v1` list, and give the console the
total it now drops.

- `GET /api/v1/surveys/{id}/responses` answers `{data, total, limit, offset}`, with `total` the
  count of the survey's responses in the caller's org and `limit`/`offset` the clamped page
  served.
- `404 not_found` for a survey outside the caller's org is unchanged; `cxp:read` still suffices.
- The wire fixture pins a `surveyResponseList` sample, and the console's type tests hold it to
  `PaginatedResponse<SurveyResponse>`.
- The console's survey detail page lists `data` and shows `total`.
- `routes/mod.rs` and the resource-server design doc name only the dashboard exception.

## Scope

In: the `responses` handler in `crates/cxp-api/src/routes/surveys.rs`; the wire fixture
(`routes/mod.rs` `wire_fixture`, `tests/fixtures/wire.json`); `crates/cxp-api/tests/api.rs`;
`apps/web/src/lib/api/surveys.ts`, `apps/web/src/hooks/use-surveys.ts`,
`apps/web/src/pages/surveys/survey-detail.tsx`, `apps/web/src/lib/types.test.ts`; the convention
docs.

Out: MCP `get_survey_responses` (already `{items, total}`), survey response submission, console
paging UI, the dashboard metrics exception (#35), `cxp-core` (unchanged: it already returns the
total).

## Architecture

- `crates/cxp-api/src/routes/surveys.rs:140-155` `responses` (its whole doc comment is rewritten,
  dropping the #36 note): return
  `ApiResult<Json<Paginated<SurveyResponse>>>`; build it as
  `let page = q.page(); Paginated::new(tx.list_survey_responses(id, page).await?, page)`, the
  pattern every other list handler uses. Doc comment drops "plain JSON array" and states the
  envelope. Public interface change: **breaking** (array → object) for this one route.
- `cxp_core::FeedbackExt::list_survey_responses` (`crates/cxp-core/src/feedback.rs:555`) already
  returns `(rows, total)` with the org-pinned count; unchanged.
- `routes/mod.rs` module doc: "Two exceptions remain" becomes one (the dashboard, #35).
  `wire_fixture::payloads()` gains
  `"surveyResponseList": Paginated::new((vec![survey_response()], 1), page)`; `wire.json`
  regenerated.
- Console: `surveysApi.getResponses(id, params?)` returns
  `apiFetch<PaginatedResponse<SurveyResponse>>` from
  `/surveys/${id}/responses?limit=..&offset=..`; `surveyKeys.responses(id, params)` includes the
  params; `useSurveyResponses(id, params?)`; the detail page reads `responsesData?.data ?? []`
  and `responsesData?.total ?? 0`.
- Design doc `docs/specs/2026-10-14-otto-resource-server-design.md:371-373`: the sentence names
  only the dashboard exception.

## Invariants impact

- Tenant isolation / `begin_live`: unchanged; the handler still opens via `state.begin` and the
  core query binds `self.org()`.
- Auth: unchanged (`require_read`).
- Metering: unchanged; the route stays free (asserted in
  `only_creating_a_survey_charges_and_it_charges_once`).
- Small shared DB: the query was already bounded (`Page` clamps limit to 200) and already counted
  the total; no new query.
- No SQL outside `cxp-core`: no new SQL anywhere. The api test seeds responses through
  `cxp_core::feedback::FeedbackExt::submit_survey_response` on an org-pinned `Db::begin(org)`
  transaction (as `crates/cxp-core/tests` do), so seeding runs under RLS.
- Migrations: none.

## Out-of-band

None. No config, secret, scope, migration, or role change, and no recurring-cost impact.

## Error Handling & Edge Cases

- Unknown or other-org survey: `list_survey_responses` returns `NotFound` before counting → `404
  not_found`, unchanged.
- Survey with no responses: `{data: [], total: 0, limit: 50, offset: 0}`.
- Offset past the end: `data: []`, `total` still the full count.
- Out-of-range `limit`/`offset`: clamped by `Page` and echoed as served (limit 1..=200, offset ≥
  0).
- `answers` is caller JSON and passes through untouched (no casing change).

## Testing Approach

- Rust unit (`cxp-api` lib, `wire_fixture`): the fixture test fails until `wire.json` is
  regenerated with the new `surveyResponseList` key.
- Rust integration (`crates/cxp-api/tests/api.rs`): a new `#[sqlx::test]` creates a survey and two
  customers over the API, seeds three responses through `FeedbackExt::submit_survey_response` on
  an org-pinned transaction, and asserts:
  exactly the four envelope keys; `total` 3, default `limit` 50, `offset` 0; `?limit=2&offset=0`
  serves two rows with `total` 3; `?offset=10` serves `[]` with `total` 3; a response row carries
  `surveyId`/`customerId`/`completedAt` (camelCase); an empty survey answers `total` 0; another
  org's caller gets `404 not_found`.
- Web (`apps/web/src/lib/types.test.ts`): `surveyResponseList` satisfies
  `PaginatedResponse<SurveyResponse>` and joins the envelope test; an api-call test stubs fetch
  with it and checks `surveysApi.getResponses` returns `data`/`total` and requests
  `limit=50&offset=0`. `apps/web/src/pages/surveys/survey-detail.test.ts` covers
  `responsesCount` (`3`/`3` → `"3"`, `50`/`120` → `"50 of 120"`, `0`/`0` → `"0"`).
- Gates: test, lint, format, extra (migrations), the pnpm gate (`apps/web/**` changed), and the
  container build (`crates/**`, `apps/web/**` changed).

## Risks & Open Questions

- Out-of-repo readers of the array break (ticket risk, owner Rob). No in-repo reader besides the
  console. The PR body calls the break out.
- A console bundle loaded or precached before the deploy calls `.map` on the new object when a
  survey has responses, which throws, so that page fails until the service worker's auto-update
  serves the new bundle (the next load). A survey with no responses also misrenders (the old
  `length === 0` check is false on an object). Accepted: the console is first-party, the window is
  one load, and #24 changed other list shapes the same way.
