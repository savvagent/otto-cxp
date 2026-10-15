# Survey Responses Envelope Implementation Plan

> Execute task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `GET /api/v1/surveys/{id}/responses` answers `Paginated<SurveyResponse>`
(`{data, total, limit, offset}`), and the console reads it and shows the total.

**Architecture:** The handler wraps the `(rows, total)` that `cxp_core` already returns in
`routes::Paginated`, like every other list. The wire fixture pins the new shape; the console's
API client, hook, and survey detail page read `data`/`total`.

**Tech Stack:** Rust (axum 0.8, sqlx), TypeScript (React, TanStack Query, vitest).

**Ticket:** savvagent/otto-cxp#36

**Spec:** `docs/specs/2026-10-15-survey-responses-envelope-design.md`

## Global Constraints

- Work only in the worktree
  `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-36-survey-responses-envelope`; trunk
  changes only through a PR.
- Gates (DB on a private container, port 15465; never the shared :15435):
  - test: `DATABASE_URL=postgres://cxp:cxp@localhost:15465/otto_cxp cargo test --workspace --locked`
  - lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - format: `cargo fmt --all -- --check`
  - extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'` prints nothing
  - web: `pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test` (repo root)
  - container: `podman build -t otto-cxp:check-issue36 .`
- Evidence: pass counts per test binary. A panic without `DATABASE_URL` is a missing variable,
  not a failure; skipped tests are not a pass.
- Invariants: no SQL outside `cxp-core` (none added; tests seed through `FeedbackExt`); request
  path still opens via `state.begin` (`begin_live`); `cxp:read` still suffices; the route stays
  unmetered; the query stays bounded by `Page`. No migration.
- Out-of-band: none.
- Conventional commits, body explains why, ends `Fixes #36`. No attribution of any kind.

## Tasks

### Task 1: The route answers the list envelope

**Model:** sonnet

**Files:**
- Modify: `crates/cxp-api/src/routes/surveys.rs` (`responses`)
- Modify: `crates/cxp-api/src/routes/mod.rs` (module doc; `wire_fixture::payloads`)
- Modify: `crates/cxp-api/tests/fixtures/wire.json` (regenerated)
- Modify: `crates/cxp-api/tests/api.rs` (new test)
- Modify: `docs/specs/2026-10-14-otto-resource-server-design.md` (convention sentence)

- [ ] **Step 1: Write the failing tests.**
  - In `wire_fixture::payloads()` add, after `"surveyList"`,
    `"surveyResponseList": Paginated::new((vec![survey_response()], 1), page),`.
  - In `crates/cxp-api/tests/api.rs` add
    `#[sqlx::test(migrator = "cxp_core::MIGRATOR")] async fn survey_responses_answer_the_list_envelope(pool: PgPool)`:
    caller `(org, _, t) = h.caller(RW)`; create a survey (`POST /api/v1/surveys`) and an empty
    second survey, and two customers (`POST /api/v1/customers`); seed three responses with
    `let db = Db::from_pool(h.pool.clone()); let mut tx = db.begin(OrgId::from(org)).await.unwrap();`
    (check how `otto_tenant::ids::OrgId` is built from a `Uuid` in `crates/cxp-core/tests/common`)
    and `cxp_core::feedback::FeedbackExt::submit_survey_response(&mut tx, survey, customer, &json!({"q1": 9}))`,
    then commit. Assert: `GET /api/v1/surveys/{survey}/responses` is 200 with exactly the keys
    `data,limit,offset,total`, `total` 3, `limit` 50, `offset` 0, 3 rows, and `data[0]` has
    `surveyId` = survey, `customerId`, `completedAt` non-null, `answers` = `{"q1": 9}`, and no
    `survey_id` key; `?limit=2&offset=0` → 2 rows, `total` 3, `limit` 2; `?offset=10` → `[]`,
    `total` 3, `offset` 10; the empty survey → `{data: [], total: 0, limit: 50, offset: 0}`;
    another org's caller (`h.caller(RO)`) on the first survey → 404 with `code` `not_found`.
    Update the module doc's list at the top of `api.rs` only if it enumerates route shapes.
- [ ] **Step 2: Run and watch them fail.**
  `DATABASE_URL=postgres://cxp:cxp@localhost:15465/otto_cxp cargo test -p cxp-api --locked`
  (the fixture test fails on the missing key; the new api test fails because the body is an
  array).
