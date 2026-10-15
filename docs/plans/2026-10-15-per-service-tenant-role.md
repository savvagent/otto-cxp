# Give otto-cxp its own tenant role (#28) Implementation Plan

> Execute task-by-task (fresh implementer per task, review between tasks). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Tenant transactions assume otto-cxp's own NOLOGIN role `cxp_app` instead of the shared
`otto_app`, a deployment refuses to serve unless that role was actually assumed, and every `Db` in
the workspace is built by one helper.

**Architecture:** `cxp_core::TENANT_ROLE` and `cxp_core::tenant_db` wrap otto-tenant's
`Db::with_tenant_role`; `clippy.toml` forbids `Db::from_pool`/`Db::connect` elsewhere. Migration
0007 creates `cxp_app` where it can (failing where it cannot and it is absent) and grants it what
`otto_app` holds in `otto_cxp`. `cxp-server` refuses, in a deployment, an isolation report whose
tenant role was not assumed, requires the runtime login to inherit `cxp_app`, and warns while the
login is still a member of `otto_app`.

**Tech Stack:** Rust (sqlx runtime queries, tokio, axum), Postgres 16, otto-tenant at `fa1ae08`
(unchanged), clippy `disallowed-methods`.

**Ticket:** savvagent/otto-cxp#28

**Spec:** `docs/specs/2026-10-15-per-service-tenant-role-design.md`

## Global Constraints

- Gates, from the worktree root
  `/home/robhicks/dev/otto-cxp/.claude/worktrees/feat-28-cxp-tenant-role`, against this
  worktree's own Postgres container `cxp-pg-issue28` on `127.0.0.1:15467` (never `:15435`, never
  a Fly database). The cluster must have `otto_app` and `cxp_app` (`psql
  postgres://cxp:cxp@localhost:15467/otto_cxp -c "CREATE ROLE cxp_app NOLOGIN;"` if missing):
  - test: `DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test --workspace --locked`
  - one test binary: `DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test -p <crate> --test <file> --locked <name>`
  - lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - format: `cargo fmt --all -- --check`
  - extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'`
    prints nothing
  - image (once, before the PR): `podman build -t otto-cxp:check-issue28 .`
- Evidence: report pass counts per test binary. `cargo test` without `DATABASE_URL` panics rather
  than skipping; a panic there is a missing variable, not a pass. Skipped tests are not a pass.
- Invariants that apply: append-only, additive migrations (0007 is new; never edit 0001 to 0006);
  tenant isolation is RLS, proven at boot (now also: the tenant role was assumed); every tenant
  table grants to the tenant role; `SECURITY DEFINER` functions stay `REVOKE ALL … FROM PUBLIC`;
  request paths use `begin_live`; no SQL outside `cxp-core`; runtime `sqlx::query*` only; database
  error text never reaches a response; the otto-tenant/otto-resource rev does not move.
- Human-only (never run): any production DDL or role change, `fly secrets`, a hand `fly deploy`.
  The runbook is written, not executed.
- Worktree only; trunk changes only through the PR (this PR is stacked on
  `feat/27-split-migration-owner`).
- Commit format: `<type>(<scope>): <imperative, lower-case>`; body explains why and ends
  `Refs #28`. No attribution of any kind (no Co-Authored-By, no "Generated with", no credit
  markers) anywhere.

## File Structure

| File | Responsibility |
|---|---|
| **Modify.** `crates/cxp-core/src/lib.rs` | `TENANT_ROLE`, `tenant_db`, unit tests; crate docs say `cxp_app` |
| **Create.** `clippy.toml` | `disallowed-methods`: `otto_tenant::db::Db::{from_pool, connect}` |
| **Create.** `crates/cxp-core/migrations/0007_cxp_app_tenant_role.sql` | create `cxp_app` (or fail), grant it `otto_app`'s grants |
| **Modify.** `crates/cxp-core/src/db_role.rs` | `RUNTIME_GRANTS[1]` = `inheriting cxp_app`; `missing_runtime_grants` binds `TENANT_ROLE`, missing role counts; `SHARED_TENANT_ROLE`, `holds_shared_tenant_role`; docs |
| **Modify.** `crates/cxp-core/src/platform_events.rs` | one comment |
| **Modify.** `crates/cxp-core/tests/common/mod.rs` | `db` → `tenant_db`; `otto_app_db` → `tenant_role_db`; `as_app_*` use `TENANT_ROLE` |
| **Modify.** `crates/cxp-core/tests/{db,lifecycle,console_sessions,search_backlog,feedback,catalog,customers,search_plans,support}.rs` | `Db::from_pool` → `tenant_db`; `otto_app` → `TENANT_ROLE` where it means the tenant role; new tests in `db.rs` |
| **Modify.** `crates/cxp-api/src/lib.rs`, `crates/cxp-api/tests/{api,console}.rs` | `tenant_db`; `TENANT_ROLE` |
| **Modify.** `crates/cxp-mcp/tests/mcp.rs` | `tenant_db`; `TENANT_ROLE` |
| **Modify.** `crates/cxp-server/src/{boot,lib,health}.rs` | `tenant_db`; `check_tenant_role`; shared-role warning; `missing_grants` text |
| **Modify.** `crates/cxp-server/tests/e2e.rs` | roles (`cxp_app`), `RUNTIME_GROUPS`, log assertions, two new tests |
| **Modify.** `.github/workflows/ci.yml` | pre-create `otto_app` and `cxp_app` |
| **Modify.** `.env.example`, `fly.toml` (comment), `README.md`, `CLAUDE.md`, `docs/deploy/fly.md` | the tenant role, the rule, runbook T0 to T5, Verify, First deploy |

## Task Order & Rationale

1. The helper, the clippy rule, and every `Db` construction first: a mechanical change that, on
   its own, makes every test fail until `cxp_app` holds grants (proving the tests do use it).
   So Task 1 also carries migration 0007: the helper without the grants cannot pass.
2. `db_role` (runtime grants, shared-role check) and the new core tests, on top of the role.
3. `cxp-server` boot (refusal, warning, messages) and e2e.
4. CI and docs (runbook), once the behavior and log lines are final.

---

### Task 1: `TENANT_ROLE`, `tenant_db`, the clippy rule, migration 0007, every `Db` through the helper

