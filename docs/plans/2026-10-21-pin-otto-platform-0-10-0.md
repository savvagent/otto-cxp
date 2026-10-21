# Pin otto-platform 0.10.0 Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Move `otto-tenant` and `otto-resource` to otto-platform v0.10.0 together, with no
behavior change.

**Architecture:** A rev bump in the root `Cargo.toml`, a lockfile update limited to the two
packages, and the new struct fields filled with their empty values in `cxp-core`'s usage module.

**Tech Stack:** Rust, cargo.

**Ticket:** savvagent/otto-cxp#52 (PR 1 of 2; this PR carries `Refs #52`)

**Spec:** `docs/specs/2026-10-21-pin-otto-platform-0-10-0-design.md`

## Global Constraints

- Worktree only: `/home/robhicks/dev/otto-cxp/.claude/worktrees/chore-52-pin-otto-platform-0-10-0`;
  trunk changes only through the PR.
- Gates:
  - `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo fmt --all -- --check`
  - `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - `podman build -t otto-cxp:check .` (Cargo files changed)
- Evidence: report pass counts per test binary. A `cargo test` panic about `DATABASE_URL` means the
  variable is missing; it is not a pass.
- Invariants: `otto-tenant` and `otto-resource` share one rev; metering is unchanged; no migration.
- Commit format: `chore: <imperative, lower-case>`, with a body that explains why and ends
  `Refs #52`. No attribution of any kind.
- Out-of-band: none.

## File Structure

- Modify: `Cargo.toml`, `Cargo.lock`, `crates/cxp-core/src/usage.rs`

### Task 1: Move the pin

Model: haiku

- [x] **Step 1:** In `Cargo.toml`, change both `rev = "fa1ae08c…"` to
  `rev = "bfda55e155be8985761850a7c42980cc926fd4cb"`.
- [x] **Step 2:** Run `cargo update -p otto-tenant -p otto-resource`. Expected: only those two
  packages change, gaining the `hmac` and `sha2` edges.
- [x] **Step 3:** Run `cargo build --workspace --all-targets`. Expected: it fails with E0063 on the
  `UsageEvent` literal in `ship_once` and the `UsageStatus` literal in the `usage()` test helper.
- [x] **Step 4:** Set `service: None, meter: None, quantity: None` on the event, and
  `service: None, meters: Vec::new(), budget: None, blocked_reason: None` on the status.
- [x] **Step 5:** Run every gate in Global Constraints. Expected: all pass.
- [x] **Step 6:** Commit with `chore: pin otto-platform 0.10.0 for service meters`.
