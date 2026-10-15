# Split the migration owner from the runtime role (#27) Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** The serving process connects as a role that owns nothing and cannot truncate or add
triggers; migrations run as a separate owner login in Fly's release command; a role change after
boot fails `/readyz` within the hour.

**Architecture:** `cxp_core::db_role` learns what a role owns and which tables it may `TRUNCATE`
or add a `TRIGGER` to; `cxp-server` refuses an owning runtime role and `CXP_RUN_MIGRATIONS=true`
in a deployment, gains `cxp-server migrate` (connecting with `CXP_MIGRATION_DATABASE_URL`), and
re-checks the role in the hourly sweep, raising a readiness alarm. Migration 0005 grants the one
privilege the runtime needs beyond `otto_app` (`INSERT` on `deleted_orgs`) to the group role
`cxp_runtime`.

**Tech Stack:** Rust (axum, sqlx runtime queries, tokio), Postgres 16, Fly.io.

**Ticket:** savvagent/otto-cxp#27

**Spec:** `docs/specs/2026-10-15-split-migration-owner-design.md`

## Global Constraints

- Gates, from the worktree root, against this worktree's own Postgres container (never :15435,
  never a Fly database):
  - test: `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test --workspace --locked`
  - lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - format: `cargo fmt --all -- --check`
  - extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - image (once, before the PR): `podman build -t otto-cxp:check-issue27 .`
- Evidence: report pass counts per test binary. `cargo test` without `DATABASE_URL` panics
  rather than skipping; a panic there is a missing variable, not a pass. Skipped tests are not a
  pass.
- Invariants that apply: append-only, additive migrations (0005 is new; never edit 0001–0004);
  tenant isolation is RLS, proven at boot; `SECURITY DEFINER` functions keep running as their
  owner and `console_sessions_lookup` follows `console_session_org`'s owner; no SQL outside
  `cxp-core`; runtime `sqlx::query*` only, never `query!`; database error text never reaches a
  response; the pool stays small (`migrate` uses at most 2 connections); no unbounded catalog
  lists (cap at `MAX_REPORTED`).
- Human-only (never run): any production DDL/role change, `fly secrets set/import`, a hand
  `fly deploy`. The runbook is written, not executed.
- Worktree only (`/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-27-split-migration-owner`);
  trunk changes only through the PR.