**Files:**
- Modify: `crates/cxp-core/src/lib.rs`
- Create: `clippy.toml`
- Create: `crates/cxp-core/migrations/0007_cxp_app_tenant_role.sql`
- Modify: every file listed by `grep -rn 'Db::from_pool\|Db::connect' crates` (25 sites:
  `crates/cxp-api/src/lib.rs:301`, `crates/cxp-api/tests/api.rs:234,1270`,
  `crates/cxp-api/tests/console.rs:210`, `crates/cxp-core/tests/search_backlog.rs:187`,
  `crates/cxp-core/tests/console_sessions.rs:660`, `crates/cxp-core/tests/feedback.rs:804`,
  `crates/cxp-core/tests/common/mod.rs:23,53`, `crates/cxp-core/tests/lifecycle.rs:1290`,
  `crates/cxp-core/tests/db.rs:1512,1727`, `crates/cxp-mcp/tests/mcp.rs:176,304,742,888`,
  `crates/cxp-server/src/health.rs:279`, `crates/cxp-server/src/boot.rs:280,475`,
  `crates/cxp-server/src/lib.rs:452,1037`, `crates/cxp-server/tests/e2e.rs:220,2374,2596,2825,2864,2879`)
- Modify: every test that names `otto_app` as the role tenant work runs as (`SET ROLE otto_app`,
  `SET LOCAL ROLE otto_app`, `has_table_privilege('otto_app', …)` meaning "the tenant role",
  `GRANT otto_app TO <test login>` meaning "the tenant role", e2e `RUNTIME_GROUPS`, e2e log
  assertions `tenant isolation enforced as role "otto_app"`)
- Test: `crates/cxp-core/src/lib.rs` (unit), `crates/cxp-core/tests/db.rs`

**Interfaces:** produces `cxp_core::TENANT_ROLE: &str = "cxp_app"` and
`cxp_core::tenant_db(pool: sqlx::PgPool) -> otto_tenant::Db`, used by every later task.

**Out-of-band / invariants:** new migration 0007 (append-only; additive; `otto_app`'s grants
stay). The role creation in production is human-only (runbook T1, Task 4). Do not touch
`scripts/bench-search`.

**Model:** sonnet — mechanical across many files, with one migration fully specified here.

- [ ] **Step 1: Write the failing unit test** in `crates/cxp-core/src/lib.rs`'s `mod tests`:

```rust
    #[test]
    fn the_tenant_role_is_otto_cxps_own_and_every_db_assumes_it() {
        assert_eq!(super::TENANT_ROLE, "cxp_app");
        assert!(otto_tenant::db::validate_role_name(super::TENANT_ROLE).is_ok());
        assert_ne!(super::TENANT_ROLE, otto_tenant::db::DEFAULT_TENANT_ROLE);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://x@127.0.0.1:1/none")
            .unwrap();
        assert_eq!(super::tenant_db(pool).tenant_role(), super::TENANT_ROLE);
    }
```

`connect_lazy` needs a tokio runtime only when a connection is made; if it panics outside one,
make the test `#[tokio::test] async fn` instead (tokio is a dev-dependency of cxp-core; check
`crates/cxp-core/Cargo.toml`).

And in `crates/cxp-core/tests/db.rs`, change `tenant_isolation_is_enforced_on_every_tenant_table`
(line ~180) so it asserts the new role:

```rust
    let db = db(pool);
    let report = db.verify_tenant_isolation().await.expect("isolation");
    assert_eq!(report.tenant_role, cxp_core::TENANT_ROLE);
    assert!(report.tenant_role_assumed, "SET LOCAL ROLE {} was not issued", cxp_core::TENANT_ROLE);
    assert_eq!(report.effective_role, cxp_core::TENANT_ROLE);
    let summary = report.summary();
    assert!(
        summary.contains(r#"tenant isolation enforced as role "cxp_app" (assumed via SET LOCAL ROLE cxp_app)"#),
        "{summary}"
    );
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p cxp-core --lib --locked the_tenant_role` → compile error (`TENANT_ROLE` not
found). Run: `DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test -p cxp-core --test db --locked tenant_isolation_is_enforced_on_every_tenant_table`
→ FAIL (role is `otto_app`), once `lib.rs` compiles.

- [ ] **Step 3: Implement the helper** in `crates/cxp-core/src/lib.rs`, after `POOL_ACQUIRE_TIMEOUT`:

```rust
/// The role every tenant transaction assumes (`SET LOCAL ROLE`): otto-cxp's own (#28), never
/// the `otto_app` the otto services on a shared instance may share, so a privilege another
/// service grants `otto_app` never reaches a tenant transaction here. A NOLOGIN role holding
/// `otto_cxp`'s tenant grants and nothing else (migration 0007); the runtime login is a member
/// of it (and inherits its grants for the background work it does as itself).
pub const TENANT_ROLE: &str = "cxp_app";

/// The only way to make an [`otto_tenant::Db`] in this workspace: `Db::from_pool` and
/// `Db::connect` default to the shared `otto_app` and are forbidden elsewhere (`clippy.toml`),
/// so production and every test pin [`TENANT_ROLE`] alike.
///
/// otto-tenant skips `SET LOCAL ROLE` when the role is missing or not assumable rather than
/// failing; `cxp-server`'s boot refuses that in a deployment (`boot::check_tenant_role`).
pub fn tenant_db(pool: sqlx::PgPool) -> otto_tenant::Db {
    #[allow(clippy::disallowed_methods)] // the one sanctioned constructor
    otto_tenant::Db::from_pool(pool)
        .with_tenant_role(TENANT_ROLE)
        .expect("TENANT_ROLE is a valid role name")
}
```

Update the crate doc comment's "runs under row-level security as `otto_app`" to "as `cxp_app`
([`TENANT_ROLE`])".

- [ ] **Step 4: Add `clippy.toml`** at the workspace root:

```toml
# Every otto_tenant::Db is built by cxp_core::tenant_db, which pins otto-cxp's own tenant role
# (cxp_core::TENANT_ROLE, #28). Db::from_pool and Db::connect default to the shared otto_app.
disallowed-methods = [
  { path = "otto_tenant::db::Db::from_pool", reason = "use cxp_core::tenant_db, which pins cxp_core::TENANT_ROLE (#28)" },
  { path = "otto_tenant::db::Db::connect", reason = "use cxp_core::tenant_db, which pins cxp_core::TENANT_ROLE (#28)" },
]
```

