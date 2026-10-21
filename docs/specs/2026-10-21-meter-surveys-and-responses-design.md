# Meter surveys sent and responses collected — design

**Status:** DRAFT
**Issue:** #52 "Meter surveys sent and responses collected; make create_survey free" (PR 2 of 2)
**Plan:** `docs/plans/2026-10-21-meter-surveys-and-responses.md`
**Builds on:** `docs/specs/2026-10-21-pin-otto-platform-0-10-0-design.md` (PR 1, the pin to
otto-platform v0.10.0)

## Brief

> ## Problem
>
> `cxp-core`'s `BILLABLE` list (`crates/cxp-core/src/usage.rs:52`) charges `create_survey` (agent
> management) alongside `send_survey`, `submit_feedback` and `submit_survey_response`, which are
> customer-facing volume. Under the platform's new pricing (savvagent/otto-platform#55),
> management calls are free under fair use and each service bills its value meters.
>
> ## Approach
>
> - `create_survey` becomes free, including over `POST /api/v1/surveys`.
> - Two meters:
>   - `otto-cxp/surveys_sent` is charged by `send_survey` (one customer per call, so quantity 1).
>   - `otto-cxp/responses_collected` is charged by `submit_feedback` and `submit_survey_response`.
> - REST routes for the same operations charge the same meters, so REST is not a billing bypass.
> - Bump `otto-tenant`/`otto-resource` to the platform release with meter, quantity and
>   `budget_exceeded`, and declare both meters at startup. Update the server instructions'
>   billing sentence.
>
> ## Acceptance criteria
>
> - [ ] `create_survey` (MCP and REST) ships no usage event.
> - [ ] `send_survey` ships one `surveys_sent` event, and `submit_feedback` and
>   `submit_survey_response` each ship one `responses_collected` event, deduplicated by
>   `event_id`.
> - [ ] A `budget_exceeded` refusal maps to MCP `code: budget_exceeded` and a REST 402
>   `{error, code}`.
> - [ ] `whoami` and `usage` report both meters and the budget state.
>
> ## Out of scope
>
> - Prices, and the platform-side billing (savvagent/otto-platform#55).
>
> ## Risks & open items
>
> - Should `responses_collected` be billed when a response arrives from an end customer rather
>   than through an agent? Owner: @robhicks.
> - Blocked on the platform release that adds meters (savvagent/otto-platform#55).

Ticket comments (summarized; the source is #52):

- The platform half shipped in otto-platform v0.10.0. Refuse billable calls when
  `billable_blocked()` is true; that covers both `budget_exceeded` and `payment_required`. Let
  free calls through. Enforcement fails open when the platform can't be reached.
- The production slug is set (`service=otto-cxp`), so declarations as `otto-cxp/<meter>` are
  accepted.

## Premise corrections

1. **No REST route performs `send_survey`, `submit_feedback` or `submit_survey_response`.** The
   only REST write on these tables is `POST /api/v1/surveys` (create), plus survey update and
   delete (`crates/cxp-api/src/lib.rs:208-213`). `GET /api/v1/feedback` and
   `GET /api/v1/surveys/{id}/responses` are reads. The approach bullet "REST routes for the same
   operations charge the same meters" is therefore satisfied vacuously: REST has no billable
   route after this change. A test pins that no REST route writes a usage row. If a REST route
   for one of the three operations is added later, it must charge the same meter. The metering
   invariant (updated below) says so.
2. **The ticket's blocker is gone.** PR 1 (#53) pins v0.10.0, and the slug is set in production.
3. **The open item has a trivial answer today.** No end-customer path exists. Every survey
   response and feedback item comes from an authenticated agent or app calling the MCP tool;
   there is no public form, widget endpoint or webhook. So every `responses_collected` event is
   agent-initiated, and the question "bill a response that arrives from an end customer?" has no
   case to decide yet. This spec bills every successful `submit_feedback` and
   `submit_survey_response`. The PR calls the question out again for Rob, for when such a path is
   built.

## Assumptions

- **Meter names and units.** `otto-cxp/surveys_sent` uses display name "Surveys sent" and unit
  `survey`. `otto-cxp/responses_collected` uses "Responses collected" and unit `response`.
  Pricing is out of scope, and display names can be re-declared later.
- **Quantity is always 1.** Each billable call writes exactly one metered row. `send_survey` sends
  to one customer, and each `submit_*` call stores one item.
- **Enforcement is always on.** `CXP_ENFORCE_QUOTAS` gated the legacy family-wide quota, which
  v0.10.0 no longer enforces (`hard_stop` is always `false`). The budget hard stop is the org
  owner's own opt-in, and `payment_required` is the platform's call, so neither should depend on
  a deploy flag. otto-factory (#223) and otto-flags (#146) retired their `*_ENFORCE_QUOTAS` the
  same way. `CXP_ENFORCE_QUOTAS` stays accepted, and setting it only logs a startup warning, so a
  leftover setting cannot stop a boot.
- **The refusal's link is the platform root URL**, as otto-factory did. v0.10.0 has no
  `/settings/billing`. `PlatformClient::usage_page_url` needs the org slug, which costs a platform
  call on the refusal path.
- **`service` on shipped events stays `None`.** The platform derives the service from this
  server's credential, and sending it would only add a `service_mismatch` failure mode. otto-flags
  does the same.
- No Run Profile value was assumed: the Development Workflow section sets every key this run
  uses.

## Goal & success criteria

Bill otto-cxp by value: surveys sent and responses collected, each on its own declared meter.
Make `create_survey` free, refuse billable calls when the platform says the org is blocked, and
report the meters and budget to agents.

- `create_survey`, over MCP and over `POST /api/v1/surveys`, writes no `usage_outbox` row.
- Each successful `send_survey` writes one row with `meter = 'otto-cxp/surveys_sent'` and
  `quantity = 1`. Each successful `submit_feedback` and `submit_survey_response` writes one row
  with `meter = 'otto-cxp/responses_collected'` and `quantity = 1`. Each row has its own
  `event_id`, which the shipper sends unchanged on every retry. A failed call rolls its row back.
- The shipper declares both meters before shipping anything, and ships each row's `meter` and
  `quantity`.
- When the platform's status for the org has `billable_blocked()`, the three billable tools are
  refused, with nothing written. The code is `budget_exceeded` or `payment_required` (MCP
  `data.code`; REST would answer 402 `{error, code}`). Free tools are never refused. A failed
  status lookup lets the call through.
- `whoami` and `usage` report `meters` (both meters, with this month's quantity), `budget` and
  `blockedReason`.

## Scope

**In:**

- metering in `cxp-core` (`usage`), with a new migration for the outbox's meter columns
- the error variant and its MCP and REST mappings
- the MCP tools' charge sites, descriptions and server instructions
- the REST survey-create route no longer charging
- `whoami`/`usage` output
- `cxp-server` config (`CXP_ENFORCE_QUOTAS` retired) and boot
- tests and docs: `CLAUDE.md` Metering, the Invariants, the config table, `.env.example`, and
  `docs/deploy/fly.md` where it describes metering

**Out:**

- prices and platform billing
- a public end-customer response path
- re-declaring a meter after a later `unknown_meter`; a follow-up if it is ever needed
- the TypeScript SDK and React packages: they do not model usage status (checked in the plan)
- usage-based caps, rate limits or fair-use limits on free tools

## Architecture

### Meters (`crates/cxp-core/src/usage.rs`)

- `pub const SURVEYS_SENT: &str = "otto-cxp/surveys_sent";` and
  `pub const RESPONSES_COLLECTED: &str = "otto-cxp/responses_collected";`
- `pub const METER_DECLARATIONS: &[(&str, &str, &str)]` holds (key, display name, unit) for both.
- `BILLABLE` becomes the three tools `send_survey`, `submit_feedback` and
  `submit_survey_response`. A new `pub fn meter_for(tool: &str) -> Option<&'static str>` maps each
  one to its meter. `is_billable(tool)` is `meter_for(tool).is_some()`, so BILLABLE and the map
  are defined once, as one table: `pub const BILLABLE_METERS: &[(&str, &str)]`, with `BILLABLE`
  derived from it or replaced by it. The plan settles the exact form; the rule is one definition.
- `Meter::charge(tx, user, tool)` keeps its signature. A tool that is not billable stays
  `Error::Internal`, written nothing. It writes
  `INSERT INTO usage_outbox (org_id, user_id, tool, billable, meter, quantity) VALUES (…, true,
  $meter, 1)`.

### Migration `crates/cxp-core/migrations/0005_usage_outbox_meter.sql` (new)

Modeled on otto-flags' `0007_usage_outbox_meter.sql`:

```sql
SET LOCAL lock_timeout = '3s';
-- usage_outbox first: the shipper's move to rejected deletes from usage_outbox, then
-- inserts into usage_outbox_rejected (MOVE_TO_REJECTED), so take the locks in that order.
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

The order follows otto-cxp's own shipper, not otto-flags'. The comment block explains
the rollout, the `NOT VALID` and the lock timeout, as the flags file does. No new table, policy
or grant: the existing RLS policies and grants cover new columns.

### Shipper (`ship_once`, `run`)

- The claim's `RETURNING` and `Row` gain `meter: Option<String>` and `quantity: Option<i64>`.
  `UsageEvent` gets `meter: r.meter.clone()` and `quantity: r.quantity.map(|q| q as u64)`. The
  CHECK keeps `quantity` positive. The conversion is a `u64::try_from` that fails closed: a row
  that somehow fails it is moved to `usage_outbox_rejected` with reason `invalid_quantity`. It is
  never sent unmetered or with a guessed value.
- The move to `usage_outbox_rejected` copies `meter` and `quantity`.
- A legacy row (`meter IS NULL`, written by the previous release during rollout) ships unmetered,
  exactly as before.
- **Declare first.** `run` first calls `declare_meters` with `METER_DECLARATIONS` and ships
  nothing until the platform accepts them. It retries with the shipper's backoff
  (`base_backoff` doubling to `max_backoff`). The first failure, and then at most one a minute,
  logs at `error` with the HTTP status or transport kind and how many rows are waiting. The
  platform's own text goes at `debug` only. Rows stay queued, nothing is lost, and they count
  toward nothing until shipped. A receipt with `rejected` entries is logged at `error` per key
  and treated as declared, since that is a deploy bug to fix, not a reason to stall all usage.
  The plan pins this behavior. Success logs `declared usage meters` at `info`, a new boot-log
  marker. A shutdown while declaring returns at once, and the rows stay for the next process.
  This mirrors otto-flags' `declare_meters`.

### Enforcement (`Meter`)

- `enforce` goes away; `Meter::new(platform, upgrade_url)` loses the flag. Callers in
  `cxp-server` change with it.
- `warm(org)` always looks the status up. It is still a platform call before the transaction, and
  `PlatformClient` caches it for 60 s.
- `charge` reads the cached status (`status_for`, which keeps its 2 s timeout and its 5 s
  per-org fail-open window). It refuses when `status.billable_blocked()`:
  - code `PAYMENT_REQUIRED` when `blocked_reason` is `payment_required`;
  - otherwise `BUDGET_EXCEEDED`, covering `blocked_reason = budget_exceeded`, an unknown future
    reason, and a budget with `blocked` but no reason.
  The legacy `is_blocked()` check and the unshipped-row count go away. Under v0.10.0 they could
  never refuse.
- No status (lookup failed or timed out) lets the call through and records it: fail open.

### Error (`crates/cxp-core/src/error.rs`)

- `Error::QuotaExceeded` is replaced by
  `Error::BillingBlocked { reason: BlockedReason, tool: String, url: String }`, where
  `BlockedReason` is `BudgetExceeded | PaymentRequired`.
  - `code()` is `budget_exceeded` or `payment_required`.
  - It is not retriable.
  - `Display`:
    - budget: "this organization's otto-cxp budget for this month is spent and its hard stop is
      on, so {tool} was refused. Reads and free tools still work. An owner or admin can raise the
      budget or turn the hard stop off at {url}; a change takes up to a minute to apply."
    - payment: "this organization's billing needs a payment method in good standing, so {tool}
      was refused. Reads and free tools still work. An owner can fix it at {url}; a change takes
      up to a minute to apply."
  The plan may tighten the wording.
- **MCP** (`crates/cxp-mcp/src/error.rs`): both codes join `access_revoked` under
  `ErrorCode::INVALID_REQUEST`, with `data.code` and `data.retriable: false`.
- **REST** (`crates/cxp-api/src/error.rs`): both codes are `402 Payment Required` with the usual
  `{error, code}` body. The module's code table changes to match. `quota_exceeded`/429 is no
  longer produced.

### Charge sites

- MCP `crates/cxp-mcp/src/tools/feedback.rs`:
  - `create_survey` loses `warm_quota` and `charge`.
  - The other three keep them.
  - Descriptions: `create_survey` ends "Free."; `send_survey` says it is billed on surveys sent;
    `submit_feedback` and `submit_survey_response` say they are billed on responses collected.
- MCP `crates/cxp-mcp/src/server.rs`:
  - The `INSTRUCTIONS` billing sentence becomes: send_survey is billed on surveys sent;
    submit_feedback and submit_survey_response on responses collected; create_survey and
    everything else is free; call usage to see where you stand.
  - The doc comments that say "four" say "three".
  - The test that checks `INSTRUCTIONS` names every billable tool stays.
- MCP `crates/cxp-mcp/src/tools/mod.rs`: the source-scan test that pins which tools call
  `.charge(` keeps working over the new `BILLABLE`.
- REST `crates/cxp-api/src/routes/surveys.rs`: `create` no longer warms or charges, and
  `CHARGED_AS` goes. If `AppState.meter` then has no user in `cxp-api`, it is removed from
  `AppState`. The plan checks `routes/me.rs` and the console paths first. The crate doc
  (`crates/cxp-api/src/lib.rs:27`) says no route records usage.

### `whoami` / `usage` (`usage::Status`, MCP `out`)

`Status` gains these fields (camelCase on the wire, `schemars` schema):

- `service: Option<String>`
- `meters: Vec<MeterOut>`, each `{meter, displayName, unit, quantity}`. Always both meters: a
  declared meter the platform omits is reported with quantity 0, so the list is stable for
  agents.
- `budget: Option<BudgetOut>`, mirroring `BudgetStatus`: `{monthlyBudgetCents, spendCents,
  percentUsed, thresholdsReached, hardStop, blocked, reason}`.
- `blockedReason: Option<String>`: the code a billable call would be refused with now (`budget_exceeded` or `payment_required`), else null. Implementation note: it was first specified as the platform's raw value; review changed it so the report always matches the refusal, including an unknown future reason or a blocked budget with no reason.

The legacy fields (`plan`, `includedOps`, `billableUsed`, `remaining`, `totalCalls`,
`periodStart`, `hardStop`) stay, so no consumer breaks, and are documented as deprecated.

- `enforced` is always `true`.
- `warning` changes meaning: true when billable calls are blocked, or when the budget's
  `percentUsed` is at least 80. When `percentUsed` is unknown (`None`), only the blocked state sets it. It no longer reflects the deprecated family-wide bucket, whose
  "always warn on a plan that includes none" would now be noise.

The `usage` tool's description says what it reports and that only the three tools add to the
meters. `whoami`'s description changes "allowance" to the meters and budget. This is an additive
wire change; the `warning` and `enforced` meaning changes are named here.

### Config and boot (`crates/cxp-server`)

- `config.rs`: `enforce_quotas` is removed from `Config`. If `CXP_ENFORCE_QUOTAS` is set, at any
  value, startup logs one `warn` that it is retired and ignored. A non-boolean value no longer
  fails config parsing, so a leftover setting cannot stop a boot. The tests change to match.
- `upgrade_url()` becomes the platform URL itself (trailing `/` trimmed), still named for the
  refusal link. The plan may rename it `billing_url`.
- `lib.rs`/`boot.rs`: `Meter::new` without the flag, and the boot log line drops
  `enforce_quotas`.
- `.env.example`: remove `CXP_ENFORCE_QUOTAS`. `fly.toml` does not set it (checked). `CLAUDE.md`'s
  config table drops the row, or marks it retired.

### Public interface changes

| Interface | Change | Kind |
|---|---|---|
| MCP `create_survey`, REST `POST /api/v1/surveys` | free, records no usage | additive (cheaper) |
| MCP `whoami`/`usage` result | adds `service`, `meters`, `budget`, `blockedReason`; `enforced` always `true`; `warning` from the budget | additive; two field meanings change |
| MCP error `quota_exceeded`, REST 429 `quota_exceeded` | no longer produced; already inert against v0.10.0 | breaking in principle |
| MCP errors `budget_exceeded`, `payment_required`; REST 402 | new | additive |
| `CXP_ENFORCE_QUOTAS` | retired, logs a warning | breaking in principle (ignored, never fatal) |
| Tool descriptions, server instructions | billing wording | doc |

The squash commit is `feat(core)!:` with a `BREAKING CHANGE:` footer naming the two
breaking-in-principle rows, as otto-flags #146 did.

## Invariants impact

- **Metering**: rewritten in `CLAUDE.md` (Critical Patterns and Invariants) to say:
  - Exactly `send_survey` (`otto-cxp/surveys_sent`), `submit_feedback` and
    `submit_survey_response` (`otto-cxp/responses_collected`) record one metered usage row each,
    quantity 1, in the tool's own transaction, into `usage_outbox`.
  - No REST route records one; a REST route added for one of these operations must charge the
    same meter.
  - Nothing else records one.
  - The shipper declares the meters before it ships.
  - The source-scan tests in `cxp-mcp` enforce it.
- **Append-only migrations**: 0005 is new; 0001–0004 are untouched. It is additive, and the
  previous release keeps working against it (see Migration).
- **Tenant isolation**: no new table. The existing outbox policies (pinned for tenant writes,
  maintenance-marker for the shipper) cover the new columns. The tenant-isolation test needs no
  change; the plan confirms.
- **Request paths use `begin_live`**: unchanged. `warm` stays before the transaction, so no
  pooled connection waits on the platform.
- **No SQL outside `cxp-core`**: holds; all new SQL is in `usage.rs` and the migration.
- **Untrusted text / no database text in responses**: the refusal message carries only constants,
  the tool name, and the configured URL.
- **Auth**: `whoami`/`usage` stay scope-free; billable tools still require `cxp:write` before
  anything else.
- **Small shared database**: `charge` loses a `count(*)` per billable call; nothing unbounded is
  added.
- **otto-platform pin**: unchanged from PR 1.

## Migration

`0005_usage_outbox_meter.sql`, above. Rollout compatibility:

- **The previous release against the new schema.** It inserts rows without `meter`/`quantity`
  (both NULL, allowed by the CHECK). Its shipper selects only the old columns and ships those
  rows unmetered. It also ships the new release's metered rows unmetered while both run. With no
  prices set, nothing is under- or over-billed in money. The quantities are under-counted only
  for the overlap; `docs/deploy/fly.md` notes this. `MIGRATOR.ignore_missing` lets it boot.
- **The new release against old rows**: they ship unmetered.
- **Locks**: two short `ACCESS EXCLUSIVE` locks with a 3 s `lock_timeout`; a timeout fails the
  boot's migration and the release retries. `NOT VALID` skips the scan.

## Out-of-band

- Migration 0005 (agent): it applies at boot; confirm `applying migrations` in the boot logs.
- The platform service slug `otto-cxp` (human, Rob): already set in production, per the #52
  thread. Nothing to do.
- Config: `CXP_ENFORCE_QUOTAS` retired, no new key. If it is set as a Fly secret or env, unsetting
  it is optional and Rob's (`fly secrets unset -a otto-cxp CXP_ENFORCE_QUOTAS`, only if `fly
  secrets list` shows it). `fly.toml` does not set it.
- No new secret, scope, role change or recurring cost.

## Error handling & edge cases

- **Declaration fails** (platform down, 401, 409 `service_not_set`): nothing ships, rows queue,
  and the error log repeats at most once a minute. Billable calls still succeed and record.
- **Declaration receipt rejects a key**: logged at `error`; shipping proceeds. The platform
  rejects that meter's events as `unknown_meter`. They are moved aside to
  `usage_outbox_rejected` per the existing path, and the log names the reason.
- **The status lookup times out** during `charge`: the call goes through (5 s fail-open window
  per org, as today).
- **Blocked mid-flight**: two concurrent calls may both pass a just-stale cached status. The
  refusal is soft at the boundary, as today.
- **`create_survey` while blocked**: allowed (free).
- **An unknown future `blocked_reason`**: refused as `budget_exceeded`, which tells the agent to
  have an owner check the console. Tested.
- **A metered row with an unrepresentable quantity**: impossible under the CHECK. Handled fail
  closed, as above.

## Testing approach

All under `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace
--locked`:

- `crates/cxp-core/src/usage.rs` unit tests:
  - `meter_for`/`BILLABLE`: exactly the three tools, with their meters; `create_survey` is not
    billable.
  - `Status::new` maps meters (filling a missing declared meter with 0), budget and blocked
    reason; `warning` and `enforced` follow the new rules.
  - The blocked-reason-to-code mapping, including an unknown reason.
- `crates/cxp-core/tests/lifecycle.rs` (the existing meter tests, rewritten):
  - Each billable tool writes one row with the right meter, quantity 1, a distinct `event_id`;
    `create_survey` is refused as not billable.
  - A refused (blocked) charge writes nothing and returns the right code.
  - A failed lookup lets the call through.
  - The shipper sends `meter`/`quantity` and re-sends the same `event_id` on retry.
  - Rejected rows keep their meter and quantity.
  - The shipper declares before shipping, and does not ship while declaring fails.
  - A legacy NULL-meter row ships unmetered.
  - These use the existing fake-platform harness in that file; the plan names it.
- `crates/cxp-core/tests/db.rs`: whatever schema assertions cover the outbox, extended for the new
  columns and constraints (the plan checks the FK/index assertions there still hold).
- `crates/cxp-mcp/tests/mcp.rs`:
  - `create_survey` writes no outbox row.
  - `send_survey`/`submit_*` write the right meter.
  - A blocked org gets `data.code` `budget_exceeded` / `payment_required`.
  - `whoami`/`usage` carry `meters`, `budget` and `blockedReason`.
  - Replaces the current `quota_exceeded` test at `mcp.rs:835`.
- `crates/cxp-api/tests/api.rs`:
  - `POST /api/v1/surveys` writes no outbox row, even for a blocked org (it is free).
  - Replaces the 429 test at `api.rs:1204`.
  - The 402 mapping is pinned in `crates/cxp-api/src/error.rs`'s unit test, since no REST route
    can raise it.
- `crates/cxp-server`:
  - config tests: `CXP_ENFORCE_QUOTAS` at any value parses, and the field is gone.
  - e2e (`tests/e2e.rs`): the fake platform serves `PUT /internal/meters`. A billable call's event
    reaches the platform with its meter and quantity after the declaration. The test at
    `e2e.rs:1603` that keys on `BILLABLE` follows the new set.

## Risks & open questions

- **End-customer responses** (ticket open item, owner Rob): no such path exists today, so all
  responses are billed. Revisit when a public response path is built.
- **Overlap under-count** during a deploy (old shipper ships new rows unmetered). Harmless while
  prices are unset; documented.
- **Re-declaration**: a meter rejected at runtime (`unknown_meter`) after a platform-side reset
  dead-letters until the next restart re-declares. Same as otto-flags (their #144); filed as a
  follow-up if it bites.
- **`warning` meaning change** could surprise an agent that relied on the legacy bucket. Named in
  the PR and in the tool description.
