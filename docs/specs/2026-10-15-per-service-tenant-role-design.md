# Give otto-cxp its own tenant role instead of the shared `otto_app` — design

**Status:** APPROVED
**Issue:** #28
**Plan:** `docs/plans/2026-10-15-per-service-tenant-role.md`
**Stacked on:** PR #46 (#27, the migration-owner split); retargeted to `main` after it merges.

## Brief

> `otto_app` is one cluster role shared by every otto service on `otto-db` (otto-platform,
> otto-factory, otto-flags, otto-cxp). After #16, the only thing stopping `cxp_server` from using
> whatever `otto_app` holds in another service's database is that `CONNECT` is revoked from
> `PUBLIC`. If any service ever grants `CONNECT`, or another database-level privilege, to
> `otto_app`, every member service can reach that database. Found by the architect review of #16
> (PR #26).

Acceptance criteria (verbatim from #28):

1. otto-cxp's tenant transactions assume a service-specific role (e.g. `cxp_app`), not the shared
   `otto_app`. This requires the matching change in `otto-tenant` (shared otto-platform crate) or a
   per-service role name passed to it.
2. Until that lands, `docs/deploy/fly.md` states the rule: no service grants `CONNECT` or any
   database-level privilege to `otto_app`.
3. The boot isolation proof and the tenant-isolation test cover the new role name.

Risks named on the ticket: cross-repo (the role name may live in `otto-tenant`, so the pin moves);
production role creation and grants are human-only (Rob).

Rob's comment on #28 (from the #16 R0 inspection): otto-factory has its own NOLOGIN `of_app`, so
`otto_app` is shared by otto-cxp and possibly otto-flags or otto-platform; every other service
still connects as a superuser; since #16's R3, `PUBLIC` has no `CONNECT` on any service database,
and `cxp_server` can open only `otto_cxp`.

## Premise corrections

- **No `otto-tenant` change and no pin move.** `otto-tenant` at the pinned rev (`fa1ae08`) already
  takes a per-service role: `Db::with_tenant_role(&'static str)` (`otto-tenant/src/db.rs:110`,
  validated against `^[a-z_][a-z0-9_]*$`; the default stays `DEFAULT_TENANT_ROLE = "otto_app"`).
  `IsolationReport` already carries `tenant_role` and `tenant_role_assumed`, and its summary names
  the effective role. So AC 1 is met inside this repo, and AC 2's "until that lands" clause becomes
  "keep the rule while `otto_app` still exists"; the rule stays in `fly.md`.
- **`with_tenant_role` fails open on a missing role, so boot must close it.** When the configured
  role does not exist or the connecting role cannot assume it (`pg_has_role(current_user, role,
  'USAGE')` false), `Db::begin` silently skips `SET LOCAL ROLE`, and `verify_tenant_isolation`
  passes for any connecting role that is neither superuser nor BYPASSRLS (`cxp_server` is
  neither). A deployment whose `cxp_app` was never created would serve tenant transactions as
  `cxp_server` itself, which also holds the background grants (the tombstone), and no check would
  say so. Boot therefore refuses, everywhere (not only in a deployment), unless the isolation
  report says `tenant_role_assumed` and the effective role is `cxp_app`.
- **Every `Db` must be built the same way.** `Db::from_pool` and `Db::connect` default to
  `otto_app` and are called in 27 places (production: `boot.rs` twice; tests in every crate). One
  missed call would run that path's tenant transactions as the shared role, and tests could
  silently pass on a role production does not use. One helper builds every `Db`, and clippy's
  `disallowed-methods` forbids the two constructors everywhere else, tests included.
- **The runtime login keeps a role it inherits table grants from.** The maintenance sweep, the
  usage shipper, `revoked`, and the console lookup run as the connecting role and today use the
  table and function grants `cxp_server` inherits from `otto_app` (#27's `RUNTIME_GRANTS`:
  "inheriting otto_app"). After this change they come from `cxp_app` instead, so `cxp_server` is a
  member of `cxp_app` with `INHERIT` (and `SET`, for `SET LOCAL ROLE`), exactly as it is of
  `otto_app` today.
- **`cxp_server` cannot leave `otto_app` before this release is live.** The running #27 release
  requires inheriting `otto_app` (it refuses to boot otherwise, and its tenant transactions `SET
  LOCAL ROLE otto_app`). So the revoke is a post-deploy step, and a rollback past this release
  needs the membership back first.

## Assumptions

- Run Profile: every value comes from CLAUDE.md's "## Development Workflow"; none was assumed.
  Status labels: none exist. Deploy: `ci-auto`, but this PR stops at "open, reviewed, CI-green"
  (stacked, and Rob runs the runbook first). Gates use this worktree's own Postgres on `:15467`,
  never the shared `:15435`.
- PR #46's runbook (S0 to S6) has run and #27 is deployed before this PR merges: `cxp_owner` owns
  `otto_cxp`, `cxp_server` owns nothing and is a member of `otto_app` and `cxp_runtime`, and
  migrations 0005 and 0006 are applied. This runbook's first step checks that read-only.
- The name `cxp_app`, as the ticket suggests: `cxp_` like `cxp_server`/`cxp_owner`/`cxp_runtime`,
  `_app` like `of_app`. Roles are cluster-wide; T0 checks nobody else uses the name.
- `cxp_app` holds exactly what `otto_app` holds in `otto_cxp` (parity), including `EXECUTE` on the
  two `SECURITY DEFINER` functions, as the brief for this work asks. Narrowing what a tenant
  transaction may execute is not in scope (see Risks).
- `otto_app`'s grants in `otto_cxp` stay: the previous release runs tenant transactions as
  `otto_app` during the rollout and after a rollback. Revoking them is a later migration, once no
  release that uses `otto_app` can come back.

## Goal & success criteria

otto-cxp's tenant transactions run as `cxp_app`, a NOLOGIN role only otto-cxp uses, holding only
`otto_cxp`'s tenant grants; a deployment refuses to serve unless that role is actually assumed;
and the runtime login can then leave `otto_app`, so no grant another service makes to `otto_app`
reaches otto-cxp.

- `tenant isolation enforced as role "cxp_app" (assumed via SET LOCAL ROLE cxp_app); …` in every
  boot log and in the boot isolation test; any boot (deployment or not) whose `cxp_app` is missing, or not
  assumable by the runtime login, refuses to serve and names the fix.
- Every `Db` in the workspace is built by `cxp_core::tenant_db`; `cargo clippy` fails on a direct
  `otto_tenant::Db::from_pool` / `Db::connect` anywhere else.
- Migration 0007 gives `cxp_app` the same effective privileges as `otto_app` on every table,
  sequence, function, and schema in `otto_cxp`; a test compares the two role by role.
- The whole service runs end to end (e2e) as a runtime login that is a member of `cxp_app` and
  `cxp_runtime` and not of `otto_app`.
- `docs/deploy/fly.md` holds the runbook (T0 to T5) Rob runs after #27's, with read-only proofs.

## Scope

**In:** `cxp_core::TENANT_ROLE` and `cxp_core::tenant_db`; `clippy.toml` (`disallowed-methods`);
migration `0007_cxp_app_tenant_role.sql`; `db_role::RUNTIME_GRANTS` ("inheriting cxp_app") and
`missing_runtime_grants`; `db_role::holds_shared_tenant_role` (boot warns while the runtime login
still belongs to `otto_app`); `boot::check_tenant_role` (refuse unless assumed, everywhere);
every `Db` construction moved to the helper; the tests (core, api, mcp, server, e2e) moved from
`otto_app` to `TENANT_ROLE`, plus new tests; CI's role setup; `.env.example`, `fly.toml` comment,
README, CLAUDE.md; `docs/deploy/fly.md` (Database role, the rule, the runbook, Verify, First
deploy).

**Out:** any `otto-tenant` change or pin move; revoking `otto_app`'s grants in `otto_cxp` (a later
migration); dropping `otto_app` (shared, not ours); narrowing `cxp_app` below `otto_app`'s grants
(for example moving `EXECUTE` on the `SECURITY DEFINER` functions to `cxp_runtime`); other
services' roles; `scripts/bench-search` (dev-only scripts that `SET ROLE otto_app`, which keeps its
grants and so keeps working).

