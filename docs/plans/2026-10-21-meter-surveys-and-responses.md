# Meter surveys sent and responses collected Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Bill otto-cxp on two declared value meters: `otto-cxp/surveys_sent` (`send_survey`) and
`otto-cxp/responses_collected` (`submit_feedback`, `submit_survey_response`). Make `create_survey`
free over MCP and REST, refuse billable calls when the platform blocks the org, and report the
meters and budget in `whoami`/`usage`.

**Architecture:**
- `cxp-core` owns the meter table, the outbox columns (migration 0005), the charge, the
  enforcement, the shipper and its declaration, and the new error variant.
- `cxp-mcp` and `cxp-api` map the error and stop charging `create_survey`.
- `cxp-server` retires `CXP_ENFORCE_QUOTAS`.

**Tech Stack:** Rust (axum, rmcp 2.0.0, sqlx 0.8 runtime queries), Postgres 16, otto-resource
v0.10.0.

**Ticket:** savvagent/otto-cxp#52 (PR 2 of 2; the PR carries `Fixes #52`)

**Spec:** `docs/specs/2026-10-21-meter-surveys-and-responses-design.md`

## Global Constraints

- **Worktree:** `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-52-meter-surveys-and-responses`
  only. Trunk changes only through the PR. Never touch the main checkout, which has unrelated
  uncommitted changes.
- **Gates:**
  - `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - `podman build -t otto-cxp:check .`, run once by the orchestrator before the PR
- **Targeted runs.** A single test file is fine while iterating, e.g.
  `DATABASE_URL=… cargo test -p cxp-core --test lifecycle --locked`. Every task ends with the full
  test gate.
- **Evidence:** report pass counts per test binary. A `cargo test` panic about `DATABASE_URL`
  means the variable is missing; it is not a pass. Skipped or ignored tests are not a pass.
  `DATABASE_URL` points only at the local Postgres on :15435, never at a Fly database. Only one
  worktree runs DB tests at a time.
- **Invariants:**
  - Migrations are append-only: 0005 is new, and 0001–0004 are never edited.
  - No SQL outside `cxp-core`, and runtime `sqlx::query`/`query_as` only (never the `query!`
    macros).
  - Request paths keep `platform_events::begin_live`, and the platform lookup (`warm`) stays
    before the transaction opens.
  - Database text never reaches a response.
  - `whoami`/`usage` need no scope; billable tools require `cxp:write` first.
  - `otto-tenant`/`otto-resource` stay on one rev.
  - The new metering rule: exactly `send_survey` → `otto-cxp/surveys_sent`, and `submit_feedback`
    and `submit_survey_response` → `otto-cxp/responses_collected`. Each records one row,
    quantity 1, in the tool's own transaction. No REST route records one, and nothing else does.
- **Commit format:** `<type>(<scope>): <imperative, lower-case>`. The body explains why and ends
  `Refs #52`; the final squash title uses `feat(core)!:` with a `BREAKING CHANGE:` footer.
- **No attribution of any kind:** no Co-Authored-By, no "Generated with", no credit markers, in
  commits, comments or docs.
- **Out-of-band:**
  - Migration 0005 applies at boot (verified after deploy).
  - Retired `CXP_ENFORCE_QUOTAS`: if it is set as a Fly secret, unsetting it is Rob's and optional.
  - No new secret, scope, role or recurring cost.

## File Structure

- Create: `crates/cxp-core/migrations/0005_usage_outbox_meter.sql`
- Modify, by crate:
  - cxp-core: `crates/cxp-core/src/usage.rs`, `crates/cxp-core/src/error.rs`,
    `crates/cxp-core/tests/lifecycle.rs`, `crates/cxp-core/tests/db.rs` (only if it asserts outbox
    columns or constraints)
  - cxp-mcp: `crates/cxp-mcp/src/error.rs`, `crates/cxp-mcp/src/server.rs`,
    `crates/cxp-mcp/src/tools/feedback.rs`, `crates/cxp-mcp/src/tools/org.rs`,
    `crates/cxp-mcp/src/tools/mod.rs`, `crates/cxp-mcp/src/tools/out.rs` (if `Status` is wrapped
    there), `crates/cxp-mcp/tests/mcp.rs`
  - cxp-api: `crates/cxp-api/src/error.rs`, `crates/cxp-api/src/lib.rs`,
    `crates/cxp-api/src/routes/surveys.rs`, `crates/cxp-api/tests/api.rs`
  - cxp-server: `crates/cxp-server/src/config.rs`, `crates/cxp-server/src/lib.rs`,
    `crates/cxp-server/src/boot.rs`, `crates/cxp-server/tests/e2e.rs`
  - docs and config: `CLAUDE.md`, `.env.example`, `docs/deploy/fly.md`