Then prove the rule bites before fixing the call sites: `cargo clippy --workspace --all-targets
--locked -- -D warnings 2>&1 | grep -c disallowed_methods` must be > 0 (one per site). If it is 0,
the path is wrong: try `otto_tenant::Db::from_pool`, and record which path worked.

- [ ] **Step 5: Write migration 0007** at `crates/cxp-core/migrations/0007_cxp_app_tenant_role.sql`,
exactly:

```sql
-- otto-cxp's own tenant role (#28). Tenant transactions SET LOCAL ROLE cxp_app
-- (cxp_core::TENANT_ROLE) instead of otto_app, which the otto services on otto-db may share: a
-- privilege any of them grants otto_app would otherwise reach otto-cxp's tenant transactions.
--
-- Roles are cluster-wide. Create it where the migrating role may (tests, a local superuser),
-- tolerating a concurrent creator. Where it may not (production's cxp_owner has no CREATEROLE)
-- it must already exist (docs/deploy/fly.md, runbook T1); otherwise this migration fails, so the
-- release command aborts the deploy instead of recording 0007 with no grants made.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cxp_app') THEN
    BEGIN
      CREATE ROLE cxp_app NOLOGIN;
    EXCEPTION
      WHEN duplicate_object OR unique_violation THEN
        NULL; -- another database in this cluster created it first
      WHEN insufficient_privilege THEN
        RAISE EXCEPTION 'role cxp_app does not exist and % may not create it', current_user
          USING HINT = 'As a superuser: CREATE ROLE cxp_app NOLOGIN; GRANT cxp_app TO <the '
                       'runtime login>; then migrate again (docs/deploy/fly.md, runbook T1).';
    END;
  END IF;
END $$;

-- The same grants 0001, 0003, and 0004 gave otto_app, one statement each over every table,
-- sequence, and function in public, then 0001's three narrowings. otto_app keeps its own: the
-- previous release runs tenant transactions as otto_app until this one replaces it, and again
-- after a rollback. A later migration revokes them.
GRANT USAGE ON SCHEMA public TO cxp_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO cxp_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO cxp_app;
-- Includes the two SECURITY DEFINER functions, expire_removed_members() and
-- console_session_org(bytea): both are REVOKE ALL ... FROM PUBLIC and granted to the tenant role
-- by name, as 0001 and 0003 did for otto_app.
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO cxp_app;
-- Tables and sequences a later migration creates as the migrating role (cxp_owner in
-- production) get the same.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO cxp_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO cxp_app;
-- As 0001 for otto_app: a tombstone is permanent and written by the runtime role (cxp_runtime,
-- 0005), never by a tenant transaction; removed members expire only through
-- expire_removed_members(); the migrator's ledger is not the app's.
REVOKE INSERT, UPDATE, DELETE ON deleted_orgs FROM cxp_app;
REVOKE DELETE ON removed_members FROM cxp_app;
REVOKE ALL ON public._sqlx_migrations FROM cxp_app;
```

- [ ] **Step 6: Move every `Db` construction to the helper.** `Db::from_pool(x)` →
`cxp_core::tenant_db(x)`; `Db::connect(url).await.unwrap()` →
`cxp_core::tenant_db(sqlx::PgPool::connect(url).await.unwrap())` (e2e). In
`crates/cxp-core/tests/common/mod.rs`: `db(pool)` returns `cxp_core::tenant_db(pool)`; rename
`otto_app_db` to `tenant_role_db` (update every caller) and make its `after_connect` run
`SET ROLE cxp_app` built from `cxp_core::TENANT_ROLE` (`format!("SET ROLE {}", cxp_core::TENANT_ROLE)`;
the closure needs an owned `String` or a `&'static str` const); `as_app_unpinned` runs
`SET LOCAL ROLE` + `TENANT_ROLE`. Update the module docs to say the tenant role (`cxp_app`).
Remove now-unused `use otto_tenant::Db` imports.