## Architecture

### Roles after this change

| Role | Login | Owns | Holds | Used by |
|---|---|---|---|---|
| `cxp_owner` | yes | `otto_cxp` and everything in it | `SET` on `app.console_session_lookup` | `cxp-server migrate` (release command), unchanged |
| `cxp_server` | yes | nothing | member of `cxp_app` and `cxp_runtime` (and `otto_app` until T5); `CONNECT` on `otto_cxp` | the serving process |
| `cxp_app` | no | nothing | `otto_cxp`'s tenant grants (migration 0007): the same as `otto_app`'s there | `SET LOCAL ROLE` in tenant transactions; inherited by `cxp_server` for the background work |
| `cxp_runtime` | no | nothing | `INSERT` on `deleted_orgs` (0005), unchanged | the runtime's grants beyond the tenant role |
| `otto_app` | no | nothing | its grants in `otto_cxp`, left in place for the previous release | nothing in this release |

### Why a new `cxp_app`, not #27's `cxp_runtime`

`cxp_runtime` exists to hold what the runtime login does **as itself** and a tenant transaction
must not: today `INSERT` on `deleted_orgs` (a tombstone written by a tenant transaction would let
one org's request permanently lock out another org; the baseline revokes it from `otto_app` for
exactly that reason), and by #27's design every future background-only privilege. Making it the
`SET LOCAL ROLE` target would hand all of that to every tenant transaction, and every grant a later
migration gives background work would silently widen what a request can do. Two roles keep the
two privilege sets apart: `cxp_app` is the request's identity and holds only the tenant grants;
`cxp_runtime` holds the background extras; the login inherits both. A tenant transaction, having
`SET LOCAL ROLE cxp_app`, holds `cxp_app`'s privileges only (not the login's, not
`cxp_runtime`'s), which `tenant_transactions_cannot_write_the_tombstone` proves.