### Task 1: Outbox meter columns (migration 0005)

Model: sonnet

- [ ] **Step 1:** Write a failing test in `crates/cxp-core/tests/db.rs`, or extend the existing
  outbox schema test if one exists. It asserts:
  - `usage_outbox` and `usage_outbox_rejected` each have nullable `meter text` and
    `quantity bigint`;
  - inserting a row with `meter` set and `quantity` NULL, or with `quantity = 0`, is refused by a
    CHECK;
  - rows with both NULL, or `('otto-cxp/surveys_sent', 1)`, are accepted.
  Insert inside a maintenance transaction (`cxp_core::maintenance::begin`) or a pinned tenant
  transaction, as the file's other outbox tests do.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-core --test db --locked`. Expected: the new test fails.
- [ ] **Step 3:** Create `crates/cxp-core/migrations/0005_usage_outbox_meter.sql`.
  - Header comment: what it adds and why (#52), that it is additive and rollback-compatible (the
    previous release writes both columns NULL, and its shipper ignores them, shipping rows
    unmetered), why `NOT VALID`, and why `lock_timeout`.
  - Then:
    ```sql
    SET LOCAL lock_timeout = '3s';
    -- usage_outbox first: the shipper's move to rejected deletes from usage_outbox, then inserts
    -- into usage_outbox_rejected (usage.rs MOVE_TO_REJECTED), so take the locks in that order.
    ALTER TABLE usage_outbox
      ADD COLUMN meter text,
      ADD COLUMN quantity bigint,
      ADD CONSTRAINT usage_outbox_meter_quantity
        CHECK ((meter IS NULL) = (quantity IS NULL) AND (quantity IS NULL OR quantity > 0)) NOT VALID;
    ALTER TABLE usage_outbox_rejected
      ADD COLUMN meter text,
      ADD COLUMN quantity bigint,
      ADD CONSTRAINT usage_outbox_rejected_meter_quantity
        CHECK ((meter IS NULL) = (quantity IS NULL) AND (quantity IS NULL OR quantity > 0)) NOT VALID;
    ```
  - No policy or grant changes: existing table-level grants and RLS cover new columns. Confirm
    that 0001's grants on these tables are table-level, not column lists.
- [ ] **Step 4:** Re-run the step 2 command. Expected: it passes. Then run the full test gate,
  clippy, fmt and the migrations check. Expected: all pass, and the check prints nothing.
- [ ] **Step 5:** Commit `feat(core): add meter and quantity to the usage outbox`.

### Task 2: Metered charge, budget enforcement, and the BillingBlocked error

Model: opus

The core change. The workspace must compile and pass at the end of the task, so this task also
updates every caller of what it changes: `Meter::new` and `Error::QuotaExceeded` users in cxp-mcp,
cxp-api and cxp-server, and their tests. Tool descriptions, routes and docs are later tasks.