- [ ] **Step 7: Replace `otto_app` where it means "the tenant role"** in tests. Rule: if the test
is about what a tenant transaction or the runtime's inherited grants can do, use
`cxp_core::TENANT_ROLE` (in SQL strings via `format!`). Keep `otto_app` only where the test is
about the shared role itself: (a) `the_schema_keeps_its_structural_invariants` checks the grant
matrix for BOTH roles (loop over `[cxp_core::TENANT_ROLE, "otto_app"]`, message naming the role);
(b) the db_role tests that assert a grant to `otto_app` makes a database reachable
("through \"otto_app\"") keep that case. In e2e: `shared_roles` creates `otto_app`, `cxp_app`,
`cxp_runtime`; `RUNTIME_GROUPS = ["cxp_app", "cxp_runtime"]` (doc: "the tenant role and the
tombstone grant"); every `tenant isolation enforced as role "otto_app"` assertion becomes
`"cxp_app"`; the two `split_with(&pool, &["otto_app"])` tests (lines ~2893, ~2936: "a member of
the tenant role but not `cxp_runtime`") become `&[cxp_core::TENANT_ROLE]`. In
`crates/cxp-core/tests/db.rs` the runtime-grant test at ~2316-2380 still expects
`"inheriting otto_app"` for now; Task 2 changes it (leave its `otto_app` grants alone in this
task, and if it fails here because the role check is still `otto_app`, it should not: the check is
unchanged in Task 1).

- [ ] **Step 8: Run the gates**

`cargo clippy --workspace --all-targets --locked -- -D warnings` → clean (no
`disallowed_methods`). `cargo fmt --all`. Then
`DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test --workspace --locked` → all
pass, 0 skipped. The two Step 1 tests pass.

- [ ] **Step 9: Commit**

```bash
git add clippy.toml crates/
git commit -m "feat(core): run tenant transactions as otto-cxp's own cxp_app role" -m "otto_app is shared by the otto services on otto-db, so a privilege any of them grants it reaches otto-cxp's tenant transactions. cxp_core::tenant_db builds every Db with cxp_core::TENANT_ROLE (otto-tenant's with_tenant_role), clippy forbids the default-role constructors elsewhere so tests cannot drift from production, and migration 0007 gives cxp_app the grants otto_app holds here, leaving otto_app's for the previous release.

Refs #28"
```

---

### Task 2: the runtime grants and the shared-role check in `db_role`, and the new core tests

**Files:**
- Modify: `crates/cxp-core/src/db_role.rs` (`RUNTIME_GRANTS` ~244, `missing_runtime_grants` ~812,
  module docs ~5, `OTHER_SERVICE_DATABASES` docs ~100)
- Modify: `crates/cxp-core/src/platform_events.rs:273` (comment only)
- Test: `crates/cxp-core/tests/db.rs`

**Interfaces:** consumes `TENANT_ROLE`, `tenant_db` (Task 1). Produces
`db_role::RUNTIME_GRANTS = ["INSERT on deleted_orgs", "inheriting cxp_app"]`,
`db_role::SHARED_TENANT_ROLE: &str = "otto_app"`, and
`pub async fn holds_shared_tenant_role(db: &Db) -> Result<bool>`, used by Task 3.

**Out-of-band / invariants:** no SQL outside cxp-core; runtime queries only; no database text in
responses (these are boot-only).

**Model:** sonnet — small, specified catalog queries and tests.

- [ ] **Step 1: Write the failing tests** in `crates/cxp-core/tests/db.rs`:
  - Change the existing runtime-grant test (~2316-2390,
    `missing_runtime_grants_names_the_tombstone_insert` or its current name): logins are granted
    `cxp_core::TENANT_ROLE` instead of `otto_app`; expected lists become
    `["INSERT on deleted_orgs", "inheriting cxp_app"]` and `["inheriting cxp_app"]`; add a case: a
    login that is a member of `otto_app` and `cxp_runtime` but not `cxp_app` lacks
    `["inheriting cxp_app"]`.
  - New `#[sqlx::test(migrator = "cxp_core::MIGRATOR")] async fn holds_shared_tenant_role_sees_membership_in_otto_app(pool)`:
    using the file's existing role-guard helper pattern (see how the runtime-grant test creates
    and drops logins, and its `as_role` pools), a login member of `otto_app` → `true`; a login
    member of `cxp_app` and `cxp_runtime` only → `false`.
  - New `async fn cxp_app_holds_exactly_what_otto_app_holds(pool)`: for every relation in
    `public` (`relkind IN ('r','p','v','m','f')`) and each of SELECT, INSERT, UPDATE, DELETE,
    TRUNCATE, REFERENCES, TRIGGER; every sequence (`relkind = 'S'`) and USAGE, SELECT, UPDATE;
    every function in `public` and EXECUTE; and schema `public` and USAGE, CREATE: collect rows
    where `has_*_privilege('cxp_app', oid, p) IS DISTINCT FROM has_*_privilege('otto_app', oid, p)`
    in one SQL query per kind, and assert each list is empty (print the differences). Also assert
    the SECURITY DEFINER pair explicitly: `has_function_privilege('cxp_app',
    'public.console_session_org(bytea)', 'EXECUTE')` and `'public.expire_removed_members()'` are
    true, and `has_function_privilege('public', …)` is false for both.
  - New `async fn tenant_transactions_cannot_write_the_tombstone(pool)`: create a login (password,
    no privileges) that is a member of `cxp_app` and `cxp_runtime` (create `cxp_runtime` with the
    file's existing duplicate-tolerant `DO $$ … CREATE ROLE cxp_runtime NOLOGIN …` pattern and
    `GRANT INSERT ON deleted_orgs TO cxp_runtime`), connect a pool as it, build
    `cxp_core::tenant_db(pool)`. Unpinned on the pool: `INSERT INTO deleted_orgs (org_id) VALUES ($1)`
    succeeds. Inside `db.begin(org).await` the same insert fails with SQLSTATE `42501`. (Check the
    `deleted_orgs` columns in `0001_baseline.sql` first and insert whatever its NOT NULL columns
    need.) Close the pool and drop the login at the end, as the neighbouring tests do.
  - New `async fn the_previous_release_still_isolates_as_otto_app(pool)`:

```rust
    // The release before #28 builds its Db with otto-tenant's default role. During the rollout,
    // and after a rollback, it runs against a schema migration 0007 has changed: otto_app keeps
    // every grant, so isolation still holds as otto_app.
    #[allow(clippy::disallowed_methods)] // deliberately the previous release's construction
    let previous = otto_tenant::Db::from_pool(pool.clone());
    let report = previous.verify_tenant_isolation().await.expect("isolation as otto_app");
    assert_eq!(report.effective_role, "otto_app");
    assert!(report.tenant_role_assumed);
```

- [ ] **Step 2: Run them to see them fail**

`DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test -p cxp-core --test db --locked`
→ the runtime-grant and `holds_shared_tenant_role` tests fail (compile error for the latter);
the parity, tombstone, and previous-release tests should already pass on Task 1's migration (they
pin behavior; note it in the report).

- [ ] **Step 3: Implement** in `crates/cxp-core/src/db_role.rs`:

```rust
/// What the runtime role needs besides its own login:
///
/// - `INSERT on deleted_orgs`: what it does as itself and a tenant transaction may not, on a
///   table it does not own (#27): the `org.deleted` tombstone. Migration 0005 grants it to
///   `cxp_runtime`.
/// - `inheriting cxp_app` ([`crate::TENANT_ROLE`]): the maintenance sweep, the usage shipper, and
///   the console lookup run as the connecting role and use the table and function grants it
///   inherits from the tenant role (#28); a `NOINHERIT` login, or a grant `WITH INHERIT FALSE`,
///   holds none of them, and otto-tenant cannot `SET LOCAL ROLE` to a role it does not hold.
pub const RUNTIME_GRANTS: [&str; 2] = ["INSERT on deleted_orgs", "inheriting cxp_app"];

/// The tenant role the otto services on a shared instance may share. otto-cxp no longer uses it
/// (#28), but the release before it does, so the runtime login stays a member until that
/// release cannot come back (docs/deploy/fly.md, runbook T5).
pub const SHARED_TENANT_ROLE: &str = "otto_app";
```

`missing_runtime_grants`: bind `crate::TENANT_ROLE` as `$1` in place of the `'otto_app'`
literal, and make a missing role count as missing:

```rust
        "SELECT coalesce(NOT pg_catalog.has_table_privilege(current_user, \
                pg_catalog.to_regclass('public.deleted_orgs'), 'INSERT'), false), \
                coalesce(NOT pg_catalog.pg_has_role(current_user, \
                (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = $1), 'USAGE'), \
                true)",
```