### `cxp-core`

- `lib.rs`:
  ```rust
  /// The role every tenant transaction assumes (`SET LOCAL ROLE`): otto-cxp's own (#28), never
  /// the `otto_app` the otto services share. Migration 0007 creates it where it can and grants it
  /// otto_cxp's tenant grants.
  pub const TENANT_ROLE: &str = "cxp_app";

  /// The only way to make an `otto_tenant::Db` in this workspace (clippy forbids
  /// `Db::from_pool` / `Db::connect` elsewhere), so production and every test pin the same role.
  pub fn tenant_db(pool: sqlx::PgPool) -> otto_tenant::Db
  ```
  `tenant_db` calls `Db::from_pool(pool).with_tenant_role(TENANT_ROLE)` under
  `#[allow(clippy::disallowed_methods)]`; `with_tenant_role` can fail only on an invalid name, so
  it `expect`s, and a unit test proves `TENANT_ROLE` validates and `tenant_db(..).tenant_role() ==
  TENANT_ROLE` (on a `connect_lazy` pool, no database).
- `clippy.toml` (workspace root, new):
  ```toml
  disallowed-methods = [
    { path = "otto_tenant::db::Db::from_pool", reason = "use cxp_core::tenant_db, which pins TENANT_ROLE (#28)" },
    { path = "otto_tenant::db::Db::connect", reason = "use cxp_core::tenant_db, which pins TENANT_ROLE (#28)" },
  ]
  ```
  The one deliberate exception is a test that builds the previous release's `Db` (default
  `otto_app`) to prove rollout compatibility, with a local `#[allow]` and a comment.