- Commit format: `<type>(<scope>): <imperative, lower-case>`; body explains why and ends
  `Refs #27`. No attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers) anywhere.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-core/src/db_role.rs` | `OwnedObject`, `TablePrivilege`, `DbRole::{owned, owned_total, table_privileges}`, `owns_objects`, `can_alter_tables`, summary clause, `RUNTIME_GRANTS`, `missing_runtime_grants` |
| **Create.** `crates/cxp-core/migrations/0005_runtime_role_grants.sql` | `GRANT INSERT ON deleted_orgs TO cxp_runtime` when that role exists |
| **Modify.** `crates/cxp-core/tests/db.rs` | catalog tests for ownership, table privileges, runtime grants |
| **Modify.** `crates/cxp-server/src/config.rs` | `migration_database_url` (`CXP_MIGRATION_DATABASE_URL`) |
| **Modify.** `crates/cxp-server/src/health.rs` | `RoleAlarm`, `Readiness`, `/readyz` reason `database role refused` |
| **Modify.** `crates/cxp-server/src/boot.rs` | `Verdict`, `role_verdict`, `check_db_role` (serving), `check_migration_role`, `migration_url`, `migrate`, `run_with` changes, `recheck_db_role`, sweep integration |
| **Modify.** `crates/cxp-server/src/lib.rs` | `router(..., readiness: impl Into<health::Readiness>)`; re-exports |
| **Modify.** `crates/cxp-server/src/main.rs` | `migrate` subcommand |
| **Modify.** `crates/cxp-server/tests/e2e.rs` | split-role e2e tests |
| **Modify.** `fly.toml`, `.env.example`, `docs/deploy/fly.md`, `CLAUDE.md` | release command, config, runbook, docs |

## Task Order & Rationale

Core catalog reads first (the server's checks consume them), then the server, then the e2e proof
of the whole split, then the docs and runbook that describe what the code now refuses.

---

### Task 1: What the role owns and may truncate (`cxp-core`)

Model: opus (catalog SQL with membership semantics).

- [ ] **Step 1: failing tests** in `crates/cxp-core/tests/db.rs` (reuse `TestRoles`; extend its
  `Drop` to run `REASSIGN OWNED BY <name> TO CURRENT_USER; DROP OWNED BY <name>` per role before
  `DROP ROLE`, since owner roles now own objects in the test database):
  - `the_database_role_check_sees_what_a_role_owns_and_can_truncate`: as the migrated test
    database:
    - a login that is only a member of `otto_app`: `owned` empty, `owned_total == 0`,
      `table_privileges` empty, `!can_alter_tables()`, summary ends with
      `it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table`
      (before any shared-instance clause).
    - a login made owner of one table (`ALTER TABLE tickets OWNER TO <role>`): `owned` contains
      `("table", "tickets")`, `can_alter_tables()`.
    - a login that is a member of a NOLOGIN role owning a table: reported with `owner` = the
      NOLOGIN role.
    - a login owning the database (`ALTER DATABASE <db> OWNER TO <role>`): reports kind
      `database` and schema `public` (through `pg_database_owner`).
    - a login granted `TRUNCATE ON customers`: `table_privileges` names `customers` with
      `TRUNCATE`; another granted `TRIGGER ON feedback`: names `TRIGGER`.
    - the superuser pool: `owned` empty (skipped, like the other per-user reads).
  - `missing_runtime_grants_names_the_tombstone_insert`: superuser → empty; a login in
    `otto_app` only → `["INSERT on deleted_orgs"]`; after `GRANT INSERT ON deleted_orgs TO
    cxp_runtime` (create `cxp_runtime` with the `DO $$ … EXCEPTION WHEN duplicate_object OR
    unique_violation` pattern, never drop it) and `GRANT cxp_runtime TO <login>` → empty; on an
    unmigrated database (`#[sqlx::test(migrations = false)]` variant or a fresh `TestRoles`
    database) → empty.
  - `migration_0005_grants_the_tombstone_insert_to_cxp_runtime`: with `cxp_runtime` existing
    before migrating (`#[sqlx::test(migrations = false)]`, create the role, run
    `cxp_core::migrate`), `has_table_privilege('cxp_runtime', 'deleted_orgs', 'INSERT')` is true.
  - Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test -p cxp-core --test db the_database_role_check_sees_what missing_runtime migration_0005`
    — expect compile failure / failures.
- [ ] **Step 2: implement** in `crates/cxp-core/src/db_role.rs` per the spec's "cxp_core::db_role"
  section: types `OwnedObject { user, kind: &'static str, name, owner }`,
  `TablePrivilege { user, table, privileges: Vec<&'static str> }`; fields `owned`, `owned_total`,
  `table_privileges` on `DbRole` (update `for_test` to leave them empty); per non-superuser user,
  one query for owned objects (database, schemas excluding `pg_catalog`, `information_schema`,
  `pg_toast%`, `pg_temp%`; relations in those schemas with relkind mapped to table, partitioned
  table, view, materialized view, sequence, index, foreign table, composite type; functions;
  types excluding array types and relation row types; owner where
  `pg_has_role(user, owner, 'MEMBER')`), ordered by kind then name, `LIMIT MAX_REPORTED`, plus a
  count; one query for relations (`r`,`p`,`v`,`m`,`f`) in those schemas not owned through
  membership where `has_table_privilege(user, oid, 'TRUNCATE')` or `'TRIGGER'`, `LIMIT
  MAX_REPORTED`. Methods `owns_objects()`, `can_alter_tables()`. Summary: append the clause from
  the spec before the shared-instance clause (prefix items with `session user "…" ` /
  `current user "…" ` when the users differ, like the privilege items). Module docs: a paragraph
  on ownership and TRUNCATE/TRIGGER (#27). `pub const RUNTIME_GRANTS` and
  `pub async fn missing_runtime_grants(db: &Db) -> Result<Vec<&'static str>>` (only
  `INSERT on deleted_orgs`, only when `to_regclass('public.deleted_orgs')` is not null).
  Unit tests in the module: summary text for an owner, a TRUNCATE holder, a clean role, differing
  users; `can_alter_tables`.
- [ ] **Step 3: migration** `crates/cxp-core/migrations/0005_runtime_role_grants.sql`, exactly the
  spec's DDL with its comment.
- [ ] **Step 4: run** `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test -p cxp-core`
  — all pass (existing summary assertions in `db.rs` and `db_role.rs` that compare a whole clean
  summary string get the new clause appended; keep the existing substring assertions working);
  then clippy and fmt.
- [ ] **Step 5: commit** `feat(core): see what the database role owns and may truncate` — body:
  why ownership/TRUNCATE/TRIGGER are an RLS bypass, why `cxp_runtime`, `Refs #27`.

### Task 2: Refuse an owning runtime role, `cxp-server migrate`, re-check (`cxp-server`)

Model: opus (boot lifecycle and readiness state).

- [ ] **Step 1: failing tests** (unit, in the files below):
  - `config.rs`: `CXP_MIGRATION_DATABASE_URL` parsed into `migration_database_url`, `None` when
    unset; `{:?}` never prints it.
  - `health.rs`: `the_role_alarm_holds_readiness_back` — a `Readiness` whose alarm is set makes
    `readyz` answer 503 with `{"status":"unready","reason":"database role refused"}` (call the
    router with `tower::ServiceExt::oneshot` on a lazily-connected `Db`; the alarm is checked
    before the database, like the platform check); cleared → the platform/database path as
    before.
  - `boot.rs`: `role_verdict` table (clean, privileged, other databases, owner, TRUNCATE holder ×
    deployment/local/shared instance); `check_db_role` refuses an owner and a TRUNCATE holder in a
    deployment with an error containing `refusing to serve`, the summary, and
    `cxp-server migrate`, and allows them locally; `check_migration_role` allows an owner in a
    deployment and refuses a privileged one; `migration_url` returns the migration URL when set,
    the runtime URL only when not a deployment, and an error naming
    `CXP_MIGRATION_DATABASE_URL` in a deployment without it. Existing
    `a_deployment_refuses_a_privileged_database_role` keeps passing.
  - Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test -p cxp-server --lib`
    — expect failures.
- [ ] **Step 2: implement**
  - `config.rs`: field, parse, redacted `Debug`, `for_test` → `None`; doc comment says the
    serving process never reads it.
  - `health.rs`: `#[derive(Clone, Default)] pub struct RoleAlarm(Arc<RwLock<Option<String>>>)`
    with `raise(summary)`, `clear() -> bool` (whether it was raised), `is_raised()`;
    `pub struct Readiness { platform: PlatformCheck, role: RoleAlarm }` with `new(platform)`,
    `with_role_alarm(alarm)`, and `From<PlatformCheck>`; `router(db, readiness: impl
    Into<Readiness>)`; `/readyz` checks the platform, then the alarm (`warn` per probe, fixed
    reason), then the database. Module doc paragraph on the re-check.
  - `lib.rs`: `router(..., readiness: impl Into<health::Readiness>)` passing it through;
    re-export `boot::migrate`.
  - `boot.rs`: `pub enum Verdict { Clean, AllowedLocally, Refused }`;
    `pub fn role_verdict(role, config) -> Verdict` (serving rules:
    `is_refused_in_a_deployment() || can_alter_tables()`; deployment = `config.is_deployment() ||
    role.shares_the_instance()`); `check_db_role` built on it (messages per spec);
    `check_migration_role` (only `is_refused_in_a_deployment`); `pub fn migration_url(config) ->
    Result<&str>`; `pub async fn migrate(config: &Config) -> Result<String>` (2-connection pool,
    5 s acquire timeout, inspect, `check_migration_role`, `applying migrations`, migrate,
    `migrations applied`, close; returns the role summary). In `run_with`: after
    `check_db_role`, refuse `run_migrations` in a deployment (message per spec); log the
    not-migrating line at `info` in a deployment, the existing `warn` locally; after migrations
    and before the isolation proof, `missing_runtime_grants` → refuse in a deployment / warn
    locally (message names `GRANT cxp_runtime TO <current user>`). `pub async fn
    recheck_db_role(db, config, alarm) ` per spec; `sweep_loop` takes `Config` (or the bits it
    needs) and the alarm, and calls it after each sweep pass; `run_with` creates the alarm, passes
    `Readiness::new(check).with_role_alarm(alarm.clone())` to `router`.
  - `main.rs`: parse `std::env::args().nth(1)`: none → serve; `Some("migrate")` → init tracing,
    `boot::migrate(&config).await?`, exit 0; anything else → `anyhow::bail!` with
    `usage: cxp-server [migrate]` (no port bound for `migrate`).
- [ ] **Step 3: run** `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test -p cxp-server --lib`,
  then `cargo test --workspace --locked` with the same `DATABASE_URL` (e2e callers of
  `router(... PlatformCheck::...)` must compile unchanged; the existing least-privilege e2e test
  is expected to fail now — the owner it serves as is refused; Task 3 rewrites it. Note it in the
  report rather than weakening the check). Clippy, fmt.
- [ ] **Step 4: commit** `feat(server): refuse an owning runtime role and migrate as the owner` —
  body: release-command rationale, re-check, `Refs #27`.

### Task 3: Prove the split end to end (`tests/e2e.rs`)

Model: sonnet.

- [ ] **Step 1: tests** in `crates/cxp-server/tests/e2e.rs`, replacing
  `least_privilege_owner` with helpers that build the production shape on the test database:
  `owner_role(pool, name)` (LOGIN, not privileged, `SET` on `app.console_session_lookup`, owns
  the database, `REVOKE CONNECT … FROM PUBLIC`, `GRANT CONNECT` to it) and
  `runtime_role(pool, name)` (LOGIN, not privileged, member of `otto_app` and `cxp_runtime`,
  `GRANT CONNECT`); `otto_app` and `cxp_runtime` created idempotently, never dropped;
  `DropRoleOnExit` for both logins.
  - `the_service_runs_whole_as_a_runtime_role_that_owns_nothing` (from
    `the_service_runs_whole_as_a_least_privilege_database_owner`): `cxp_server::boot::migrate`
    with a deployment `Config` whose `migration_database_url` is the owner and `database_url` the
    runtime role; tables and `console_session_org` owned by the owner; then `run_with` as the
    runtime role with `run_migrations = false`; same requests, `org.deleted`, sweep, shipper and
    console-lookup assertions; then, as the runtime role, `ALTER TABLE customers NO FORCE ROW
    LEVEL SECURITY`, `ALTER TABLE customers DISABLE ROW LEVEL SECURITY`, `DROP POLICY
    customers_tenant_isolation ON customers`, `TRUNCATE customers` each fail (SQLSTATE `42501`);
    `boot.database_role` contains `it owns nothing in this database`; logs in order: the clean
    role line, `not migrating`, the isolation line, the platform line, `cxp-server listening`.
  - `a_deployment_refuses_an_owner_as_its_runtime_role`: owner as `database_url`,
    `run_migrations = false`, after migrating: refused, error contains `refusing to serve` and
    `owns`, nothing listens.
  - `a_deployment_refuses_to_migrate_as_the_runtime_role`: runtime role, `run_migrations = true`,
    deployment: refused, error names `CXP_RUN_MIGRATIONS`.
  - `migrate_needs_its_own_credential_in_a_deployment`: no migration URL → error names
    `CXP_MIGRATION_DATABASE_URL`, `_sqlx_migrations` absent; superuser migration URL → refused
    naming `SUPERUSER`.
  - `the_role_recheck_raises_and_clears_the_readiness_alarm`: runtime role `Db`, deployment
    config: `recheck_db_role` leaves the alarm clear; `GRANT pg_read_all_data TO <runtime>` →
    raised, and `cxp_server::health::router(db, Readiness::new(PlatformCheck::Accepted)
    .with_role_alarm(alarm))` answers 503 `database role refused`; `REVOKE` → cleared.
  - Keep `a_deployment_refuses_a_superuser_database_role` passing.
  - Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15464/otto_cxp cargo test -p cxp-server --test e2e`.
- [ ] **Step 2: mutation check** — temporarily drop `|| role.can_alter_tables()` from
  `role_verdict`; `a_deployment_refuses_an_owner_as_its_runtime_role` must fail; restore. Record
  in the report.
- [ ] **Step 3: run** the full test gate, clippy, fmt.
- [ ] **Step 4: commit** `test(server): run the whole service as a runtime role that owns nothing`
  — body, `Refs #27`.

### Task 4: Release command, config, runbook (`fly.toml`, `.env.example`, `docs/deploy/fly.md`, `CLAUDE.md`)

Model: orchestrator (docs; reviewed like code).

- [ ] **Step 1:** `fly.toml`: `[deploy] release_command = "migrate"` (the image ENTRYPOINT is `cxp-server`, so Fly runs `cxp-server migrate`);
  `CXP_RUN_MIGRATIONS = "false"` in `[env]`; header comments (roles, secrets list gains
  `CXP_MIGRATION_DATABASE_URL`).
- [ ] **Step 2:** `.env.example`: `CXP_MIGRATION_DATABASE_URL` (commented, with what it is and
  that a deployment requires it), the database comment updated for the split.
- [ ] **Step 3:** `docs/deploy/fly.md`: Infrastructure and Migrations paragraphs; secrets table
  row; "Database role" rewritten for the split (roles table, what boot refuses, the re-check);
  new runbook "Split the migration owner from the runtime role" S0–S7 with exact commands, guards,
  read-only proofs, rollback; First deploy note; Verify lines (release command, boot, `/readyz`
  reason).
- [ ] **Step 4:** `CLAUDE.md`: config table rows (`CXP_RUN_MIGRATIONS` note,
  `CXP_MIGRATION_DATABASE_URL`), Build & Run note on `cxp-server migrate`, the tenant-isolation
  invariant text (no longer "splitting … is #27"), Migrations out-of-band line (release command).
- [ ] **Step 5:** check: `cargo fmt --all -- --check`; `git diff --check`.
- [ ] **Step 6: commit** `docs(deploy): run migrations as the owner in the release command` —
  body, `Refs #27`.

### Task 5: Record as shipped (after reviews, before the PR is handed off)

- [ ] Set the spec's `**Status:**` to `IMPLEMENTED (#<PR>)` and tick this plan's boxes in one
  final docs commit (`docs: record the migration-owner split as implemented`, `Refs #27`).