- [ ] **Step 1:** Write failing tests.
  - In `crates/cxp-core/src/usage.rs` `mod tests`:
    - `BILLABLE` is exactly `send_survey`, `submit_feedback`, `submit_survey_response`.
    - `meter_for` maps them to `SURVEYS_SENT`/`RESPONSES_COLLECTED`.
    - `meter_for("create_survey")` and `meter_for("whoami")` are `None`.
    - `METER_DECLARATIONS` keys are exactly the two meters, each prefixed `otto-cxp/`.
    - The blocked-reason mapping: `Some("payment_required")` gives `PaymentRequired`;
      `Some("budget_exceeded")`, `Some("something_new")`, and `None` with `budget.blocked` all give
      `BudgetExceeded`; not blocked gives nothing.
    - `Status::new`:
      - It reports both declared meters in `METER_DECLARATIONS` order, with the platform's
        quantity, or 0 when the platform omits a meter. A meter the platform reports that is not
        declared here is dropped.
      - It mirrors `budget` and `blocked_reason`.
      - `enforced` is always true.
      - `warning` is true when `billable_blocked()`, or when `budget.percent_used >= Some(80)`.
        Otherwise it is false, including `percent_used: None` and a plan that includes 0 legacy
        operations.
  - In `crates/cxp-core/tests/lifecycle.rs`, rewrite the meter tests:
    - `charge_records_one_row_per_billable_call_and_refuses_free_tools`: each tool in `BILLABLE`
      writes one row with `billable = true`, its meter, and `quantity = 1`, with distinct
      `event_id`s. `create_survey` and other free names give `internal` and write nothing.
    - `an_enforcing_meter_refuses_an_org_out_of_budget`: the fake platform's `usage_status`
      serves `blocked_reason: "budget_exceeded"` (and in a second case `"payment_required"`). The
      charge fails with code `budget_exceeded` / `payment_required`, writes nothing, and is not
      retriable. With `budget: {blocked: false, reason: "spend_unavailable"}` the charge
      succeeds.
    - `an_enforcing_meter_fails_open`: with the platform unreachable, the charge succeeds and
      records.
    - Delete `an_enforcing_meter_counts_only_this_periods_unshipped_usage`: the unshipped count is
      gone. Adjust the other tests that construct `Meter::new(…, enforce, …)`.
    - Extend the fake platform's `usage_status` handler (`lifecycle.rs:173`) so a test can set
      `blocked_reason`, `budget` and `meters` through the `Shared` state.
  - In `crates/cxp-core/src/error.rs` tests: `BillingBlocked` codes are `budget_exceeded` and
    `payment_required`, they are not retriable, and the message names the tool and the URL but no
    database text.
  - In `crates/cxp-mcp/src/error.rs` tests: both codes map to `INVALID_REQUEST` with `data.code`
    and `data.retriable: false`.
  - In `crates/cxp-api/src/error.rs` tests: both codes are `402` with body `{error, code}`.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-core --locked`. Expected: it fails to compile or the new tests fail.
- [ ] **Step 3:** Implement in `crates/cxp-core/src/usage.rs`.
  - Add `pub const SURVEYS_SENT`, `pub const RESPONSES_COLLECTED`, and
    `pub const METER_DECLARATIONS: &[(&str, &str, &str)]`:
    `(SURVEYS_SENT, "Surveys sent", "survey")` and
    `(RESPONSES_COLLECTED, "Responses collected", "response")`.
  - Define `pub const BILLABLE_METERS: &[(&str, &str)]` (tool, meter), one table. Derive
    `BILLABLE` from it: keep `pub const BILLABLE: &[&str]` with the three names, plus a unit test
    asserting it equals the table's tools, since a `const` cannot map. Add
    `pub fn meter_for(tool) -> Option<&'static str>`. `is_billable` becomes
    `meter_for(t).is_some()`.
  - Remove `Meter.enforce` and the `enforce` parameter of `Meter::new(platform, upgrade_url)`.
    `warm` always calls `status_for`.
  - `charge`:
    - Not billable stays `Error::Internal`, recording nothing.
    - With `Some(status)` and `status.billable_blocked()`, return
      `Error::BillingBlocked { reason, tool, url: self.upgrade_url.clone() }`.
    - Otherwise
      `INSERT INTO usage_outbox (org_id, user_id, tool, billable, meter, quantity) VALUES ($1, $2,
      $3, true, $4, 1)`.
    - Remove the unshipped `count(*)` and the `is_blocked()`/`QuotaExceeded` path. Keep the
      fail-open `status_for` as is.
  - Add `pub enum BlockedReason { BudgetExceeded, PaymentRequired }` (in `error.rs` or
    `usage.rs`; one definition) with `fn code(&self) -> &'static str` returning
    `otto_resource::BUDGET_EXCEEDED`/`PAYMENT_REQUIRED`. Add
    `fn blocked_reason(status: &UsageStatus) -> Option<BlockedReason>` per the mapping above.
  - `Status`:
    - Add `service: Option<String>`, `meters: Vec<MeterOut>`, `budget: Option<BudgetOut>` and
      `blocked_reason: Option<String>`.
    - `MeterOut` is `{meter, display_name, unit, quantity: u64}`. `BudgetOut` is
      `{monthly_budget_cents, spend_cents, percent_used, thresholds_reached: Vec<u8>, hard_stop,
      blocked, reason}`. Both are `#[serde(rename_all = "camelCase")]` and derive
      `schemars::JsonSchema`.
    - Doc comments mark the legacy fields deprecated (the family-wide bucket).
    - `Status::new(u)` drops the `enforced` argument and sets `enforced: true`.
    - Update the module docs: billable tools, meters, always-on enforcement that fails open.
