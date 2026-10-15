# Console refresh hardening Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Enforce the console refresh store's time budget in the database, make
`delete_generation` lease-aware, prove the lookup policy survives `DROP OWNED BY`, and finish the
enabling runbook.

**Architecture:** `begin_live_bounded` sets transaction-local `lock_timeout` and
`statement_timeout` before the lifecycle lock; `store_refreshed` and `release_lease` use it with
`STORE_TIMEOUTS`; a compile-time check in `cxp-http` ties those timeouts and the pool's acquire
timeout to `STORE_ALLOWANCE`.

**Tech Stack:** Rust, sqlx (runtime queries), PostgreSQL 16.

**Ticket:** savvagent/otto-cxp#31 (hardening only; `Refs #31`, the issue stays open)

**Spec:** `docs/specs/2026-10-15-console-refresh-hardening-design.md`

## Global Constraints

- Gates (worktree `/home/robhicks/dev/otto-cxp/.claude/worktrees/fix-31-console-refresh-hardening`,
  this run's own Postgres on :15463, never :15435 or a Fly database):
  - test: `DATABASE_URL=postgres://cxp:cxp@localhost:15463/otto_cxp cargo test --workspace --locked`
  - lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - format: `cargo fmt --all -- --check`
  - extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - docker: `podman build -t otto-cxp:check .` (crates/** changed), once before the PR
- Evidence: pass counts per test binary; a panic for a missing `DATABASE_URL` is a missing
  variable, not a pass.
- Invariants: request paths open with `platform_events::begin_live` (the bounded variant is the
  same function plus `SET LOCAL`s); no SQL outside `cxp-core`, runtime `sqlx::query` only, values
  bound, never formatted; database error text never reaches a response; no migration.
- Worktree only; trunk changes only through the PR.
- Commit format: `<type>(<scope>): <imperative, lower-case>`, body explains why and ends
  `Refs #31`; no attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers).

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-core/src/lib.rs` | `pub const POOL_ACQUIRE_TIMEOUT` |
| **Modify.** `crates/cxp-core/src/platform_events.rs` | `Timeouts`, `begin_live_bounded`, `pub const LIFECYCLE_LOCK` |
| **Modify.** `crates/cxp-core/src/console_sessions.rs` | `STORE_TIMEOUTS`, `STORE_STATEMENTS`; bounded store/release; lease-aware `delete_generation`; docs |
| **Modify.** `crates/cxp-http/src/console/mod.rs` | budget assertion, docs, `delete_generation` call |
| **Modify.** `crates/cxp-server/src/boot.rs` | use `POOL_ACQUIRE_TIMEOUT` |
| **Modify.** `crates/cxp-core/tests/console_sessions.rs` | timeout and stale-lease delete tests |
| **Modify.** `crates/cxp-core/tests/db.rs` | `DROP OWNED BY` survival test |
| **Modify.** `docs/deploy/fly.md` | enabling step 4: live verification |

## Task Order & Rationale

Task 1 (bounded store) and Task 2 (lease-aware delete) both change `console_sessions.rs` and its
caller; they are separate commits. Task 3 is test-only. Task 4 is docs. Each task leaves the
workspace building and green.

---

### Task 1: Enforce the store budget

Model: opus

- [ ] 1. In `crates/cxp-core/tests/console_sessions.rs` add three `#[sqlx::test]`s:
  (a) a second connection from the pool begins a transaction and runs
  `SELECT 1 FROM console_sessions WHERE id_hash = $1 FOR UPDATE` (the pool's superuser bypasses
  RLS); with that held, `store_refreshed` against a freshly claimed lease returns `Err` within
  10 s (wrap in `tokio::time::timeout(10 s)` and assert the inner result is `Err`); release the
  holder; `load` shows the old refresh token, generation 0, and the lease still set;
  (b) the same for `release_lease` (assert `Err`, lease still set);
  (c) a second connection holds `SELECT pg_advisory_xact_lock($1, hashtext($2::text))` with
  `platform_events::LIFECYCLE_LOCK` and the org; `store_refreshed` returns `Err` within 10 s;
  after release, the same store succeeds.
  Also assert in (a), after the failure, that a following `load` on the pool still works (the
  connection is not poisoned) and that `SHOW lock_timeout` on a fresh pool connection is the
  server default (`0`).
- [ ] 2. Run `DATABASE_URL=postgres://cxp:cxp@localhost:15463/otto_cxp cargo test -p cxp-core --test console_sessions --locked`: (c) fails to compile until `LIFECYCLE_LOCK` is public; once it compiles, (a)-(c) hang to the 10 s timeout and fail.
- [ ] 3. Implement: `crates/cxp-core/src/lib.rs` `pub const POOL_ACQUIRE_TIMEOUT: std::time::Duration = Duration::from_secs(5);` with a doc line; `crates/cxp-server/src/boot.rs:187` uses it.
  `platform_events.rs`: make `LIFECYCLE_LOCK` `pub` (doc: the advisory-lock class of an org's
  lifecycle lock); add
  `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Timeouts { pub lock: Duration, pub statement: Duration }`
  (std `Duration`) and `pub async fn begin_live_bounded(db, org, user, t: Timeouts)`; both it and
  `begin_live` call one private `begin_live_inner(db, org, user, Option<Timeouts>)` that, when
  `Some`, runs
  `SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $2, true)` with
  the millisecond values bound as strings, right after `db.begin(org)` and before the advisory
  lock.
  `console_sessions.rs`: `pub const STORE_TIMEOUTS: Timeouts = Timeouts { lock: 2 s, statement: 3 s }`
  and `pub const STORE_STATEMENTS: u32 = 4` with docs naming the four (lifecycle lock,
  revocation check, `UPDATE`, `COMMIT`); `store_refreshed` and `release_lease` open with
  `begin_live_bounded(db, org, None, STORE_TIMEOUTS)`; module docs say the store's waits are
  bounded in the database.
  `cxp-http/src/console/mod.rs`: in the `const _` block assert
  `POOL_ACQUIRE_TIMEOUT + STORE_TIMEOUTS.statement * STORE_STATEMENTS <= STORE_ALLOWANCE`
  (compare `as_millis()`); update `STORE_ALLOWANCE`'s doc and the module docs' bullet ("the
  statements are single-row") to describe the enforced bound and that an `org.deleted` purge
  now makes the store time out rather than wait.
- [ ] 4. Run the test command from step 2: all pass. Then
  `DATABASE_URL=postgres://cxp:cxp@localhost:15463/otto_cxp cargo test --workspace --locked`,
  clippy, fmt.
- [ ] 5. Commit `fix(core): bound the console refresh store's waits in the database` with a why
  body ending `Refs #31`.

### Task 2: Lease-aware `delete_generation`

Model: sonnet

- [ ] 1. In `crates/cxp-core/tests/console_sessions.rs` add a test: claim with a 200 ms lease,
  sleep 400 ms, claim again (newer lease, same generation); `delete_generation` with the stale
  lease returns 0 and the session still loads; with the newer lease it returns 1 and the session
  is gone. Update the two existing calls near line 265 to pass a `Lease`.
- [ ] 2. Run `DATABASE_URL=postgres://cxp:cxp@localhost:15463/otto_cxp cargo test -p cxp-core --test console_sessions --locked`: fails to compile (signature).
- [ ] 3. Change `delete_generation(db, org, id_hash, lease: Lease)` to add
  `AND refresh_lease_until = $4` and bind `lease.until`; doc: a stale holder never deletes a
  session someone else has claimed anew. Update the caller in
  `crates/cxp-http/src/console/mod.rs` (`InvalidGrant` arm) to pass `lease`.
- [ ] 4. Run the step 2 command, then the workspace test, clippy, fmt.
- [ ] 5. Commit `fix(core): delete a refused console session only under its own lease`, why body,
  `Refs #31`.

### Task 3: The lookup policy survives `DROP OWNED BY`

Model: sonnet

- [ ] 1. In `crates/cxp-core/tests/db.rs` add
  `the_console_lookup_policy_survives_drop_owned_by_its_previous_owner`: create roles
  `old` and `new` (`NOLOGIN NOSUPERUSER NOBYPASSRLS`, unique names); `ALTER TABLE console_sessions`
  and `ALTER FUNCTION console_session_org(bytea)` `OWNER TO old`; as a control,
  `CREATE POLICY cxp_test_named ON console_sessions FOR SELECT TO old USING (false)`; seed a
  session; then the runbook's steps: `REASSIGN OWNED BY old TO new`, `DROP OWNED BY old`,
  `DROP ROLE old`. Assert: `console_sessions_lookup` still exists with unchanged `roles` and
  `qual`; `cxp_test_named` is gone (so `DROP OWNED BY` did act on policies); table and function
  are owned by `new`; the lookup as `otto_app` unpinned returns the seeded org. Run the body in a
  spawned task and always clean up (`REASSIGN OWNED BY new TO <me>`, `DROP OWNED BY new`,
  `DROP ROLE IF EXISTS` both), as the existing ownership test does.
- [ ] 2. Run `DATABASE_URL=postgres://cxp:cxp@localhost:15463/otto_cxp cargo test -p cxp-core --test db --locked console_lookup`: passes (it guards behavior that already holds; verify it can
  fail by temporarily giving the lookup policy `TO old` in the test only, see it fail, revert).
- [ ] 3. Workspace test, clippy, fmt.
- [ ] 4. Commit `test(core): prove the console lookup policy survives drop owned by`, why body,
  `Refs #31`.

### Task 4: Enabling runbook

Model: haiku

- [ ] 1. In `docs/deploy/fly.md` "## Web console sign-in", after step 3, add step 4 (live
  verification, from the issue): `curl -s https://otto-cxp.savvagent.com/auth/session` shows
  `loginEnabled: true`; sign in at `/console/` and the customers list loads; a cross-site write
  (e.g. `curl -X POST` with the session cookie and `Origin: https://example.com` to an `/api/v1`
  write route) answers `403 cross_site_request`; signing out ends the session (the cookie no
  longer works; `/auth/session` reports signed out). Note the order constraint (secret staged
  before the `fly.toml` change deploys). Replace the stale "`/` is a JSON `404` until #29" text in
  "## Verify" only if it is wrong on main (it is: #29 shipped).
- [ ] 2. Commit `docs(deploy): add live verification to the console enabling runbook`, `Refs #31`.
