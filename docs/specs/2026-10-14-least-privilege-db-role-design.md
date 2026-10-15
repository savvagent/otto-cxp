# A dedicated non-superuser database role for otto-cxp — design

**Status:** IMPLEMENTED (#26)
**Issue:** #16
**Plan:** `docs/plans/2026-10-14-least-privilege-db-role.md`

## Brief

> otto-cxp connects to the shared `otto-db` with the role `fly postgres attach` creates, which is
> a superuser on the whole instance (also hosting `otto_platform`, `otto_factory`, `otto_flags`).
> Request transactions are isolated (`SET LOCAL ROLE otto_app` + forced RLS, proven at boot), but
> migrations, the maintenance sweep and shipper, tombstone writes and `/readyz` run as that
> superuser. Any future injection or code-execution bug in otto-cxp would reach the platform's
> database.

Acceptance criteria (verbatim from #16):

1. otto-cxp connects as a dedicated non-superuser, non-BYPASSRLS role that owns only `otto_cxp`;
   `CONNECT` on the other services' databases is revoked from `PUBLIC`.
2. Boot refuses to serve in production if the connecting role is superuser or BYPASSRLS.
3. `docs/deploy/fly.md` documents the role setup.

## Premise corrections

- **The schema already tolerates a non-superuser owner.** The baseline was written for one:
  `CREATE ROLE otto_app` and `GRANT otto_app TO CURRENT_USER` catch `insufficient_privilege`
  (`crates/cxp-core/migrations/0001_baseline.sql:62-80`, `:648-660`), `pg_trgm` is a trusted
  extension a database owner may create (`:82-85`), and `expire_removed_members()` is a
  `SECURITY DEFINER` function owned by whoever migrates (`:635-644`). No migration is needed.
- **No code path silently relied on the superuser.** The unpinned policies carry no `TO` clause,
  so they bind any role RLS applies to; every cross-org read goes through
  `maintenance::begin`, which sets the marker (`crates/cxp-core/src/maintenance.rs:27-33`); the
  tombstone tables `deleted_orgs` and `removed_members` are not under RLS and the owner may write
  them (`platform_events.rs:266-270`). The existing tests prove the policies as `otto_app`; the
  new e2e test proves them as the real connecting role, a table owner under FORCE.
- **Membership matters.** SUPERUSER and BYPASSRLS are never inherited, but a member can
  `SET ROLE` to the role that has them, so the check counts membership.
- **Two users, not one.** `current_user` alone misses a superuser login whose connections start
  with `SET ROLE` (`?options=-c role=...`, `ALTER ROLE ... SET role`): `RESET ROLE` returns to
  the superuser. The check reads `session_user` too.
- **Superuser-equivalent is more than SUPERUSER and BYPASSRLS.** CREATEROLE (before Postgres 16
  it grants itself any non-superuser role, BYPASSRLS ones included), CREATEDB (new databases on
  a shared instance), REPLICATION, and the predefined roles `pg_execute_server_program`,
  `pg_read_server_files`, `pg_write_server_files`, `pg_read_all_data`, `pg_write_all_data`,
  `pg_signal_backend`, `pg_read_all_stats`, `pg_monitor`, `pg_create_subscription`, and
  `pg_maintain` (17+) all reach past RLS, past `otto_cxp`, or into other services' sessions.
  `pg_database_owner` is not one: every database owner is implicitly its member, and it holds
  nothing beyond that database. Membership counts however it was granted: a Postgres 16
  `GRANT ... WITH INHERIT FALSE, SET TRUE` passes no privilege on, but still allows `SET ROLE`,
  so the check uses `pg_has_role(..., 'MEMBER')`.
- **Superuser-equivalence is not the only way out of `otto_cxp`.** A grant of `CONNECT` on
  another service's database (to `otto_app`, say, which every service's role belongs to) or
  membership in a role that owns one opens that database to a role with no privilege at all.

## Assumptions

- The production attach role is named after the app (`otto_cxp`) and owns nothing outside the
  `otto_cxp` database. The runbook checks both and refuses to proceed otherwise.
- `otto-db` runs Postgres 15 or later (16 per `docs/deploy/fly.md`), so schema `public` is owned
  by `pg_database_owner` and follows the database owner.
- The other services' attach roles are superusers too (so the `PUBLIC` revoke does not affect
  them); the runbook lists any non-superuser login role so it is granted `CONNECT` explicitly
  first.

## Goal & success criteria

- A deployment cannot boot on a session whose login user or current user is, or can become,
  superuser-equivalent, and says which user and which privilege; a developer's machine still
  can, with a warning.