- [ ] **Step 4:** Implement in `crates/cxp-core/src/error.rs`. Replace `QuotaExceeded` with
  `BillingBlocked { reason: BlockedReason, tool: String, url: String }`. `code()` is
  `reason.code()`, and it is not retriable. `Display` per reason, worded as the spec gives it:
  name the tool, say reads and free tools still work, who can fix it, at `{url}`, and that a
  change takes up to a minute.
- [ ] **Step 5:** Map the error in the other crates:
  - `crates/cxp-mcp/src/error.rs`: `"access_revoked" | "budget_exceeded" | "payment_required" =>
    INVALID_REQUEST`. Update the module's code list.
  - `crates/cxp-api/src/error.rs`: `BillingBlocked` gives `StatusCode::PAYMENT_REQUIRED`. Update
    the module's code table: `budget_exceeded`/`payment_required` are 402, and `quota_exceeded`
    is removed.
- [ ] **Step 6:** Fix every `Meter::new` caller so the workspace compiles. Do this without changing
  behavior beyond the signature: `crates/cxp-server/src/lib.rs`, `crates/cxp-api/src/lib.rs`
  tests, `crates/cxp-mcp` tests, and `crates/cxp-server/tests/e2e.rs`. Leave `config.enforce_quotas`
  in place for now (Task 5 retires it); it is simply no longer passed. Fix or replace the
  `quota_exceeded` assertions in `crates/cxp-mcp/tests/mcp.rs:835` and
  `crates/cxp-api/tests/api.rs:1204`:
  - The MCP test becomes a `budget_exceeded` refusal of `send_survey`, and asserts no outbox row.
  - The API test becomes: `POST /api/v1/surveys` still charges at this point (Task 4 makes it
    free). Rewrite it to expect 402 `{error, code: "budget_exceeded"}` for a blocked org. Task 4
    then flips it to 200 with no row.
- [ ] **Step 7:** Re-run step 2's command, then every gate. Expected: all pass.
- [ ] **Step 8:** Commit `feat(core): charge value meters and refuse billable calls when billing is
  blocked`.

### Task 3: Ship meter and quantity, and declare the meters first

Model: sonnet

