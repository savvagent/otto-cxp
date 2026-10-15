# One casing convention for `/api/v1` Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** `/api/v1` speaks camelCase keys everywhere (old snake_case request spellings still
accepted), search answers the list envelope with `totalCapped`, the dashboard answers both key
sets, and the console uses only the new shapes.

**Architecture:** serde `rename_all = "camelCase"` plus a per-field snake_case `alias` on every
request struct in `crates/cxp-api/src/routes`; a `CappedPaginated<T>` envelope for search; a
dashboard `Metrics` that flattens the camelCase core model plus deprecated snake_case keys; the
console's `lib/types.ts`, `lib/api/*.ts` and their callers switch to camelCase.

**Tech Stack:** Rust (axum 0.8, serde), TypeScript (React, vite, vitest).

**Ticket:** savvagent/otto-cxp#24

**Spec:** `docs/specs/2026-10-15-api-wire-casing-design.md`

## Global Constraints

- Gates (run from the worktree `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-24-camelcase-wire`):
  - test: `DATABASE_URL=postgres://cxp:cxp@localhost:15461/otto_cxp cargo test --workspace --locked`
    (this worktree's own Postgres on :15461, never :15435 and never a Fly database)
  - lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - format: `cargo fmt --all -- --check`
  - extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'` prints nothing
  - web (Task 3 on): `pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test` from the worktree root
  - docker (final run only): `podman build -t otto-cxp:check .`
- Evidence: report pass counts per test binary; `cargo test` without `DATABASE_URL` panics; a
  skipped test is not a pass.
- Invariants that apply: no SQL outside `cxp-core` (none is added); every route keeps its
  `require_read`/`require_write`; database error text never reaches a response; no unbounded
  list; no migration.
- Worktree only; trunk changes only through the PR.
- Commit format: `<type>(<scope>): <imperative, lower-case>`, body explains why and ends
  `Refs #24`; no attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers).

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-api/src/routes/{customers,tickets,feedback,surveys,catalog,mod}.rs` | request structs camelCase + aliases; `CappedPaginated`; search response; module docs |
| **Modify.** `crates/cxp-api/src/routes/dashboard.rs` | `Metrics` with camelCase + legacy keys |
| **Modify.** `crates/cxp-api/tests/fixtures/wire.json` | regenerated |
| **Modify.** `crates/cxp-api/tests/api.rs` | integration coverage for the new shapes and both spellings |
| **Modify.** `apps/web/src/lib/types.ts`, `apps/web/src/lib/types.test.ts`, `apps/web/src/lib/api/*.ts`, callers in `apps/web/src/{hooks,pages,components}` | console speaks camelCase |
| **Modify.** `docs/specs/2026-10-14-otto-resource-server-design.md` | the convention in the REST section |

## Task Order & Rationale

Request side first (pure additive, unit-testable without a DB), then the two response changes
and the fixture (which the console tests load), then the console against the new fixture, then
docs. Each task leaves every gate green.

---

### Task 1: camelCase request structs with snake_case aliases

**Files:**
- Modify: `crates/cxp-api/src/routes/customers.rs` (`CreateBody`, `UpdateBody`, `ListParams`, `SearchParams`)
- Modify: `crates/cxp-api/src/routes/tickets.rs` (`CreateBody`, `UpdateBody`, `ListParams`)
- Modify: `crates/cxp-api/src/routes/feedback.rs` (`ListParams`)
- Modify: `crates/cxp-api/src/routes/surveys.rs` (`CreateBody`, `UpdateBody`, `ListParams`)
- Modify: `crates/cxp-api/src/routes/catalog.rs` (all `Create*Body`, `Update*Body`, `*Params`, and its `tests` module)
- Modify: `crates/cxp-api/src/routes/mod.rs` (`PageParams`; new `#[cfg(test)] mod casing`)

**Interfaces:** produces the camelCase request contract Task 3's console uses.

**Out-of-band / invariants:** none; no handler logic changes.

**Model:** sonnet — mechanical serde attributes plus tests.

- [ ] **Step 1: Write the failing tests.** Append to `crates/cxp-api/src/routes/mod.rs`:

```rust
/// Request structs take camelCase keys and, as aliases, the snake_case
/// spellings `/api/v1` accepted before #24, so older callers keep working.
#[cfg(test)]
mod casing {
    use axum::extract::Query;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use uuid::Uuid;

    fn body<T: DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    fn query<T: DeserializeOwned>(qs: &str) -> T {
        let uri: http::Uri = format!("/x?{qs}").parse().unwrap();
        Query::<T>::try_from_uri(&uri).unwrap().0
    }

    fn query_err<T: DeserializeOwned>(qs: &str) -> bool {
        let uri: http::Uri = format!("/x?{qs}").parse().unwrap();
        Query::<T>::try_from_uri(&uri).is_err()
    }

    const A: Uuid = Uuid::from_u128(1);
    const B: Uuid = Uuid::from_u128(2);
    const C: Uuid = Uuid::from_u128(3);
    const D: Uuid = Uuid::from_u128(4);

    /// `experienceId=A&applicationId=B&pageId=C&componentId=D` in one spelling.
    fn catalog_qs(camel: bool) -> String {
        if camel {
            format!("experienceId={A}&applicationId={B}&pageId={C}&componentId={D}")
        } else {
            format!("experience_id={A}&application_id={B}&page_id={C}&component_id={D}")
        }
    }

    #[test]
    fn customer_list_params_take_both_spellings() {
        for camel in [true, false] {
            let q: super::customers::ListParams =
                query(&format!("limit=5&offset=1&{}", catalog_qs(camel)));
            assert_eq!((q.limit, q.offset), (Some(5), Some(1)));
            assert_eq!(q.experience_id, Some(A));
            assert_eq!(q.application_id, Some(B));
            assert_eq!(q.page_id, Some(C));
            assert_eq!(q.component_id, Some(D));
        }
    }

    #[test]
    fn survey_list_params_take_both_spellings() {
        for camel in [true, false] {
            let q: super::surveys::ListParams = query(&catalog_qs(camel));
            assert_eq!(
                (q.experience_id, q.application_id, q.page_id, q.component_id),
                (Some(A), Some(B), Some(C), Some(D))
            );
        }
    }

    #[test]
    fn ticket_params_and_bodies_take_both_spellings() {
        for (customer, rest) in [("customerId", catalog_qs(true)), ("customer_id", catalog_qs(false))] {
            let q: super::tickets::ListParams =
                query(&format!("status=open&priority=high&{customer}={A}&{rest}"));
            assert_eq!(q.customer_id, Some(A));
            assert_eq!(q.experience_id, Some(A));
            assert_eq!(q.application_id, Some(B));
            assert_eq!(q.page_id, Some(C));
            assert_eq!(q.component_id, Some(D));
            assert_eq!(q.status.as_deref(), Some("open"));
        }
        for key in ["customerId", "customer_id"] {
            let b: super::tickets::CreateBody = body(json!({ key: A, "subject": "s" }));
            assert_eq!(b.customer_id, A);
        }
        for key in ["assignedTo", "assigned_to"] {
            let b: super::tickets::UpdateBody = body(json!({ key: "grace" }));
            assert_eq!(b.assigned_to.as_deref(), Some("grace"));
        }
    }

    #[test]
    fn feedback_params_take_both_spellings() {
        for (customer, kind, rest) in [
            ("customerId", "feedbackType", catalog_qs(true)),
            ("customer_id", "feedback_type", catalog_qs(false)),
        ] {
            let q: super::feedback::ListParams =
                query(&format!("{customer}={A}&{kind}=nps&{rest}"));
            assert_eq!(q.customer_id, Some(A));
            assert_eq!(q.feedback_type.as_deref(), Some("nps"));
            assert_eq!(
                (q.experience_id, q.application_id, q.page_id, q.component_id),
                (Some(A), Some(B), Some(C), Some(D))
            );
        }
    }

    #[test]
    fn catalog_params_and_bodies_take_both_spellings() {
        use super::catalog::*;
        for camel in [true, false] {
            let (exp, app, page, kind, team) = if camel {
                ("experienceId", "applicationId", "pageId", "componentType", "teamId")
            } else {
                ("experience_id", "application_id", "page_id", "component_type", "team_id")
            };
            let q: ApplicationParams = query(&format!("{exp}={A}"));
            assert_eq!(q.experience_id, Some(A));
            let q: PageParamsByApplication = query(&format!("{app}={B}"));
            assert_eq!(q.application_id, Some(B));
            let q: ComponentParams = query(&format!("{page}={C}"));
            assert_eq!(q.page_id, Some(C));

            let b: CreateExperienceBody = body(json!({ "name": "e", team: D }));
            assert_eq!(b.team_id, Some(D));
            let b: UpdateExperienceBody = body(json!({ team: D }));
            assert_eq!(b.team_id, Some(Some(D)));
            let b: CreateApplicationBody = body(json!({ "name": "a", exp: A, team: D }));
            assert_eq!((b.experience_id, b.team_id), (Some(A), Some(D)));
            let b: UpdateApplicationBody = body(json!({ team: null }));
            assert_eq!(b.team_id, Some(None));
            let b: CreatePageBody = body(json!({ "name": "p", app: B, team: D }));
            assert_eq!((b.application_id, b.team_id), (Some(B), Some(D)));
            let b: UpdatePageBody = body(json!({ team: D }));
            assert_eq!(b.team_id, Some(Some(D)));
            let b: CreateComponentBody =
                body(json!({ "name": "c", page: C, kind: "button", team: D }));
            assert_eq!((b.page_id, b.team_id), (Some(C), Some(D)));
            assert_eq!(b.component_type.as_deref(), Some("button"));
            let b: UpdateComponentBody = body(json!({ kind: "link", team: D }));
            assert_eq!(b.component_type.as_deref(), Some("link"));
            assert_eq!(b.team_id, Some(Some(D)));
        }
    }

    #[test]
    fn both_spellings_of_one_field_are_refused() {
        let both = serde_json::from_value::<super::tickets::CreateBody>(
            json!({ "customerId": A, "customer_id": B, "subject": "s" }),
        );
        assert!(both.is_err());
        assert!(query_err::<super::customers::ListParams>(&format!(
            "pageId={A}&page_id={B}"
        )));
    }
}
```

- [ ] **Step 2: Run to verify it fails.**
  Run: `cargo test -p cxp-api --locked --lib casing`
  Expected: FAIL (camelCase keys are ignored, so `assert_eq!(…, Some(A))` fails on the camel
  iteration; `both_spellings_of_one_field_are_refused` fails because `customerId` is unknown).

- [ ] **Step 3: Implement.** On every request struct listed under **Files**, add
  `#[serde(rename_all = "camelCase")]` after `#[derive(Debug, Deserialize)]` (including structs
  whose fields are all single words: `customers::{CreateBody, UpdateBody, SearchParams}`,
  `surveys::{CreateBody, UpdateBody}`, `mod::PageParams`). On every field whose Rust name contains
  `_`, add `#[serde(alias = "<rust field name>")]`, merged into any existing `#[serde(...)]`.
  Examples:

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    #[serde(alias = "experience_id")]
    pub experience_id: Option<Uuid>,
    #[serde(alias = "application_id")]
    pub application_id: Option<Uuid>,
    #[serde(alias = "page_id")]
    pub page_id: Option<Uuid>,
    #[serde(alias = "component_id")]
    pub component_id: Option<Uuid>,
}
```

  Catalog team fields swap primary name and alias:

```rust
    #[serde(default, alias = "team_id")]
    pub team_id: Option<Uuid>,
    // update bodies:
    #[serde(default, alias = "team_id", deserialize_with = "present")]
    pub team_id: Option<Option<Uuid>>,
```

  Update the catalog module doc's **The team field** paragraph: bodies take `teamId` (the
  convention) and `team_id` as an alias (sending both is a `400`). Rename the existing catalog
  unit test to `the_team_field_tells_absent_from_null_and_accepts_both_spellings` (keep the name;
  its assertions already cover both spellings and still pass unchanged).

- [ ] **Step 4: Run to verify it passes.**
  Run: `cargo test -p cxp-api --locked --lib` — Expected: all pass, including the six `casing::*`
  tests and `catalog::tests::*`.
  Then: `DATABASE_URL=postgres://cxp:cxp@localhost:15461/otto_cxp cargo test -p cxp-api --locked`
  (the integration suite still sends snake_case and must stay green: alias coverage),
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`.

- [ ] **Step 5: Commit.**

```bash
git add crates/cxp-api/src/routes
git commit -m "feat(api): take camelCase request keys with snake_case aliases" \
  -m "One wire convention for /api/v1: request bodies and query parameters now use the camelCase keys responses already use. The snake_case spellings stay as serde aliases so existing callers keep working.

Refs #24"
```

---

### Task 2: search envelope, dashboard keys, fixture, integration tests

**Files:**
- Modify: `crates/cxp-api/src/routes/mod.rs` (`Paginated` gets `rename_all`; new `CappedPaginated`; `wire_fixture` payloads)
- Modify: `crates/cxp-api/src/routes/customers.rs:73-110` (search)
- Modify: `crates/cxp-api/src/routes/dashboard.rs`
- Modify: `crates/cxp-api/tests/fixtures/wire.json` (regenerated)
- Modify: `crates/cxp-api/tests/api.rs` (around lines 546-575 and the catalog/ticket sections)
- Modify: `crates/cxp-server/tests/e2e.rs:1333-1339` (the `RouteExpect::CountsOnlyB` dashboard assertion)

**Interfaces:** consumes Task 1's request structs; produces `wire.json` keys `customerSearch` and the new `dashboardMetrics` that Task 3's `types.test.ts` reads.

**Out-of-band / invariants:** search keeps its bounded `Page` and capped count; scope checks unchanged; no SQL.

**Model:** sonnet — contained serde/handler change with a spec-given shape.

- [ ] **Step 1: Write the failing integration assertions.** In `crates/cxp-api/tests/api.rs`, replace the search block (currently asserting `found.body.as_array()` and the `x-total-*` headers) with:

```rust
    let found = h
        .call("GET", "/api/v1/customers/search?q=ada", Some(&t), None)
        .await;
    assert_eq!(found.status, StatusCode::OK, "{}", found.body);
    // Search answers the list envelope plus `totalCapped` (#24); the #17
    // count headers are gone.
    assert_eq!(found.body["data"].as_array().unwrap().len(), 1);
    assert_eq!(found.body["total"], 1);
    assert_eq!(found.body["totalCapped"], false);
    assert_eq!(found.body["limit"], 50);
    assert_eq!(found.body["offset"], 0);
    assert!(found.headers.get("x-total-count").is_none());
    assert!(found.headers.get("x-total-capped").is_none());
```

  Replace the ticket creation right after it and add a camelCase twin and a filter check:

```rust
    // The old snake_case body key still works (an alias) ...
    let ticket = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customer_id": id, "subject": "Broken"})),
        )
        .await;
    assert_eq!(ticket.status, StatusCode::OK, "{}", ticket.body);
    assert_eq!(ticket.body["priority"], "medium");
    assert_eq!(ticket.body["customerId"], id.as_str());
    // ... and so does the camelCase one, the convention.
    let ticket2 = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customerId": id, "subject": "Also broken", "priority": "low"})),
        )
        .await;
    assert_eq!(ticket2.status, StatusCode::OK, "{}", ticket2.body);
    assert_eq!(ticket2.body["customerId"], id.as_str());
    for key in ["customerId", "customer_id"] {
        let listed = h
            .call("GET", &format!("/api/v1/tickets?{key}={id}"), Some(&t), None)
            .await;
        assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
        assert_eq!(listed.body["total"], 2, "{key}");
    }
    let unrelated = h
        .call(
            "GET",
            &format!("/api/v1/tickets?customerId={}", uuid::Uuid::new_v4()),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(unrelated.body["total"], 0);
    let both = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customerId": id, "customer_id": id, "subject": "x"})),
        )
        .await;
    assert_eq!(both.status, StatusCode::BAD_REQUEST);
    assert_eq!(both.body["code"], "invalid_argument");
```

  and the metrics assertion (two tickets now, both open):

```rust
    let metrics = h
        .call("GET", "/api/v1/dashboard/metrics", Some(&t), None)
        .await;
    // camelCase, plus the snake_case keys this route answered with before
    // #24, kept for existing consumers.
    assert_eq!(
        metrics.body,
        json!({
            "totalCustomers": 1, "totalTickets": 2, "openTickets": 2,
            "totalFeedback": 0, "totalSurveys": 0, "totalExperiences": 0,
            "total_customers": 1, "total_tickets": 2, "open_tickets": 2,
            "total_feedback": 0, "total_surveys": 0, "total_experiences": 0,
        })
    );
```

  If `uuid` is not already imported in `api.rs`, use the path the file already uses for UUIDs
  (check the imports at the top; `sqlx`/`uuid` are reachable via the existing dev-dependencies —
  add `uuid.workspace = true` under `[dev-dependencies]` in `crates/cxp-api/Cargo.toml` only if
  it is not already a normal dependency of `cxp-api`, which it is). Read the surrounding test
  before editing: if any later assertion in the same test counts tickets (for example a
  `total_tickets` or list `total`), update it for the second ticket.

  In `crates/cxp-server/tests/e2e.rs`, the `RouteExpect::CountsOnlyB` arm asserts the dashboard
  body exactly; extend its expected `json!` with the camelCase twins of the same counts:
  `"totalCustomers": n("customers"), "totalTickets": n("tickets"), "openTickets": open,
  "totalFeedback": n("feedback"), "totalSurveys": n("surveys"), "totalExperiences":
  n("experiences")` alongside the existing snake_case keys. Then
  `grep -n "x-total\|as_array" crates/cxp-server/tests/e2e.rs` and confirm no assertion reads the
  search body as an array (its search case only checks for leaked text).

  Also find the catalog test in `api.rs` that creates an entity with a team (search for
  `teamId` / `team_id`); if it only uses one spelling, leave it, since Task 1's unit tests cover
  both spellings of every catalog body.

- [ ] **Step 2: Run to verify it fails.**
  Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15461/otto_cxp cargo test -p cxp-api --locked --test api`
  Expected: FAIL at the search envelope assertion (`found.body["data"]` is `null` on an array).

- [ ] **Step 3: Implement.**

  In `routes/mod.rs`, add `#[serde(rename_all = "camelCase")]` to `Paginated`, and below it:

```rust
/// A search response: [`Paginated`] plus `totalCapped`. `total` counts
/// matches up to [`cxp_core::search::TOTAL_CAP`]; `totalCapped: false` means
/// `total` and `data` are exact, `true` that `total` is a lower bound (more
/// matches than the cap, or the search stopped before counting them all),
/// `data` is then the requested page of the best-ranked matches found, and a
/// page starting at or past the cap is empty.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CappedPaginated<T: Serialize> {
    pub data: Vec<T>,
    pub total: i64,
    pub total_capped: bool,
    pub limit: i64,
    pub offset: i64,
}

impl<T: Serialize> CappedPaginated<T> {
    pub fn new(found: cxp_core::search::SearchPage<T>, page: cxp_core::Page) -> Self {
        Self {
            data: found.items,
            total: found.total,
            total_capped: found.total_capped,
            limit: page.limit(),
            offset: page.offset(),
        }
    }
}
```

  (If `cxp_core::search::SearchPage` is not public at that path, use the path
  `customers::search_customers` returns — check `crates/cxp-core/src/search.rs` and
  `crates/cxp-core/src/lib.rs`.)

  In `routes/customers.rs`, delete `TOTAL_COUNT`, `TOTAL_CAPPED`, the `http::{HeaderName,
  HeaderValue}` import, and rewrite `search`:

```rust
/// The best matches first, in the list envelope plus `totalCapped`
/// ([`CappedPaginated`]); paged by `limit`/`offset` (default 50).
pub async fn search(
    State(state): State<AppState>,
    Extension(caller): Extension<Principal>,
    ApiQuery(q): ApiQuery<SearchParams>,
) -> ApiResult<Json<CappedPaginated<Customer>>> {
    caller.require_read()?;
    let page = cxp_core::Page::new(q.limit, q.offset);
    let mut tx = state.begin(&caller).await?;
    let found = tx.search_customers(&q.q, page).await?;
    tx.commit().await?;
    Ok(Json(CappedPaginated::new(found, page)))
}
```

  Search the workspace for other uses of `TOTAL_COUNT`/`TOTAL_CAPPED`/`x-total-count`
  (`grep -rn "TOTAL_COUNT\|TOTAL_CAPPED\|x-total" crates apps docs`) and remove or update them.

  Replace `routes/dashboard.rs`'s `Metrics` with (`Serialize` only: `flatten` is used on the serialize side; the six `LegacyMetrics`
  fields are exactly `DashboardMetrics`' six fields):

```rust
/// The headline counts: the camelCase keys of the convention, plus the
/// snake_case keys this route answered with before #24.
#[derive(Debug, Serialize)]
pub struct Metrics {
    #[serde(flatten)]
    pub current: DashboardMetrics,
    #[serde(flatten)]
    pub legacy: LegacyMetrics,
}

/// Deprecated: the pre-#24 snake_case keys, the same values as `current`.
/// Kept so consumers that read `total_customers` keep working; removal is
/// tracked in #<follow-up issue number, filled in by Task 4>.
#[derive(Debug, Serialize)]
pub struct LegacyMetrics {
    pub total_customers: i64,
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub total_feedback: i64,
    pub total_surveys: i64,
    pub total_experiences: i64,
}

impl From<DashboardMetrics> for Metrics {
    fn from(m: DashboardMetrics) -> Self {
        Self {
            legacy: LegacyMetrics {
                total_customers: m.total_customers,
                total_tickets: m.total_tickets,
                open_tickets: m.open_tickets,
                total_feedback: m.total_feedback,
                total_surveys: m.total_surveys,
                total_experiences: m.total_experiences,
            },
            current: m,
        }
    }
}
```

  with `use cxp_core::dashboard::{DashboardExt, DashboardMetrics};` and the handler returning
  `Ok(Json(Metrics::from(m)))`. Write the doc line as `tracked in a follow-up issue` if no number
  is given yet; Task 4 fills in the number.

  In `wire_fixture::payloads()`: build `"dashboardMetrics"` as
  `super::dashboard::Metrics::from(cxp_core::dashboard::DashboardMetrics { total_customers: 1,
  total_tickets: 2, open_tickets: 1, total_feedback: 3, total_surveys: 1, total_experiences: 1 })`
  and add
  `"customerSearch": super::CappedPaginated::new(cxp_core::search::SearchPage { items: vec![customer()], total: 1, total_capped: false }, page),`.

- [ ] **Step 4: Regenerate the fixture and run.**
  Run: `UPDATE_WIRE_FIXTURES=1 cargo test -p cxp-api --locked --lib wire_fixture` (expected to
  panic "fixture regenerated"), then `cargo test -p cxp-api --locked --lib` (all pass), then
  `DATABASE_URL=postgres://cxp:cxp@localhost:15461/otto_cxp cargo test -p cxp-api -p cxp-server --locked`
  (all pass, e2e included), clippy and fmt as in Global Constraints. Inspect `git diff
  crates/cxp-api/tests/fixtures/wire.json`: only `dashboardMetrics` gains six camelCase keys and
  `customerSearch` is added.

- [ ] **Step 5: Commit.**

```bash
git add crates/cxp-api crates/cxp-server/tests/e2e.rs
git commit -m "feat(api): answer search in the list envelope and metrics in camelCase" \
  -m "Search returned a bare array with its count in X-Total-* headers, a third shape beside the list envelope; it now answers {data, total, totalCapped, limit, offset}. The dashboard adds the camelCase keys and keeps its snake_case ones for existing consumers.

Refs #24"
```

---

### Task 3: the console speaks camelCase

**Files:**
- Modify: `apps/web/src/lib/types.ts`
- Modify: `apps/web/src/lib/types.test.ts`
- Modify: `apps/web/src/lib/api/{customers,tickets,feedback,surveys,experiences,applications,pages,components}.ts`
- Modify: callers that `tsc` flags, expected at least `apps/web/src/hooks/use-{tickets,customers,applications,pages,components,experiences,surveys,feedback}.ts`, `apps/web/src/pages/catalog/{experiences,applications,pages-page,components-page}.tsx`, `apps/web/src/pages/{customers,surveys,tickets,feedback}/*.tsx`, `apps/web/src/pages/dashboard/index.tsx`, `apps/web/src/components/command-palette.tsx`

**Interfaces:** consumes `wire.json` (`dashboardMetrics`, `customerSearch`) from Task 2 and the camelCase request contract from Task 1.

**Out-of-band / invariants:** none. Do not touch `sdk/typescript` or `packages/react` (they call `/mcp`, not `/api/v1`).

**Model:** sonnet — many mechanical edits guided by `tsc`.

- [ ] **Step 1: Write the failing test.** In `apps/web/src/lib/types.test.ts`:
  - import `CappedPaginatedResponse` from `./types`;
  - change the `dashboardMetrics` key list to
    `{ totalCustomers: true, totalTickets: true, openTickets: true, totalFeedback: true, totalSurveys: true, totalExperiences: true } satisfies Record<keyof DashboardMetrics, true>`;
  - add `cappedPaginated: { data: true, total: true, totalCapped: true, limit: true, offset: true } satisfies Record<keyof CappedPaginatedResponse<unknown>, true>`;
  - add to `typed`: `dashboardMetrics: wire.dashboardMetrics satisfies DashboardMetrics` (if not
    already present) and `customerSearch: wire.customerSearch satisfies CappedPaginatedResponse<Customer>`;
  - wherever the file asserts `sameKeys(wire.dashboardMetrics, keys.dashboardMetrics)`, replace it
    with a test that the fixture holds exactly the camelCase keys plus their legacy snake_case
    twins with equal values:

```ts
  it("answers dashboard metrics in camelCase, with the legacy snake_case keys alongside", () => {
    const snake = (k: string) => k.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
    const camel = Object.keys(keys.dashboardMetrics);
    sameKeys(wire.dashboardMetrics, Object.fromEntries([...camel, ...camel.map(snake)].map((k) => [k, true])));
    const m = wire.dashboardMetrics as Record<string, number>;
    for (const k of camel) expect(m[snake(k)]).toBe(m[k]);
  });

  it("answers search in the list envelope with totalCapped", () => {
    sameKeys(wire.customerSearch, keys.cappedPaginated);
    sameKeys(wire.customerSearch.data[0], keys.customer);
  });
```

  - find any test in that file (or `apps/web/test`, `apps/web/src/**/*.test.ts`) that asserts an
    API URL or request body with snake_case keys (for example a `customersApi`/`experiencesApi`
    fetch mock checking `experience_id` or `team_id`) and change the expectation to the camelCase
    key (`experienceId`, `teamId`, …). If `customersApi.search` is tested, it now returns the
    envelope.

- [ ] **Step 2: Run to verify it fails.**
  Run: `pnpm install --frozen-lockfile && pnpm --filter ./apps/web build` (from the worktree root; if the filter
  syntax is not accepted, `cd apps/web && pnpm build`). Expected: FAIL in `tsc`
  (`CappedPaginatedResponse` not exported; `DashboardMetrics` keys mismatch).

- [ ] **Step 3: Implement.**
  - `types.ts`: rewrite the header comment to: responses, request bodies and query parameters
    are all camelCase (the one `/api/v1` convention, `crates/cxp-api/src/routes/mod.rs`); enum
    values stay as the server stores them. `DashboardMetrics` becomes
    `{ totalCustomers; totalTickets; openTickets; totalFeedback; totalSurveys; totalExperiences }`
    (all `number`) with a doc comment that the server also sends deprecated snake_case twins the
    console does not read. Add:

```ts
/** A search response: the list envelope plus whether `total` is capped. */
export interface CappedPaginatedResponse<T> extends PaginatedResponse<T> {
  totalCapped: boolean;
}
```

  - `lib/api/*.ts`: rename every request-body field and every query-parameter key to camelCase,
    both in the TypeScript parameter types and in the `searchParams.set("…")` names:
    `experience_id→experienceId`, `application_id→applicationId`, `page_id→pageId`,
    `component_id→componentId`, `customer_id→customerId`, `feedback_type→feedbackType`,
    `assigned_to→assignedTo`, `team_id→teamId`, `component_type→componentType`. Example:

```ts
    if (params?.experienceId) searchParams.set("experienceId", params.experienceId);
```

    `customersApi.search` returns `apiFetch<CappedPaginatedResponse<Customer>>(…)`.
  - Run `cd apps/web && pnpm exec tsc -b` (or `pnpm build`) and fix every caller it flags: hooks'
    mutation parameter types, pages building request objects (`team_id: values.team_id || null` →
    `teamId: values.team_id || null`; keep zod form-field names and the browser URL parameter
    names such as `searchParams.get("experience_id")` as they are — they are not the API wire),
    the dashboard page (`metrics?.totalCustomers`, …), and the command palette
    (`setCustomers(customerResults.value.data.slice(0, 5))`).
  - `grep -rn "_id\b\|_type\b\|assigned_to\|total_" apps/web/src/lib/api apps/web/src/hooks` must
    show no API-wire snake_case key afterwards (form state and browser URL params excepted).

- [ ] **Step 4: Run to verify it passes.**
  Run from the worktree root: `pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test`.
  Expected: all packages build; vitest passes in every package with counts reported (no skips).

- [ ] **Step 5: Commit.**

```bash
git add apps/web
git commit -m "feat(web): send and read camelCase on the /api/v1 wire" \
  -m "The console sent snake_case bodies and query parameters while reading camelCase responses. It now uses the one convention throughout, reads the dashboard's camelCase metrics, and reads search from the list envelope.

Refs #24"
```

---

### Task 4: document the convention and file the follow-ups

**Files:**
- Modify: `crates/cxp-api/src/routes/mod.rs` (module doc)
- Modify: `crates/cxp-api/src/routes/surveys.rs` (`responses` doc)
- Modify: `crates/cxp-api/src/routes/dashboard.rs` (follow-up number)
- Modify: `docs/specs/2026-10-14-otto-resource-server-design.md` (REST section ~line 355-367; "Out of scope" ~line 418-419)

**Interfaces:** none.

**Out-of-band / invariants:** none.

**Model:** orchestrator, directly (docs plus two `gh issue create` calls).

- [ ] **Step 1: File the follow-ups** with `gh issue create --repo savvagent/otto-cxp --label enhancement --body-file <file>`:
  (a) remove `/dashboard/metrics`'s deprecated snake_case keys once no consumer reads them;
  (b) move `GET /api/v1/surveys/{id}/responses` into the `Paginated` envelope. Each body cites
  #24 and states the change and its acceptance criterion.
- [ ] **Step 2: Module doc.** Replace the `routes/mod.rs` header with the convention: every JSON
  key in request bodies and responses and every query-parameter name is camelCase; multi-word
  request keys also accept their pre-#24 snake_case spelling as an alias (both at once is `400`);
  enum values and caller-owned JSON (`metadata`, `questions`, `answers`) are untouched; lists are
  `Paginated` `{data, total, limit, offset}`, search is `CappedPaginated` (adds `totalCapped`),
  and `GET /surveys/{id}/responses` is the one plain array (follow-up #b); `GET /dashboard/metrics`
  also sends deprecated snake_case keys (follow-up #a); errors are `{error, code}` and the auth
  layer's `401`/`403` use the RFC 6750 names `error`/`error_description`. Update the `responses`
  doc in `surveys.rs` to cite follow-up #b, and `LegacyMetrics`' doc to cite #a.
- [ ] **Step 3: Design doc.** In the REST section replace "Request bodies are snake_case
  (`team_id`), and catalog bodies also accept `teamId` as a permanent alias; responses are
  camelCase. The casing mismatch is resolved by #15 (the console reads camelCase); the broader
  question of one convention for `/api/v1` is tracked in #24." with a statement of the convention
  (as in Step 2, condensed, citing #24), and in "Out of scope" replace "one casing convention for
  `/api/v1` is #24" with "`/api/v1` uses one camelCase convention (#24)". Also fix the sentence
  "Catalog create/update with a `teamId` …" only if it now contradicts.
- [ ] **Step 4: Run** `cargo test -p cxp-api --locked --lib`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `cargo doc -p cxp-api --no-deps` (no broken intra-doc links).
- [ ] **Step 5: Commit** `docs(api): state the one /api/v1 wire convention` with body `Refs #24`.

---

### Task 5: full gate run

- [ ] Run every gate in Global Constraints from the worktree, including `podman build -t otto-cxp:check .`, and record per-binary counts and `git rev-parse HEAD`.