and update its doc ("`inheriting cxp_app` when `current_user` does not hold the tenant role's
privileges, or that role does not exist"). Add a unit test in `db_role.rs`'s `mod tests` pinning
`RUNTIME_GRANTS[1] == format!("inheriting {}", crate::TENANT_ROLE)`.

```rust
/// Whether the session's login or current user is still a member of [`SHARED_TENANT_ROLE`]
/// (false where it does not exist). Boot warns in a deployment while it is: nothing in this
/// release uses it, and a grant another service makes to it would reach this login.
pub async fn holds_shared_tenant_role(db: &Db) -> Result<bool> {
    let member: bool = sqlx::query_scalar(
        "SELECT coalesce((SELECT pg_catalog.pg_has_role(session_user, r.oid, 'MEMBER') \
                              OR pg_catalog.pg_has_role(current_user, r.oid, 'MEMBER') \
                          FROM pg_catalog.pg_roles r WHERE r.rolname = $1), false)",
    )
    .bind(SHARED_TENANT_ROLE)
    .fetch_one(db.pool())
    .await?;
    Ok(member)
}
```

Module docs line 5: "Tenant transactions drop to `cxp_app` ([`crate::TENANT_ROLE`])". In
`OTHER_SERVICE_DATABASES`'s doc, "`otto_app`, which this service's role belongs to, holds table
privileges there" → "`otto_app`, which this service's login may still belong to (until runbook
T5), holds table privileges there". `platform_events.rs:273`: "otto_app may not DELETE
tombstones" → "the tenant role may not DELETE tombstones".

- [ ] **Step 4: Run to see them pass**

`DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test -p cxp-core --locked` → all
pass, 0 skipped; `cargo test -p cxp-core --lib --locked` includes the new unit test. Note: the
server crate's `boot.rs` unit test that expects `"inheriting otto_app"` will now fail; that is
Task 3's. Run `cargo clippy --workspace --all-targets --locked -- -D warnings` and
`cargo fmt --all -- --check`.

- [ ] **Step 5: Commit**

```bash
git add crates/cxp-core
git commit -m "feat(core): require the runtime role to inherit cxp_app" -m "The background tasks use the grants the runtime login inherits from the tenant role, now cxp_app. holds_shared_tenant_role lets boot say when the login can leave otto_app. New tests prove cxp_app holds exactly otto_app's privileges, that a tenant transaction cannot write a tombstone, and that the previous release still isolates as otto_app.

Refs #28"
```

(If `boot.rs`'s unit test fails at this commit, include its one-line expected-text change here so
the commit builds and tests green: `"inheriting otto_app"` → `"inheriting cxp_app"` and the fix
text `GRANT otto_app TO` → `GRANT cxp_app TO`, plus the matching `missing_grants` format string in
`boot.rs`; Task 3 does the rest.)

---

### Task 3: `cxp-server` boot refuses an unassumed tenant role, warns about `otto_app`; e2e

**Files:**
- Modify: `crates/cxp-server/src/boot.rs` (`run_with` ~518-527, `missing_grants` ~409-428, docs,
  `mod tests`)
- Modify: `crates/cxp-server/src/lib.rs` (docs naming `otto_app` as the tenant role, if any)
- Test: `crates/cxp-server/src/boot.rs` (unit), `crates/cxp-server/tests/e2e.rs`

**Interfaces:** consumes `TENANT_ROLE`, `db_role::{RUNTIME_GRANTS, holds_shared_tenant_role,
SHARED_TENANT_ROLE}`. Produces `pub fn check_tenant_role(report: &otto_tenant::IsolationReport,
login: &str, deployment: bool) -> anyhow::Result<()>`.

**Out-of-band / invariants:** tenant isolation proven at boot; no database text in a response
(these are logs and boot errors only).

**Model:** sonnet — specified logic; e2e follows existing patterns closely.

- [ ] **Step 1: Write the failing unit tests** in `boot.rs` `mod tests`. Build reports with:

```rust
    fn report(effective: &str, assumed: bool) -> otto_tenant::IsolationReport {
        otto_tenant::IsolationReport {
            tenant_role: cxp_core::TENANT_ROLE.into(),
            effective_role: effective.into(),
            tenant_role_assumed: assumed,
            role_is_superuser: false,
            role_bypasses_rls: false,
            tables: vec![],
        }
    }

    #[test]
    fn a_deployment_refuses_tenant_transactions_that_did_not_assume_cxp_app() {
        assert!(check_tenant_role(&report("cxp_app", true), "cxp_server", true).is_ok());
        assert!(check_tenant_role(&report("cxp_app", true), "cxp_server", false).is_ok());
        let err = check_tenant_role(&report("cxp_server", false), "cxp_server", true)
            .unwrap_err()
            .to_string();
        assert!(err.contains(r#"tenant transactions run as "cxp_server""#), "{err}");
        assert!(err.contains(r#"GRANT cxp_app TO "cxp_server""#), "{err}");
        // Assumed, but landing somewhere else, is refused too.
        assert!(check_tenant_role(&report("otto_app", true), "cxp_server", true).is_err());
        // A developer's machine is only warned.
        assert!(check_tenant_role(&report("cxp_server", false), "cxp_server", false).is_ok());
    }
```

(If `IsolationReport` is not re-exported at `otto_tenant::IsolationReport`, use
`otto_tenant::isolation::IsolationReport`.) Update the existing `missing_grants` unit tests
(~1015-1035) to expect `inheriting cxp_app` and `GRANT cxp_app TO "…" WITH INHERIT TRUE`.

- [ ] **Step 2: Run to see them fail**

`cargo test -p cxp-server --lib --locked check_tenant_role` → compile error.

- [ ] **Step 3: Implement** in `boot.rs`:

```rust
/// Whether tenant transactions really run as otto-cxp's own tenant role. otto-tenant issues
/// `SET LOCAL ROLE cxp_app` only when the role exists and the login holds it, and otherwise
/// runs tenant transactions as the login itself, which `verify_tenant_isolation` accepts for any
/// role that is neither superuser nor BYPASSRLS. That login also holds the background grants
/// (the tombstone), and would serve every request with them, silently. So a deployment refuses
/// unless the report says the role was assumed and is the effective role; a developer's machine
/// is warned.
pub fn check_tenant_role(
    report: &otto_tenant::IsolationReport,
    login: &str,
    deployment: bool,
) -> Result<()> {
    if report.tenant_role_assumed && report.effective_role == cxp_core::TENANT_ROLE {
        return Ok(());
    }
    let role = cxp_core::TENANT_ROLE;
    let why = format!(
        "tenant transactions run as {:?}, not otto-cxp's own tenant role {role} (it does not \
         exist, or {} cannot assume it): as a superuser, CREATE ROLE {role} NOLOGIN; GRANT \
         {role} TO {}; then migrate (docs/deploy/fly.md, runbook T1)",
        report.effective_role,
        sql_identifier(login),
        sql_identifier(login),
    );
    if deployment {
        tracing::error!("{why}: refusing to serve");
        return Err(anyhow!("refusing to serve: {why}"));
    }
    tracing::warn!("{why}: allowed only because this is not a deployment");
    Ok(())
}
```