- [ ] **Step 1:** Write failing tests in `crates/cxp-core/tests/lifecycle.rs`.
  - The fake platform records each shipped event's `meter`/`quantity`, as JSON fields (extend
    `ship` at `lifecycle.rs:115`). It also serves `PUT /internal/meters`, recording the declared
    keys. A `Shared` switch makes it answer 503 or 409 `service_not_set`, or reject one key in the
    receipt. Read the v0.10.0 request and receipt shapes from
    `git -C /home/robhicks/dev/otto-platform show
    v0.10.0:crates/otto-resource/src/{client.rs,types.rs}` (`MeterDeclarationBatch`,
    `MeterDeclarationReceipt`).
  - `ship_once` sends a charged `send_survey` row with `meter = "otto-cxp/surveys_sent"`,
    `quantity = 1`. A legacy row inserted with `meter`/`quantity` NULL is sent without either
    field. A retried row is sent with the same `event_id`.
  - A rejected metered row lands in `usage_outbox_rejected` with its `meter` and `quantity`.
  - `run`:
    - It declares both meters before the first `ship_usage`.
    - While declaration answers 503, nothing ships and the rows stay. Once it succeeds, they ship.
    - A shutdown while declaring returns promptly, without shipping.
    - A receipt rejecting one key still lets shipping proceed, and the other meter's rows ship.
    - Adapt `run_ships_then_flushes_on_shutdown`.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-core --test lifecycle --locked`. Expected: the new tests fail.
- [ ] **Step 3:** Implement in `crates/cxp-core/src/usage.rs`.
  - `Row` gains `meter: Option<String>` and `quantity: Option<i64>`. The claim's `RETURNING`
    gains them.
  - The `UsageEvent` gets `meter: r.meter.clone()` and `quantity`. Convert with
    `u64::try_from`. Before sending, partition out any row whose meter is set but whose quantity
    is missing, not positive, or above `otto_resource::MAX_METER_QUANTITY`. Move those straight to
    `usage_outbox_rejected` with reason `invalid_quantity`, through the existing move path. Never
    send a guessed quantity. This is unreachable under the CHECK, but keeps the shipper total.
  - `MOVE_TO_REJECTED`/`INSERT_REJECTED` carry `meter, quantity`.
  - Add `async fn declare_meters(db, platform, cfg, shutdown) -> bool`, modeled on otto-flags'
    (`git -C /home/robhicks/dev/otto-flags show origin/main:crates/flags-core/src/usage.rs`,
    `declare_meters`):
    - It loops on `platform.declare_meters(&METER_DECLARATIONS…)` with `base_backoff` doubling
      to `max_backoff`, returning `false` as soon as `shutdown` flips.
    - On success it logs `info!("declared usage meters")` with the count, and logs each
      `receipt.rejected` entry at `error` with its key and reason.
    - On failure it logs at `error` on the first failure, then at most once a minute. The log
      carries the status code or "transport", and the count of rows waiting (`SELECT count(*)
      FROM usage_outbox` in a maintenance transaction; on failure log without it). The error's
      body text goes at `debug` only.
  - `run` calls it first and returns `Ok(())` without the final pass if it returns `false`.
    Update `run`'s and the module's docs.
- [ ] **Step 4:** Re-run step 2's command, then every gate. Expected: all pass. The
  `cxp-server` e2e may now need its fake platform to serve `PUT /internal/meters`. If e2e fails
  only for that, add the route there in this task, answering `{"declared": […], "rejected": []}`
  in the receipt's shape.
- [ ] **Step 5:** Commit `feat(core): ship usage meters and declare them before shipping`.

### Task 4: MCP and REST surfaces

Model: sonnet

- [ ] **Step 1:** Write failing tests.
  - `crates/cxp-mcp/tests/mcp.rs`:
    - `create_survey` succeeds for an org whose platform status is `budget_exceeded`, and writes
      no `usage_outbox` row.
    - `send_survey` writes one row with meter `otto-cxp/surveys_sent`.
    - `submit_feedback` and `submit_survey_response` each write one row with
      `otto-cxp/responses_collected`.
    - `usage` and `whoami` results carry `meters` (both keys), `budget` and `blockedReason`.
    - The server instructions mention `send_survey`, `submit_feedback` and
      `submit_survey_response`, do not call `create_survey` billed, and name both meters.
  - `crates/cxp-api/tests/api.rs`: `POST /api/v1/surveys` for a blocked org returns 200 and writes
    no `usage_outbox` row. This replaces Task 2's 402 expectation. No route in
    `cxp_api::routes()` writes a usage row: extend whatever route-coverage test exists, or
    assert it for the write routes.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-mcp -p cxp-api --locked`. Expected: the new tests fail.
- [ ] **Step 3:** Implement the MCP side.
  - `crates/cxp-mcp/src/tools/feedback.rs`:
    - `create_survey` drops `warm_quota` and `charge`, and its description ends "Needs cxp:write.
      Free."
    - `send_survey`'s description says "Billed on otto-cxp/surveys_sent."
    - `submit_feedback`'s and `submit_survey_response`'s say "Billed on
      otto-cxp/responses_collected."
  - `crates/cxp-mcp/src/server.rs`: rewrite the `INSTRUCTIONS` billing sentence: "You are billed
    for send_survey (surveys sent) and for submit_feedback and submit_survey_response (responses
    collected); create_survey and everything else, including whoami and usage, is free. Call
    usage to see where you stand." Update "four" to "three" in the doc comments.
  - `crates/cxp-mcp/src/tools/org.rs`: `usage` description: this service's own meters
    (`otto-cxp/surveys_sent`, `otto-cxp/responses_collected`) this month, the org's otto-cxp
    budget, and `blockedReason`. Only send_survey, submit_feedback and submit_survey_response add
    to them. Free. If `warning` is true, tell the person you are working for. `whoami`'s
    description says it reports the meters and budget, or null when the platform cannot say. Also
    update the module doc ("how much budget is left").
  - `crates/cxp-mcp/src/tools/mod.rs`: keep the source-scan tests green over the new `BILLABLE`,
    and update the doc comment at line 12.