- A test proves a role with none of those privileges, owning a fresh database, can migrate it,
  pass the boot checks, serve, apply `org.deleted`, sweep, and ship usage.
- Rob has a copy-paste runbook that moves production to such a role with no window in which the
  running release cannot connect.

## Scope

In: the boot check (`cxp_core::db_role`, `cxp-server` `boot.rs`, `config.rs`); the tests; the
runbook in `docs/deploy/fly.md`; `fly.toml`, `.env.example`, and `CLAUDE.md` wording.

Out: running the runbook (Rob); least-privilege roles for the other otto services; refusing a
deployment whose role cannot assume `otto_app` (isolation then still holds through FORCE, and
`verify_tenant_isolation` reports it); revoking `CONNECT` on `postgres`; re-checking the role
after boot (noted on #27).

## Architecture

### What "production" means

`Config::is_deployment()` (`crates/cxp-server/src/config.rs`) is true when any of these holds:
the public URL's host is not loopback (`localhost`, `127.0.0.0/8`, `::1`, the `is_loopback` that
already gates `CXP_PLATFORM_URL`'s plain http); `CXP_RESOURCE_URI`'s host is not loopback; or
`FLY_APP_NAME` is set (read with the rest of the config in `Config::from_lookup`, so tests stay
pure). The public URL alone is not enough: `CXP_RESOURCE_URI` is set independently and
`CXP_ALLOWED_HOSTS` can admit the real host, so a loopback public URL can still serve real
tokens. Chosen over a new `CXP_*` key because a key can be unset or mistyped in production,
silently disabling the check. Local development (`.env.example`: `http://localhost:8080`), CI,
and the existing e2e boot tests (`http://127.0.0.1:<port>` for both URLs) are loopback and keep
working with the test cluster's superuser. The cost: running locally behind a tunnel needs a
non-superuser local role, which `.env.example` shows how to make.

**The shared-instance rule.** URLs describe the process, not the database: a developer running
locally against `otto-db` through `fly proxy` has loopback URLs and production's data. So
`boot::check_db_role` also treats the process as a deployment when the instance hosts any of
`db_role::OTHER_SERVICE_DATABASES` (`otto_platform`, `otto_factory`, `otto_flags`) besides the
current database (`DbRole::shares_the_instance`, read from `pg_database`, which every role may
read). Why those three names and not a broader rule ("more than one non-template database",
"databases owned by other roles"):

- The test cluster and a developer's Postgres host many databases: `otto_cxp` beside every
  per-test database `#[sqlx::test]` creates (and keeps when a test fails). Any count- or
  owner-based rule would turn every test run into a "deployment" and refuse the superuser the
  e2e boot test runs as. `otto_cxp` itself is therefore not in the list either.
- The names are exactly the databases `otto-db` hosts beside `otto_cxp` today (the Topology
  section of `docs/deploy/fly.md`), so the signal is precise on production and never fires on a
  per-service local container. A developer who puts every otto service on one local Postgres is
  treated as a deployment and needs the local least-privilege role `.env.example` shows, which
  is the point: that instance holds other services' data.
- A new otto service on `otto-db` must be added to the list (the runbook's new-service note
  says so). Missing it weakens only this signal, not the CONNECT check below.

Tests stand in uniquely named databases through `db_role::inspect_with(db, names)`, so creating
them never makes the shared test cluster look like `otto-db` to the tests running beside.

### The check

- `cxp_core::db_role::inspect(&Db) -> DbRole` (new, `crates/cxp-core/src/db_role.rs`; SQL stays in
  cxp-core): `session_user` and `current_user`, and for each (once when they are equal):
  - every role with `rolsuper`, `rolbypassrls`, `rolcreaterole`, `rolcreatedb`, or
    `rolreplication`, or named in `db_role::PREDEFINED_ROLES` (matched by name, so a role a
    server version lacks is simply absent), that `pg_has_role(user, oid, 'MEMBER')`, the user
    itself first, at most 8 (a superuser is a member of every role, so for one only itself is
    kept);
  - unless the user is a superuser (already refused, and able to open every database), every
    other database it may `CONNECT` to (`has_database_privilege`), skipping templates,
    databases that disallow connections, the current database, `postgres`, and `repmgr`,
    explained by the first ACL entry (`aclexplode` of `datacl`, or `acldefault` when it is
    NULL) granting `CONNECT` to a role the user holds the privileges of
    (`pg_has_role(..., 'USAGE')`, which includes the owner's entry), else `PUBLIC`. It counts
    when the grant is not `PUBLIC`'s, or the database is one of `OTHER_SERVICE_DATABASES`; at
    most 8. `PUBLIC`'s default `CONNECT` elsewhere is not counted: every database created
    without a revoke has it, every per-test database included, and it opens only what
    `PUBLIC` holds there.

  Then the `OTHER_SERVICE_DATABASES` present on the instance. Each entry records which user
  holds it. `DbRole::summary()` is the log line, naming the session user when it differs, each
  reachable database with the grantee it is reached through, and the services sharing the
  instance.
- `boot::check_db_role(&DbRole, &Config)` (`crates/cxp-server/src/boot.rs`), called right after
  connecting and before migrations, so a refused role runs nothing. A session is refused when
  it is privileged or reaches another database (`DbRole::is_refused_in_a_deployment`); the
  process is a deployment when `Config::is_deployment()` or `DbRole::shares_the_instance()`:
  - not refused: `info`, `database role "cxp_server" holds no superuser-equivalent privilege
    (...), directly or through membership, and can connect to no other service's database`
    (plus `; the instance also hosts ...` on `otto-db`);
  - refused in a deployment: `error`, and `run` returns `refusing to serve: database role
    "otto_cxp" is SUPERUSER; ...` or `... can connect to database "otto_platform" (through
    "otto_app")` (the process exits non-zero, the Fly health check fails);
  - refused and not a deployment: `warn`, `... allowed only because this is not a deployment
    (loopback CXP_PUBLIC_URL and CXP_RESOURCE_URI, no FLY_APP_NAME, and no other otto service's
    database on the instance)`.
- `Boot` gains `database_role: String` (the summary), as it carries `isolation`.

### The production role

`cxp_server`: LOGIN, NOSUPERUSER, NOBYPASSRLS, NOCREATEDB, NOCREATEROLE, NOREPLICATION,
CONNECTION LIMIT 15 (two machines' pools of 5, a third during a rolling deploy, and a psql
session), in no predefined role; member
of `otto_app` with INHERIT and SET (otto-tenant decides whether to `SET LOCAL ROLE` with
`pg_has_role(..., 'USAGE')`, which needs INHERIT); owner of `otto_cxp` and, via `REASSIGN OWNED`,
of every object in it; default privileges for it re-granted to `otto_app`. `CONNECT` on the four
service databases and `repmgr` revoked from `PUBLIC` (each non-superuser role that uses one
granted it explicitly first, `repmgr` included) and granted back to `cxp_server` on `otto_cxp`
only. The runbook is `docs/deploy/fly.md` "Database role", R0 to R7; R2 runs under a 5-second
`lock_timeout`, R5 pipes the secret from `read -rs` through `printf` into `fly secrets import`,
and R7 is a guarded `DO` block that refuses a role this setup depends on, a database owner, a
role with a live session, or one that still owns relations in `otto_cxp`.

Ordering: R1 to R4 (create, transfer ownership, revoke, verify) cannot affect the running
release, which is a superuser. R5 sets `DATABASE_URL` without `--stage`, restarting the running
(pre-#16) release on the new role before this PR merges, so the switch is proven apart from the
code change. R6 is the merge. R7 makes the attach role NOLOGIN.

## Invariants impact

- Tenant isolation: tightened. The connecting role is now proven bound by RLS in a deployment,
  as long as it does not alter its own tables (see Risks).
- No SQL outside cxp-core: the catalog query lives in `cxp_core::db_role`.
- Small shared DB: each catalog query is bounded (`LIMIT 8` per user; the shared-instance query
  matches at most three names), and they run once, at boot.
- Migrations: none added.
- `CLAUDE.md`'s isolation invariant now says production's role is `cxp_server` once the runbook
  has run, and that until then a release with the check will not boot there.

## Migration

None.

## Out-of-band

Human (Rob), **before this PR merges**: the runbook R0 to R5 in `docs/deploy/fly.md` (role,
ownership, `CONNECT` revokes, verification, `DATABASE_URL` switch). After merge: R6 verification,
R7 (attach role NOLOGIN). No new config key, no new secret name (`DATABASE_URL` changes value).

## Error handling & edge cases

- Catalog read fails: boot fails with `could not read the database role's privileges`.
- A superuser lists only itself; a member of a privileged role is named with the role it can
  become.
- `otto_app` membership is not a privilege (it holds none of the attributes); the test asserts
  a role holding only that passes.
- A superuser login that `SET ROLE`s on connect: the session user is named as privileged.
- A Postgres version without `pg_maintain` or `pg_create_subscription`: the name matches no row.
- A grant to a role the user belongs to without INHERIT: `has_database_privilege` (which honors
  inheritance, as login does) decides whether the database is reachable at all; the grantee
  shown is one whose privileges the user holds.
- A REASSIGN guard failure aborts its transaction; nothing changes.

## Testing approach

- `cxp-core` `tests/db.rs`, logging in as real roles (dropped by a guard even when the test
  fails): `inspect` under a plain role, a BYPASSRLS role, a member of a BYPASSRLS role, a
  CREATEROLE role, a REPLICATION role, members of `pg_execute_server_program` and
  `pg_read_all_data`, and the superuser test pool; a superuser session that `SET ROLE`s to a
  plain role on connect; a CREATEDB role, a member of each of `pg_signal_backend`,
  `pg_monitor`, `pg_read_all_stats`, `pg_create_subscription` (and `pg_maintain` where the
  server has it), and a `WITH INHERIT FALSE, SET TRUE` member of a BYPASSRLS role; and, with
  stand-in service databases through `inspect_with`, `CONNECT` through `PUBLIC` on a service
  database (refused) but not on another (ignored), the R3 revoke, a grant to a group the role
  belongs to, membership in a database's owner, and the shared-instance signal.
- `cxp-server` `config.rs` unit test: loopback URLs without `FLY_APP_NAME` are not deployments;
  a non-loopback public URL or resource URI, or `FLY_APP_NAME`, is.
- `cxp-server` `boot.rs` unit test: a deployment refuses every privilege, held by the session or
  the current user, directly or through membership, and another service's database within
  reach; a non-deployment only warns; a shared instance is a deployment whatever the URLs say,
  where a clean role still serves.
- `cxp-server` `tests/e2e.rs`:
  - `the_service_runs_whole_as_a_least_privilege_database_owner`: on an empty test database, a
    fresh LOGIN role set up as the runbook does (owner, `otto_app` member, `CONNECT` revoked from
    `PUBLIC`) migrates it, then `run` boots as it with a non-loopback public URL: the role and
    isolation lines are logged in order, tools serve, `org.deleted` writes its tombstone and
    purges, the boot sweep removes stale `platform_events`, `removed_members`, and
    `usage_outbox_rejected` rows across orgs, the final flush ships the billable event, and the
    unpinned owner sees nothing in tenant tables. A mutation that drops the maintenance marker
    makes it fail.
  - `a_deployment_refuses_a_superuser_database_role`: boot refuses before migrating.
  - The existing boot test now also expects the loopback warning.
- The runbook SQL was dry-run against a local Postgres 16 with a simulated superuser attach role
  and stand-in `otto_platform`, `otto_factory`, `otto_flags`, and `repmgr` databases (and a
  non-superuser `repmgr` role), including the R2 guard refusing an `old_role` that owns other
  databases or does not own `public.customers`, R2 timing out on a held lock and changing
  nothing, and R7's guard refusing a wrong role, a database owner, a role with a live session,
  and one that still owns relations. The binary was booted against that instance with loopback
  URLs: refused as the attach role, refused as `cxp_server` with `CONNECT` granted to
  `otto_app`, served as `cxp_server` once the grant was revoked.

## Risks & open questions

- **Merge order is load-bearing.** Merged before R5, the deploy boots as the attach role, is
  refused, and fails its health check. The runbook and the PR hand-off say so.
- A non-superuser role of another service that connects through `PUBLIC` and is missed in R3
  fails at its next connection; R3's queries list them first, and a `GRANT CONNECT` fixes it
  immediately.
- The other services' attach roles stay superusers; a compromise of one of them still reaches
  `otto_cxp`. Their own issues.
- **RLS binds `cxp_server` only while it leaves its own tables alone.** As their owner it can
  `NO FORCE` or `DISABLE ROW LEVEL SECURITY`, `DROP POLICY`, and `TRUNCATE` (which ignores RLS
  altogether), so code execution as `cxp_server` could turn RLS off on, or empty,
  `otto_cxp`'s tables (not reach other databases). Fix: separate the migration owner from the
  runtime role, #27.
- **The check runs once, at boot.** A grant made later is caught at the next restart, not
  before; re-checking periodically is noted on #27.
- **`PUBLIC` `CONNECT` on a database not in `OTHER_SERVICE_DATABASES` is not refused.** On
  `otto-db` the runbook revokes it from every service database; a new service must revoke its
  own and join the list.
- **`otto_app` is shared by every otto service.** A database-level grant to it (above all
  `CONNECT`) would open that database to every service's login role. Rule, in
  `docs/deploy/fly.md`: no service grants `otto_app` any database-level privilege. Fix: a
  tenant role per service, #28.