- `db_role.rs`:
  - `RUNTIME_GRANTS = ["INSERT on deleted_orgs", "inheriting cxp_app"]` (the second built from
    `TENANT_ROLE` in the docs; the string literal stays a `&str` constant and a test pins it to
    `TENANT_ROLE`).
  - `missing_runtime_grants` checks `pg_has_role(current_user, <TENANT_ROLE oid>, 'USAGE')`, the
    role name bound as `$1`. A missing role now counts as missing (today a missing `otto_app`
    counts as present): `cxp_app` is required, and on a developer's machine the result only warns.
  - New `pub async fn holds_shared_tenant_role(db) -> Result<bool>`: whether `session_user` or
    `current_user` is a member (`'MEMBER'`) of `otto_app` (false when that role does not exist).
    `SHARED_TENANT_ROLE = "otto_app"` documents the name.
  - Module docs: tenant transactions drop to `cxp_app`.
- `platform_events.rs:273` comment: "the tenant role may not DELETE tombstones".

### Migration `0007_cxp_app_tenant_role.sql`

```sql
-- otto-cxp's own tenant role (#28). Tenant transactions SET LOCAL ROLE cxp_app
-- (cxp_core::TENANT_ROLE) instead of otto_app, which every otto service on otto-db may share:
-- a privilege any service grants otto_app would otherwise reach otto-cxp, and the reverse.
--
-- Roles are cluster-wide. Create it where this role may (tests, local superusers), tolerating a
-- concurrent creator; where it may not (production's cxp_owner has no CREATEROLE), it must
-- already exist (docs/deploy/fly.md, runbook T1), and otherwise this migration fails, so the
-- release command aborts the deploy rather than record 0007 with no grants made.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cxp_app') THEN
    BEGIN
      CREATE ROLE cxp_app NOLOGIN;
    EXCEPTION
      WHEN duplicate_object OR unique_violation THEN
        NULL; -- another database in this cluster created it first
      WHEN insufficient_privilege THEN
        RAISE EXCEPTION 'role cxp_app does not exist and % may not create it: as a superuser, '
                        'CREATE ROLE cxp_app NOLOGIN; GRANT cxp_app TO <the runtime login>; '
                        '(docs/deploy/fly.md, runbook T1), then migrate again', current_user;
    END;
  END IF;
END $$;

-- The same grants 0001, 0003 and 0004 gave otto_app, as one statement each: every table,
-- sequence and function in public, then the same three narrowings as 0001. otto_app keeps its
-- own: the previous release still runs tenant transactions as otto_app until this one replaces
-- it (and after a rollback); a later migration revokes them.
GRANT USAGE ON SCHEMA public TO cxp_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO cxp_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO cxp_app;
-- Includes the two SECURITY DEFINER functions (expire_removed_members, console_session_org),
-- which are REVOKE ALL ... FROM PUBLIC and granted to the tenant role by name.
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO cxp_app;
-- Tables and sequences a later migration creates, as the migrating role (cxp_owner in production).
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO cxp_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO cxp_app;
-- As 0001 for otto_app: tombstones are written by the runtime role (cxp_runtime) and permanent;
-- removed members expire only through expire_removed_members(); the migrator's ledger is not
-- the app's.
REVOKE INSERT, UPDATE, DELETE ON deleted_orgs FROM cxp_app;
REVOKE DELETE ON removed_members FROM cxp_app;
REVOKE ALL ON public._sqlx_migrations FROM cxp_app;
```