In `run_with`, replace the isolation block with:

```rust
    let report = db
        .verify_tenant_isolation()
        .await
        .context("refusing to serve: tenant isolation is not enforced by this database")?;
    let isolation = report.summary();
    tracing::info!("{isolation}");
    check_tenant_role(&report, &role.current_user, deployment)?;
```

After the runtime-grant block (before the isolation proof), in a deployment only:

```rust
    if deployment {
        match cxp_core::db_role::holds_shared_tenant_role(&db).await {
            Ok(true) => tracing::warn!(
                "database role {} is still a member of {}, the tenant role the otto services \
                 share; this release no longer uses it: REVOKE {} FROM {} once it is verified \
                 (docs/deploy/fly.md, runbook T5)",
                sql_identifier(&role.current_user),
                cxp_core::db_role::SHARED_TENANT_ROLE,
                cxp_core::db_role::SHARED_TENANT_ROLE,
                sql_identifier(&role.current_user),
            ),
            Ok(false) => {}
            Err(e) => tracing::warn!(error = %e.log_detail(),
                "could not read whether the database role still belongs to otto_app"),
        }
    }
```

`missing_grants`: rename the destructured `otto_app` to `tenant`, and push
`format!("GRANT {} TO {role} WITH INHERIT TRUE", cxp_core::TENANT_ROLE)`. Fix doc comments in
`boot.rs` that say tenant transactions use `otto_app`.

- [ ] **Step 4: Write the failing e2e tests** in `crates/cxp-server/tests/e2e.rs`, following the
patterns of `a_deployment_refuses_an_owner_as_its_runtime_role` and the runtime-role tests around
it (deployment config, `split_with`, captured logs, "nothing listens"):
  - `a_deployment_refuses_a_runtime_role_that_cannot_assume_cxp_app`: `split_with(&pool,
    &["otto_app", "cxp_runtime"])` (the shape before runbook T1), owner migrates via
    `boot::migrate`, then `run_with` as the runtime login with `run_migrations = false` in a
    deployment config: `Err` whose text contains `cxp_app` and `GRANT cxp_app TO`; nothing listens.
  - `the_rollout_shape_serves_and_warns_about_otto_app`: `split_with(&pool, &["otto_app",
    "cxp_app", "cxp_runtime"])` (between the deploy and runbook T5): serves (`/readyz` ready),
    the captured log contains `is still a member of otto_app` and
    `tenant isolation enforced as role "cxp_app"`.
  - In `the_service_runs_whole_as_a_runtime_role_that_owns_nothing`: assert, as the superuser
    pool, `SELECT pg_has_role($1, 'otto_app', 'MEMBER')` is false for the runtime login, and the
    log has no `is still a member of otto_app` line.

- [ ] **Step 5: Run to see them pass**

`DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test -p cxp-server --locked` →
all pass (lib, e2e, secret_env), 0 skipped. Then the whole workspace test gate, clippy, fmt.

- [ ] **Step 6: Commit**

```bash
git add crates/cxp-server
git commit -m "feat(server): refuse to serve unless tenant transactions assume cxp_app" -m "otto-tenant runs tenant transactions as the login itself when the tenant role is missing or not assumable, and the isolation proof accepts that for a non-superuser login, which here also holds the background grants. A deployment now refuses unless SET LOCAL ROLE cxp_app was issued, and warns while the login still belongs to the shared otto_app.

Refs #28"
```

---

### Task 4: CI, docs, and the production runbook

**Files:**
- Modify: `.github/workflows/ci.yml` (step "Pre-create the otto_app role", ~60-64)
- Modify: `.env.example` (~20), `fly.toml` (comment ~16), `README.md` (~27)
- Modify: `CLAUDE.md` (Tenancy section; Migrations section "grants to `otto_app`"; Invariants
  "Append-only migrations" SECURITY DEFINER clause and "Tenant isolation is RLS, proven"; Worktree
  bootstrap "Role" step; Out-of-band "Migrations (agent)"; Ship `verify:` log line; security focus
  "RLS under `otto_app`")