- [ ] **Step 3: Implement.** In `routes/surveys.rs`, `responses` returns
  `ApiResult<Json<Paginated<SurveyResponse>>>`:
  `let page = q.page(); ... let found = tx.list_survey_responses(id, page).await?; tx.commit().await?; Ok(Json(Paginated::new(found, page)))`.
  Rewrite its whole doc comment: one page of the survey's responses, newest first, in the
  [`Paginated`] envelope; `404` when the survey is not in the caller's org. Import `Paginated`
  if not already. In `routes/mod.rs`'s module doc replace "Two exceptions remain, each tracked:
  `GET /surveys/{id}/responses` is a plain array (#36), and `GET /dashboard/metrics` also sends
  ..." with "One exception remains, tracked: `GET /dashboard/metrics` also sends ..." (keep the
  rest). In the design doc (around line 371) replace "The tracked exceptions are survey
  responses (a plain array, #36) and `GET /api/v1/dashboard/metrics`, which also sends its
  deprecated snake_case keys (#35)." with "The tracked exception is
  `GET /api/v1/dashboard/metrics`, which also sends its deprecated snake_case keys (#35)."
  Regenerate the fixture:
  `UPDATE_WIRE_FIXTURES=1 cargo test -p cxp-api --lib wire_fixture` (it panics by design after
  writing), then re-run without the variable.
- [ ] **Step 4: Run the gates and watch them pass:** test, lint, format, extra (Global
  Constraints). The web gate will still pass at this point because nothing in `apps/web` reads
  `surveyResponseList` yet.
- [ ] **Step 5: Commit**: `feat(api): answer survey responses in the list envelope`, body: the
  route was the last list answering a bare array and dropped the total cxp-core computes; ends
  `Refs #36`.

### Task 2: The console reads the envelope and shows the total

**Model:** sonnet

**Files:**
- Modify: `apps/web/src/lib/api/surveys.ts` (`getResponses`)
- Modify: `apps/web/src/hooks/use-surveys.ts` (`surveyKeys.responses`, `useSurveyResponses`)
- Modify: `apps/web/src/pages/surveys/survey-detail.tsx` (reads `data`/`total`; `responsesCount`)
- Modify: `apps/web/src/lib/types.test.ts`
- Create: `apps/web/src/pages/surveys/survey-detail.test.ts`

- [ ] **Step 1: Write the failing tests.**
  - `types.test.ts`: add `surveyResponseList: wire.surveyResponseList satisfies PaginatedResponse<SurveyResponse>,`
    to `typed`; add `[typed.surveyResponseList, keys.surveyResponse]` to the envelope test's
    `lists`; import `surveysApi` from `./api/surveys` and add to "api calls decode the server's
    payloads": `respondWith(wire.surveyResponseList)`, `const page = await surveysApi.getResponses(wire.survey.id)`,
    expect `page.total` 1, `page.data[0].surveyId` equals `wire.surveyResponse.surveyId`, and
    the stubbed fetch was called with a URL containing
    `/surveys/${wire.survey.id}/responses?limit=50&offset=0` (read the mock's first call
    argument; check `apiFetch` in `apps/web/src/lib/api-client.ts` for the URL it builds).
  - `survey-detail.test.ts`: `import { responsesCount } from "./survey-detail";` and expect
    `responsesCount(3, 3)` → `"3"`, `responsesCount(50, 120)` → `"50 of 120"`,
    `responsesCount(0, 0)` → `"0"`.
- [ ] **Step 2: Run and watch them fail.** `cd apps/web && pnpm build` (type errors:
  `getResponses` returns an array) and `pnpm test` (after a build, for `dist/sw.js`).
- [ ] **Step 3: Implement.**
  - `surveys.ts`: `getResponses: (id: string, params?: { limit?: number; offset?: number }) => { const searchParams = new URLSearchParams(); searchParams.set("limit", String(params?.limit ?? 50)); searchParams.set("offset", String(params?.offset ?? 0)); return apiFetch<PaginatedResponse<SurveyResponse>>(`/surveys/${id}/responses?${searchParams.toString()}`); }`.
  - `use-surveys.ts`: `responses: (id: string, params: Record<string, unknown> = {}) => [...surveyKeys.detail(id), "responses", params] as const`;
    `useSurveyResponses(id, params?: { limit?: number; offset?: number })` keys on
    `surveyKeys.responses(id!, params ?? {})` and calls `surveysApi.getResponses(id!, params)`.
  - `survey-detail.tsx`: export
    `export function responsesCount(shown: number, total: number): string { return shown < total ? `${shown} of ${total}` : String(total); }`
    (doc comment: the console fetches the first page only); `const responses = responsesData?.data ?? []; const total = responsesData?.total ?? 0;`;
    heading `Responses` becomes `Responses ({responsesCount(responses.length, total)})` once
    loaded (plain `Responses` while loading); replace the inline
    `import("@/lib/types").SurveyResponse` annotation in `.map` with nothing (inferred) or a
    top-level type import.
- [ ] **Step 4: Run the gates and watch them pass:** web
  (`pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test` from the repo root),
  plus test, lint, format, extra, and the container build.
- [ ] **Step 5: Commit**: `feat(web): read survey responses from the list envelope`, body: the
  route now answers `{data, total, limit, offset}`; the detail page shows the total; ends
  `Refs #36`.