- [ ] **Step 4:** Implement the REST side.
  - `crates/cxp-api/src/routes/surveys.rs`: `create` drops `warm` and `charge`; delete
    `CHARGED_AS`.
  - `crates/cxp-api/src/lib.rs`: if nothing in `cxp-api` uses `AppState.meter` any more (check
    `routes/me.rs` and every handler), remove the field and the constructor parameter, and fix
    the callers in `cxp-server` and the tests. Update the crate doc (`lib.rs:27`): no route
    records usage, and a REST route added for a billable operation must charge the same meter.
- [ ] **Step 5:** Re-run step 2's command, then every gate. Expected: all pass.
- [ ] **Step 6:** Commit `feat(mcp): make create_survey free and report meters and budget`.

### Task 5: Retire CXP_ENFORCE_QUOTAS and point refusals at the platform

Model: haiku

- [ ] **Step 1:** Write failing tests in `crates/cxp-server/src/config.rs` `mod tests`.
  - Config parses with `CXP_ENFORCE_QUOTAS` set to `yes`, to `yes-please` (formerly a parse
    error, test at `config.rs:718`), or unset.
  - There is no `enforce_quotas` field. A function `retired_vars(&vars) -> Vec<&'static str>` (or
    similar) reports `CXP_ENFORCE_QUOTAS` when it is set, so startup can warn.
  - `upgrade_url()` returns the platform URL with no trailing `/`.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-server --lib --locked`. Expected: the new tests fail.
- [ ] **Step 3:** Implement.
  - Remove `enforce_quotas` from `Config`, its `Debug`, the boot log field (`boot.rs:252`) and the
    tests.
  - At startup, after logging is initialized, log one `warn!` per retired variable that is set:
    "CXP_ENFORCE_QUOTAS is retired and ignored: billable calls are refused whenever the platform
    reports the org blocked (budget_exceeded, payment_required)". Use the same place other
    startup warnings are logged.
  - `upgrade_url()` becomes `self.platform_url.trim_end_matches('/').to_string()`, with its doc
    updated: v0.10.0 has no `/settings/billing`.
- [ ] **Step 4:** Re-run step 2's command, then every gate. Expected: all pass.
- [ ] **Step 5:** Commit `feat(server): retire CXP_ENFORCE_QUOTAS`.

### Task 6: End-to-end metering and docs

Model: sonnet

- [ ] **Step 1:** Write a failing e2e test in `crates/cxp-server/tests/e2e.rs`.
  - Through the real router and a real shipper, against the fake platform, call `send_survey`,
    `submit_feedback`, `submit_survey_response` and `create_survey` over `/mcp`.
  - The platform receives the meter declaration first, then exactly three events: one
    `surveys_sent` and two `responses_collected`, each quantity 1, with distinct event ids, and
    none for `create_survey`.
  - Update the coverage check at `e2e.rs:1603` that keys on `BILLABLE`.
- [ ] **Step 2:** Run `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test -p
  cxp-server --test e2e --locked`. Expected: the new test fails, or fails to compile.
- [ ] **Step 3:** Make it pass. Only test-harness changes should be needed, since Tasks 2–4 built
  the behavior. If production code must change, report DONE_WITH_CONCERNS saying what and why.
- [ ] **Step 4:** Update the docs.
  - `CLAUDE.md`:
    - Rewrite the "### Metering" pattern and the **Metering** invariant to the rule in Global
      Constraints: meters declared at shipper start; no REST route records usage; a REST route
      added for a billable operation must charge the same meter.
    - In the Configuration table, drop the `CXP_ENFORCE_QUOTAS` row, or mark it "retired:
      ignored, warns".
    - In "Crates", the `usage` description becomes "metering: value meters, declaration, outbox
      shipper".
    - Keep each edit to one or two lines; reasoning lives in the code docs.
  - `.env.example`: remove the `CXP_ENFORCE_QUOTAS` line.
  - `docs/deploy/fly.md`:
    - Where it describes usage and quota (around line 894), describe the two meters, the
      declaration, the new boot-log marker `declared usage meters`, and always-on enforcement
      that fails open.
    - Note the rollout overlap: the previous release ships metered rows unmetered while both run.
    - Add `declared usage meters` to the list of boot markers (around line 856).
- [ ] **Step 5:** Run every gate. Expected: all pass.
- [ ] **Step 6:** Commit `docs: describe value metering and the retired quota flag`.
