# Pin otto-platform 0.10.0 — design

**Status:** DRAFT
**Issue:** #52 "Meter surveys sent and responses collected; make create_survey free" (PR 1 of 2)
**Plan:** `docs/plans/2026-10-21-pin-otto-platform-0-10-0.md`

## Brief

#52 asks otto-cxp to bill two value meters (`otto-cxp/surveys_sent`, `otto-cxp/responses_collected`)
and make `create_survey` free. One of its approach bullets:

> Bump `otto-tenant`/`otto-resource` to the platform release with meter, quantity and
> `budget_exceeded`, and declare both meters at startup.

The repo's Development Workflow puts an otto-platform pin move in its own PR ("otto-platform pin
(agent, own PR)"). This PR is that move and nothing else. The metering PR follows it and closes #52.
Ticket comments: the platform half shipped in otto-platform v0.10.0 (savvagent/otto-platform#84),
and this service's slug is set in production (`service=otto-cxp`).

## Premise corrections

None.

## Assumptions

- **v0.10.0 is rev `bfda55e155be8985761850a7c42980cc926fd4cb`** (the `v0.10.0` tag, "chore:
  release 0.10.0 (#85)"). otto-flags pins the same rev.
- **The platform already serves v0.10.0 in production.** Its usage-status body keeps the legacy
  fields and always sends `hard_stop: false`, so `UsageStatus::is_blocked()` is already never true
  for otto-cxp, whatever crate version it links. This PR does not change that. The metering PR
  replaces the legacy check with `billable_blocked()`.

## Goal & success criteria

Move `otto-tenant` and `otto-resource` together to v0.10.0 with no behavior change, so the metering
PR starts from the new types.

- Both crates name the same rev in the root `Cargo.toml`.
- `Cargo.lock` changes only the two platform packages and their new `hmac`/`sha2` edges.
- The new `UsageEvent` fields (`service`, `meter`, `quantity`) are `None` for every shipped event.
  serde leaves them off the wire, so the platform reads the events exactly as before.
- Every gate passes.

## Scope

**In:** `Cargo.toml`, `Cargo.lock`, the two struct literals in `crates/cxp-core/src/usage.rs` that
v0.10.0's new fields break, and this spec and plan.

**Out:** everything #52 asks for beyond the pin: meters, declarations, `create_survey` becoming
free, the budget refusal, `whoami`/`usage` output, and docs.

## Architecture

- `Cargo.toml:29-30`: both `rev`s become `bfda55e…`.
- `crates/cxp-core/src/usage.rs` `ship_once`: the `UsageEvent` built from each outbox row sets
  `service: None, meter: None, quantity: None`. That is an unmetered event, which the platform
  records (and counts as billable or not) as before.
- `crates/cxp-core/src/usage.rs` test helper `usage()`: the `UsageStatus` literal gains
  `service: None, meters: Vec::new(), budget: None, blocked_reason: None`.

No public interface of otto-cxp changes.

## Invariants impact

- **otto-platform pin:** holds. Both crates move to one rev.
- **Metering:** unchanged. The same four tools record the same rows.
- Nothing else is touched.

## Migration

None.

## Out-of-band

None: no secret, config key, scope, migration, role change, or recurring cost.

## Error handling & edge cases

- An older platform would still parse the unmetered events, since the new fields are left off the
  wire. That is moot because production runs v0.10.0.
- `UsageStatus` decodes with the new fields defaulted (`#[serde(default)]`), so test doubles that
  serve the old body still parse.

## Testing approach

No new tests: the change is type-level and adds no behavior. The full gates prove it. That
includes the shipper tests in `crates/cxp-core` and the e2e tests in `crates/cxp-server/tests`,
which drive a fake platform.

## Risks & open questions

- None beyond what the metering PR carries.