No `GRANT cxp_app TO CURRENT_USER`: unlike 0001 in #16's model, the migrating role is not the
serving role (#27), and the owner has no business in a tenant transaction; granting the role to a
login is a role change, made by a superuser (T1) or by a test's own setup. Additive only: a role
(where it can be created) and grants; the previous release reads and writes the same schema as
before and uses `otto_app`'s unchanged grants. No new table, so no RLS or isolation-list change;
no new `SECURITY DEFINER` function. `GRANT … ON ALL FUNCTIONS` as `cxp_owner` covers `pg_trgm`'s
functions too (owned by `cxp_owner` since S3's `REASSIGN OWNED`), as 0001's did for `otto_app`.
`_sqlx_migrations` is in `public` and owned by the migrating role, so the explicit revoke applies.

### `cxp-server`

- `boot.rs`, `lib.rs`, `health.rs`: every `Db::from_pool` becomes `cxp_core::tenant_db`.
- New `pub fn check_tenant_role(report: &IsolationReport, login: &str) -> Result<()>`, called
  right after `verify_tenant_isolation` in `run_with`:
  - assumed and `effective_role == TENANT_ROLE` → `Ok` (the `info` summary already logged);
  - otherwise → `error` log and refusal, **on every machine, not only in a deployment**:
    `refusing to serve: tenant transactions run as "<effective>", not otto-cxp's own tenant role
    cxp_app (it does not exist, or "<login>" cannot assume it): as a superuser, CREATE ROLE
    cxp_app NOLOGIN; GRANT cxp_app TO "<login>" WITH INHERIT TRUE; then migrate (docs/deploy/fly.md,
    runbook T1)` (`WITH INHERIT TRUE`, so the hint also fixes a member that does not inherit).
  `<login>` is `role.current_user`. Why no local exception (unlike the privilege checks, which
  only warn locally because a local Postgres hands out a superuser): a local superuser creates
  `cxp_app` in 0007 and can `SET ROLE` to it, so the check passes there anyway; CI and the tests
  create it; and a local non-superuser runtime that cannot assume it would be exactly the silent
  fallback this exists to stop. Refusing everywhere means no configuration ever runs tenant
  transactions as anything but `cxp_app`.
- After the runtime-grant check, in a deployment only: if `holds_shared_tenant_role` →
  `warn`: `database role "<login>" is still a member of otto_app, the tenant role the otto services
  share; this release no longer uses it: REVOKE otto_app FROM "<login>" once it is verified
  (docs/deploy/fly.md, runbook T5)`. Not a refusal: the previous release needs the membership, so
  it must outlive the rollout. A read error here is logged at `warn` and does not stop boot (it is
  advice, not a check).
- `missing_grants` names `GRANT cxp_app TO <role> WITH INHERIT TRUE` for the second grant.
- Docs in `boot.rs`/`lib.rs` that say `otto_app` for the tenant role now say `cxp_app`.

### Public-interface changes

Additive: `cxp_core::TENANT_ROLE`, `cxp_core::tenant_db`, `db_role::holds_shared_tenant_role`,
`db_role::SHARED_TENANT_ROLE`, `boot::check_tenant_role`. Changed (intended): the second
`RUNTIME_GRANTS` entry's text, the boot log's isolation line (`cxp_app`), and a deployment's
refusal of a runtime login that cannot assume `cxp_app`. No wire, route, tool, config key, scope,
or secret changes.

## Invariants impact

- **Append-only migrations:** new `0007_cxp_app_tenant_role.sql` (after #27's 0005/0006); a role
  and grants only. The previous release (tenant role `otto_app`) is unaffected: `otto_app`'s grants
  stay. `ignore_missing` lets it boot against 0007.
- **Tenant isolation is RLS, proven:** strengthened. The boot proof now also requires that the
  tenant role was actually assumed in a deployment. The policies name no role, so they bind
  `cxp_app` as they bound `otto_app`; FORCE RLS is unchanged. The tenant-isolation test asserts
  `cxp_app`. CLAUDE.md's text ("Tenant transactions run as `otto_app`", "grants to `otto_app`") is
  updated; new tenant tables grant to `cxp_app` (default privileges cover the migrating role's new
  tables; a migration still grants explicitly, as 0003/0004 did).
- **`SECURITY DEFINER` functions:** unchanged and still `REVOKE ALL … FROM PUBLIC`; 0007 grants
  `EXECUTE` to `cxp_app` (parity with `otto_app`). CLAUDE.md's "before being granted to `otto_app`"
  becomes "to the tenant role `cxp_app`".
- **Request paths use `begin_live`; unpinned access only through `maintenance`:** unchanged; the
  maintenance marker policies name no role.
- **No SQL outside `cxp-core`; runtime queries only:** the new SQL (`holds_shared_tenant_role`,
  the migration) is in `cxp-core`, runtime `sqlx::query*`. The role name in `SET LOCAL ROLE` is
  interpolated by otto-tenant after its own validation.
- **Small shared database:** one extra catalog query at boot; no new pool.
- **Composite FKs, auth, metering, teams, webhooks, untrusted text, licensing, pin:** untouched.
  The `otto-tenant`/`otto-resource` rev does not move.

## Migration

`crates/cxp-core/migrations/0007_cxp_app_tenant_role.sql`, as above. Rollout in both directions:
the old release ignores `cxp_app` and keeps using `otto_app`; the new release needs `cxp_app` to
exist before the release command runs (T1) and refuses to serve without it. If T1 was skipped, the
release command fails on 0007 (nothing recorded; sqlx runs it in one transaction), the deploy
aborts, and the old release keeps serving.

## Out-of-band

All human-only (Rob), per CLAUDE.md "Database role change". The runbook goes in
`docs/deploy/fly.md` as "Runbook: give otto-cxp its own tenant role", after #27's:

- **Before merge:**
  - **T0 (read-only).** #27's end state: `cxp_owner` owns `otto_cxp`; `_sqlx_migrations` max
    version 6; `cxp_server` owns nothing and is a member of `otto_app` and `cxp_runtime`; no role
    named `cxp_app` exists; `PUBLIC` has no `CONNECT` on any service database.
  - **T1.** As postgres: `CREATE ROLE cxp_app NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB
    NOCREATEROLE NOREPLICATION; GRANT cxp_app TO cxp_server;`. The plain `GRANT` (INHERIT from
    `cxp_server`'s own `INHERIT` attribute, the default, and SET) works on Postgres 15 and 16
    alike; Postgres 16's `WITH INHERIT TRUE, SET TRUE` is not assumed. T2 proves both. The running #27 release is
    unaffected (it does not use `cxp_app`; its hourly role re-check finds nothing on a role that
    owns and holds nothing). No restart.
  - **T2 (read-only).** `cxp_app`: NOLOGIN, no attribute boot refuses, a member of nothing, owns
    nothing, holds no privilege yet. `cxp_server`: `pg_has_role(…, 'cxp_app', 'USAGE')` and
    `'MEMBER'` true; and, with `cxp_server`'s own login through `fly proxy`, `BEGIN; SET LOCAL
    ROLE cxp_app; SELECT current_user; ROLLBACK;` answers `cxp_app`.
- **Merge** (or hand `fly deploy` while `FLY_DEPLOY` is off). The release command applies 0007 as
  `cxp_owner`.
- **After deploy:**
  - **T3 (verify).** Release command log: `applying migrations`, `migrations applied`. Each
    machine: `tenant isolation enforced as role "cxp_app" (assumed via SET LOCAL ROLE cxp_app)`,
    and the `still a member of otto_app` warning.
  - **T4 (read-only).** `cxp_app`'s privileges equal `otto_app`'s on every table, sequence,
    function, and schema (a query listing differences: 0 rows); `_sqlx_migrations` max version 7.
  - **T5.** `REVOKE otto_app FROM cxp_server;`, then `fly apps restart otto-cxp`; `/readyz` ready,
    no warning. From then on, rolling back to a release before this one needs `GRANT otto_app TO
    cxp_server;` first (#27's release refuses a runtime role that does not inherit `otto_app`).
- **Later (not this PR):** a migration revoking `otto_app`'s grants in `otto_cxp`, once no release
  that uses it can be rolled back to.

The PR body lists T0 to T5, marking T0 to T2 as before merge (CLAUDE.md "Database role
change"). No new secret, config key, or scope; no recurring cost.

## Error handling & edge cases

- **`cxp_app` missing at the release command** (T1 skipped): 0007 raises, naming the fix; the
  deploy aborts; the old release serves.
- **`cxp_app` exists but `cxp_server` is not a member** (T1's `GRANT` skipped): the new release's
  runtime-grant check (`inheriting cxp_app` missing) and `check_tenant_role` both refuse to serve;
  the new machines fail their health check; the error names `GRANT cxp_app TO "cxp_server"`.
- **Member with `INHERIT FALSE`** (Postgres 16 `WITH INHERIT FALSE`): `pg_has_role(…, 'USAGE')`
  is false, so otto-tenant skips `SET LOCAL ROLE`; the runtime-grant check and
  `check_tenant_role` refuse.
- **Member with `SET FALSE`** (Postgres 16): otto-tenant's probe (`USAGE`) says assumable, `SET
  LOCAL ROLE cxp_app` fails, and `verify_tenant_isolation` errors: boot refuses.
- **A grant to `cxp_app` after boot** (`GRANT pg_read_all_data TO cxp_app`): the hourly re-check
  inspects everything `cxp_server` is a member of, so it raises the readiness alarm as for any
  other refused privilege (#27).
- **`cxp_app` revoked from `cxp_server` after boot:** the cached assumability is stale; `SET
  LOCAL ROLE` fails and every tenant transaction errors (fails closed); the hourly runtime-grant
  re-check logs `error`. A restart refuses with the fix.
- **Local superuser:** creates `cxp_app` in 0007 and may `SET ROLE` to anything; `check_tenant_role`
  passes (assumed). A local non-superuser runtime that cannot assume `cxp_app`: refused, like a
  deployment (no local exception).
- **A local non-superuser, non-CREATEROLE owner migrating a cluster with no `cxp_app`:** 0007
  fails (where 0001 only logged a NOTICE for `otto_app`), naming the fix. `.env.example`'s local
  split recipe creates `cxp_app` first.
- **T4's parity query returns rows in production** (production's `otto_app` grants could differ
  from a fresh migration's): stop before T5, keep `cxp_server` in `otto_app`, and report the
  difference; the release still serves, as `cxp_app`.
- **Concurrent test databases creating `cxp_app`:** tolerated in 0007; CI pre-creates it anyway.

## Testing approach

Gates: `DATABASE_URL=postgres://cxp:cxp@localhost:15467/otto_cxp cargo test --workspace --locked`
(this worktree's own container, with `otto_app` and `cxp_app` pre-created as CI does), clippy,
fmt, the migrations-append-only diff, `podman build -t otto-cxp:check-issue28 .`. No pnpm path
changes. Plus a manual check: a direct `otto_tenant::Db::from_pool` in a test makes clippy fail
(mutation check, reverted), and the runbook's SQL (T0 to T5) run against a local database put in
#27's end state.

- `crates/cxp-core/src/lib.rs` unit: `TENANT_ROLE` validates; `tenant_db(..).tenant_role() ==
  TENANT_ROLE`.
- `crates/cxp-core/tests/common/mod.rs`: `db` calls `cxp_core::tenant_db`; `otto_app_db`,
  `as_app_unpinned` set `TENANT_ROLE` (renamed `tenant_role_db`; the `as_app_*` names stay).
- `crates/cxp-core/tests/db.rs`:
  - `tenant_isolation_is_enforced_on_every_tenant_table`: `report.tenant_role == TENANT_ROLE`,
    `tenant_role_assumed`, `effective_role == "cxp_app"`, summary names `cxp_app`.
  - `the_schema_keeps_its_structural_invariants`: the per-table grant matrix checked for both
    `cxp_app` and `otto_app` (otto_app's kept for the previous release).
  - new `cxp_app_holds_exactly_what_otto_app_holds`: for every relation, sequence, function in
    `public` and the schema itself, each privilege's `has_*_privilege` answer is the same for both
    roles.
  - new `tenant_transactions_cannot_write_the_tombstone`: a runtime login member of `cxp_app` and
    `cxp_runtime` (created by the test) can `INSERT INTO deleted_orgs` unpinned as itself, but not
    inside `tenant_db(..).begin(org)` on its own pool (42501): `SET LOCAL ROLE cxp_app` leaves
    `cxp_runtime`'s grant behind.
  - new `the_previous_release_still_isolates_as_otto_app`: after 0007, a default `otto_tenant::Db`
    (the previous release's) passes `verify_tenant_isolation` as `otto_app` (local `#[allow]`).
  - `missing_runtime_grants_*`: `inheriting cxp_app`; a member of `otto_app` only lacks it.
  - 0007's refusal when `cxp_app` is absent and cannot be created is not testable in the shared
    test cluster (`cxp_app` is cluster-wide, and dropping it would break concurrent tests). It is
    proved manually instead, on a second scratch container that has no `cxp_app`: the built image's
    `migrate`, as a non-CREATEROLE owner, exits non-zero naming the fix, and `_sqlx_migrations`
    stops at 6.
  - `holds_shared_tenant_role`: true for a member of `otto_app`, false for a member of `cxp_app`
    only.
  - the role-check tests that grant `otto_app` to plain roles grant `cxp_app` (and keep one
    `otto_app` case where the test is about a grant to the shared role).
- Other core test files, `cxp-api`, `cxp-mcp`: `Db::from_pool` → `cxp_core::tenant_db`; `SET ROLE
  otto_app` → `TENANT_ROLE`.
- `crates/cxp-server/tests/e2e.rs` also: `a_runtime_role_that_cannot_assume_cxp_app_is_refused_even_locally`
  (local config, where the runtime-grant check only warns, so `check_tenant_role` itself refuses).
- `crates/cxp-server/src/boot.rs` unit: `check_tenant_role` for assumed / not assumed / assumed
  but a different effective role (refused on every machine); `missing_grants` text names `cxp_app`.
- `crates/cxp-server/tests/e2e.rs`:
  - `shared_roles` creates `otto_app`, `cxp_app`, `cxp_runtime`; `RUNTIME_GROUPS = ["cxp_app",
    "cxp_runtime"]`; every boot-log assertion names `cxp_app`.
  - `the_service_runs_whole_as_a_runtime_role_that_owns_nothing` now runs with a runtime login that
    is not a member of `otto_app`, and asserts `pg_has_role(runtime, 'otto_app', 'MEMBER')` is
    false and the isolation line names `cxp_app`.
  - new `a_deployment_refuses_a_runtime_role_that_cannot_assume_cxp_app`: runtime member of
    `otto_app` and `cxp_runtime` only (the shape before T1): refused, error names `cxp_app`,
    nothing listens.
  - new `the_rollout_shape_serves_and_warns_about_otto_app`: runtime member of `otto_app`,
    `cxp_app`, `cxp_runtime` (between deploy and T5): serves, logs the `still a member of otto_app`
    warning.
- CI: `Pre-create the tenant roles` creates `otto_app` and `cxp_app`.

## Risks & open questions

- **`cxp_app` keeps `EXECUTE` on `expire_removed_members` and `console_session_org`** (parity with
  `otto_app`). A tenant transaction can call them: the first deletes expired removed-member rows
  across orgs (bounded to rows already past their TTL); the second answers an org id for a session
  hash the caller already holds. Moving both to `cxp_runtime` would narrow the tenant role further;
  a follow-up.
- **`otto_app`'s grants stay in `otto_cxp`** until a later migration. While `cxp_server` is still
  a member (until T5), a grant another service makes to `otto_app` at the cluster level still
  reaches it; T5 closes that. After T5, `otto_app` holds table grants in `otto_cxp` that only a
  member of `otto_app` could use, and any other service's login that is a member of `otto_app`
  could use them if it could `CONNECT` to `otto_cxp`, which `PUBLIC`'s revoked `CONNECT` prevents
  (the fly.md rule, kept).
- **`cxp_app` is cluster-wide.** Another service could grant it to its login; only a superuser or a
  role with `ADMIN` on `cxp_app` can, and every other service connects as a superuser today
  (#28's comment). Not preventable from inside `otto_cxp`.
- **The migration creates a cluster role where it can.** On a developer's or CI's superuser
  Postgres, 0007 leaves `cxp_app` behind across databases, as 0001 does with `otto_app`.
- **Production steps are human-only** and must run after #27's runbook and before merge; this
  agent ran them only against a local container.