- Modify: `docs/deploy/fly.md` ("Database role" table and text, the `otto_app` rule paragraph,
  new runbook after the split runbook's S7, "First deploy" step 2 note, "Verify" log line)

**Interfaces:** documents the log lines Task 3 produces, verbatim.

**Out-of-band / invariants:** this task writes the human-only runbook; it runs none of it.

**Model:** sonnet — docs with exact text given; the runbook SQL is specified below.

- [ ] **Step 1: CI.** Replace the step with:

```yaml
      # 0001 creates otto_app and 0007 creates cxp_app, each tolerating a lost race, but many
      # test databases migrate in parallel against these cluster-scoped roles; creating them
      # first takes the race out.
      - name: Pre-create the tenant roles
        run: |
          psql "$DATABASE_URL" -c "CREATE ROLE otto_app NOLOGIN;"
          psql "$DATABASE_URL" -c "CREATE ROLE cxp_app NOLOGIN;"
```

and the services comment: "so the migrations can create and grant otto_app and cxp_app".

- [ ] **Step 2: Small docs.**
  - `.env.example` line 20: `GRANT cxp_app, cxp_runtime TO cxp_dev;` and add before it
    `--   CREATE ROLE cxp_app NOLOGIN;       -- once per cluster (migration 0007 creates it only as a superuser)`.
  - `fly.toml`: "Isolation rests on `SET LOCAL ROLE cxp_app` (otto-cxp's own tenant role, #28)
    under row-level security; cxp-server proves that at boot and refuses to serve otherwise."
  - `README.md`: "under otto-cxp's own `cxp_app` role".
  - `CLAUDE.md`: every place listed above. Tenancy: add one sentence: "Tenant transactions assume
    `cxp_core::TENANT_ROLE` (`cxp_app`, #28); build every `Db` with `cxp_core::tenant_db` (clippy
    forbids `Db::from_pool`/`Db::connect`)." Migrations: "grants to `cxp_app`". Invariant (tenant
    isolation): "Tenant transactions run as `cxp_app` (`SET LOCAL ROLE`), otto-cxp's own role; a
    deployment refuses to serve unless it was assumed (`boot::check_tenant_role`)", "requires
    `INSERT` on `deleted_orgs` (through `cxp_runtime`) and inheriting `cxp_app`", and keep the
    `otto_app` rule sentence ("No service may grant `CONNECT`, or any database-level privilege, to
    the shared `otto_app`; otto-cxp's login leaves it in runbook T5"). Append-only invariant:
    "before being granted to the tenant role `cxp_app`". Bootstrap Role step: check and create
    both `otto_app` and `cxp_app`. Out-of-band Migrations: "beyond the tenant role `cxp_app`".
    `verify:`: `tenant isolation enforced as role "cxp_app"`. Security focus: "RLS under
    `cxp_app`".

- [ ] **Step 3: `docs/deploy/fly.md`.**
  - "Database role": four roles now. Table rows: `cxp_server` "member of `cxp_app` and
    `cxp_runtime` (and of `otto_app` until T5)"; new row `cxp_app` | no | nothing | `otto_cxp`'s
    tenant grants, the same as `otto_app`'s there (migration 0007) | `SET LOCAL ROLE` in tenant
    transactions; inherited by `cxp_server` for the background work. The text after the table:
    `cxp_server` is a member of `cxp_app` so tenant transactions can `SET LOCAL ROLE cxp_app`.
    Explain why `cxp_app` and not `cxp_runtime` (two sentences from the spec).
  - The `otto_app` paragraph: keep the rule in bold, say otto-cxp no longer uses `otto_app` since
    #28 (tenant transactions assume `cxp_app`; boot refuses otherwise), and that `cxp_server`
    leaves it in T5; the rule stays for every service while `otto_app` exists.
  - Boot refusals list: add "a deployment refuses to serve unless tenant transactions assumed
    `cxp_app` (`tenant transactions run as "…", not otto-cxp's own tenant role cxp_app …`), and
    warns while the login is still a member of `otto_app`."
  - New section after S7: `### Runbook: give otto-cxp its own tenant role (human-only, Rob)` with
    the text below (adapt only line wrapping):

````markdown
[#28](https://github.com/savvagent/otto-cxp/issues/28). It starts from the state the split
runbook (S0 to S7) and #27's deploy left: `cxp_owner` owns `otto_cxp`, `cxp_server` owns nothing
and is a member of `otto_app` and `cxp_runtime`, and migrations 1 to 6 are applied. It ends with
tenant transactions running as `cxp_app` and `cxp_server` no longer a member of `otto_app`. Every
step is a production role change on the shared `otto-db`; the agent that wrote this ran the SQL
only against a local container put in the same state. **T0 to T2 must be done before the PR for
#28 merges**: its release command fails on migration 0007 if `cxp_app` does not exist (`cxp_owner`
cannot create roles), and its machines refuse to serve unless `cxp_server` can assume it. T3 to T5
follow the deploy. Nothing before T5 changes what the running release does.

**T0. Look first (read-only).** In `fly postgres connect -a otto-db`:

```sql
\set ON_ERROR_STOP on
-- cxp_app does not exist yet (0 rows); cxp_server, cxp_owner, cxp_runtime, otto_app do.
SELECT rolname, rolcanlogin, rolsuper, rolbypassrls, rolcreaterole, rolcreatedb, rolreplication
FROM pg_roles WHERE rolname IN ('cxp_app', 'cxp_server', 'cxp_owner', 'cxp_runtime', 'otto_app')
ORDER BY 1;
-- cxp_owner owns otto_cxp; PUBLIC has no CONNECT (no "=c" entry) on any service database.
SELECT datname, pg_get_userbyid(datdba) AS owner, datacl FROM pg_database
WHERE datname IN ('otto_cxp', 'otto_platform', 'otto_factory', 'otto_flags') ORDER BY 1;
-- What cxp_server is a member of: otto_app and cxp_runtime.
SELECT r.rolname FROM pg_auth_members m JOIN pg_roles r ON r.oid = m.roleid
WHERE m.member = 'cxp_server'::regrole ORDER BY 1;
\c otto_cxp
-- 6: #27 is deployed.
SELECT max(version) FROM _sqlx_migrations;
```

**T1. Create the role.** As postgres:

```sql
\set ON_ERROR_STOP on
-- NOLOGIN: only ever a SET LOCAL ROLE target. It holds nothing until migration 0007 grants it
-- otto_cxp's tenant grants, and is a member of nothing.
CREATE ROLE cxp_app NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION;
-- INHERIT and SET (the defaults): otto-tenant needs SET for SET LOCAL ROLE cxp_app, and the
-- background tasks, which run as cxp_server itself, use the grants it inherits.
GRANT cxp_app TO cxp_server;
```

The running release does not use `cxp_app`, and its hourly role re-check finds nothing in a role
that owns and holds nothing: no restart.

**T2. Prove it (read-only).**

```sql
-- f for every attribute; a member of nothing (0 rows from the second query).
SELECT rolcanlogin, rolsuper, rolbypassrls, rolcreaterole, rolcreatedb, rolreplication
FROM pg_roles WHERE rolname = 'cxp_app';
SELECT roleid::regrole FROM pg_auth_members WHERE member = 'cxp_app'::regrole;
-- t, t: cxp_server holds it (MEMBER) and inherits its grants (USAGE).
SELECT pg_has_role('cxp_server', 'cxp_app', 'MEMBER') AS member,
       pg_has_role('cxp_server', 'cxp_app', 'USAGE') AS inherits;
\c otto_cxp
-- 0 rows: it owns nothing in otto_cxp and can create nothing there.
SELECT c.relname FROM pg_class c WHERE c.relowner = 'cxp_app'::regrole
UNION ALL SELECT 'database CREATE/TEMPORARY'
WHERE has_database_privilege('cxp_app', 'otto_cxp', 'CREATE')
   OR has_database_privilege('cxp_app', 'otto_cxp', 'TEMPORARY')
UNION ALL SELECT 'schema public CREATE' WHERE has_schema_privilege('cxp_app', 'public', 'CREATE');
```

And as `cxp_server` itself (its password is in your password manager since R1), through `fly
proxy 15499:5432 -a otto-db` in a second terminal: `psql
"postgres://cxp_server@localhost:15499/otto_cxp"`, then

```sql
BEGIN; SET LOCAL ROLE cxp_app; SELECT current_user; ROLLBACK;   -- cxp_app
```

**Then merge the PR for #28** (with `FLY_DEPLOY` enabled the merge deploys; otherwise `fly deploy
-a otto-cxp --remote-only`). The release command applies 0007 as `cxp_owner`. If T1 was skipped
it fails with `role cxp_app does not exist and cxp_owner may not create it`, nothing is recorded,
and the old release keeps serving: run T1, then deploy again.

**T3. Verify the deploy.** See [Verify](#verify). The release command's log shows `applying
migrations`, `migrations applied`; each machine's shows `tenant isolation enforced as role
"cxp_app" (assumed via SET LOCAL ROLE cxp_app)`, and, until T5, `database role "cxp_server" is
still a member of otto_app, the tenant role the otto services share; this release no longer uses
it: REVOKE otto_app FROM "cxp_server" once it is verified`.

**T4. Prove the grants (read-only).** As postgres in `otto_cxp`:

```sql
-- 7.
SELECT max(version) FROM _sqlx_migrations;
-- 0 rows: cxp_app holds exactly what otto_app holds on every table, sequence, and function.
SELECT c.relname, p.priv FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
CROSS JOIN unnest(ARRAY['SELECT','INSERT','UPDATE','DELETE','TRUNCATE','REFERENCES','TRIGGER']) AS p(priv)
WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
  AND has_table_privilege('cxp_app', c.oid, p.priv) <> has_table_privilege('otto_app', c.oid, p.priv)
UNION ALL
SELECT c.relname, p.priv FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
CROSS JOIN unnest(ARRAY['USAGE','SELECT','UPDATE']) AS p(priv)
WHERE n.nspname = 'public' AND c.relkind = 'S'
  AND has_sequence_privilege('cxp_app', c.oid, p.priv) <> has_sequence_privilege('otto_app', c.oid, p.priv)
UNION ALL
SELECT f.oid::regprocedure::text, 'EXECUTE' FROM pg_proc f JOIN pg_namespace n ON n.oid = f.pronamespace
WHERE n.nspname = 'public'
  AND has_function_privilege('cxp_app', f.oid, 'EXECUTE') <> has_function_privilege('otto_app', f.oid, 'EXECUTE');
-- f, t: a tenant transaction may not write a tombstone; cxp_server may (through cxp_runtime).
SELECT has_table_privilege('cxp_app', 'public.deleted_orgs', 'INSERT') AS tenant_tombstone,
       has_table_privilege('cxp_server', 'public.deleted_orgs', 'INSERT') AS runtime_tombstone;
```

If the parity query returns rows (production's `otto_app` grants can differ from a fresh
migration's), stop here: do not run T5, keep `cxp_server` in `otto_app`, and report the rows. The
release still serves, as `cxp_app`.

**T5. Leave `otto_app`.** Once T3 and T4 pass, as postgres:

```sql
REVOKE otto_app FROM cxp_server;
-- f
SELECT pg_has_role('cxp_server', 'otto_app', 'MEMBER');
```

```bash
fly apps restart otto-cxp
curl -s https://otto-cxp.savvagent.com/readyz      # ready
fly logs -a otto-cxp --no-tail                     # tenant isolation enforced as role "cxp_app"; no "still a member of otto_app"
```

From here, a grant any service makes to `otto_app` reaches nothing of otto-cxp's: `cxp_server`
is not a member, and `otto_app`'s own grants in `otto_cxp` are usable only by a role that can
`CONNECT` to `otto_cxp`, which `PUBLIC` cannot. **Rolling back** to a release before #28 after
T5 needs the membership back first, because #27's release refuses a runtime role that does not
inherit `otto_app`: `GRANT otto_app TO cxp_server;`, then roll back. Before T5, a rollback needs
nothing (`cxp_app` stays, unused).

**Later.** A migration revoking `otto_app`'s grants in `otto_cxp`, once no release that uses it
can come back. `otto_app` itself is shared; otto-cxp never drops it.
````

  - "First deploy" step 2 comment: after "then the split runbook's S1 to S4 and S6's staged
    secret", add "and this runbook's T1 (`cxp_app`, before the first release command runs 0007)".
  - "Verify": `tenant isolation enforced as role "cxp_app" (assumed via SET LOCAL ROLE cxp_app)`.

- [ ] **Step 4: Check** `grep -rn 'otto_app' CLAUDE.md docs/deploy/fly.md README.md .env.example fly.toml .github`
and confirm every remaining mention is about the shared role itself (the rule, T5, the previous
release, R-runbook history), not the tenant role otto-cxp uses. Run `cargo fmt --all -- --check`
(no Rust change expected).

- [ ] **Step 5: Commit**

```bash
git add .github CLAUDE.md README.md .env.example fly.toml docs/deploy/fly.md
git commit -m "docs(deploy): runbook for otto-cxp's own tenant role" -m "Rob creates cxp_app and grants it to cxp_server before the release that runs migration 0007, proves it read-only, and revokes otto_app from cxp_server once the release is verified. CI pre-creates cxp_app beside otto_app.

Refs #28"
```

---

### Task 5 (orchestrator, not an implementer): proofs beyond the gates

- Mutation check: add a direct `otto_tenant::Db::from_pool` to one test; `cargo clippy` fails with
  `disallowed_methods`; revert.
- Mutation check: make `check_tenant_role` always `Ok`; `a_deployment_refuses_a_runtime_role_that_cannot_assume_cxp_app`
  still fails via the runtime-grant check (expected: two independent refusals), and the unit test
  fails; revert.
- 0007's refusal: a second scratch container without `cxp_app`; the built image's `migrate` as a
  non-CREATEROLE owner exits non-zero with `role cxp_app does not exist`; `_sqlx_migrations` max
  stays 6.
- Runbook rehearsal: on a scratch database in #27's end state (owner `cxp_owner`, runtime member of
  `otto_app` and `cxp_runtime`, migrated to 6 by the parent image), run T0 to T2, migrate with this
  branch's image as the owner, serve as the runtime (deployment config) and see the log lines,
  run T4 (0 rows), T5, restart, and see no warning.
