# A dedicated non-superuser database role for otto-cxp — design

**Status:** DRAFT (#16)
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

## Assumptions

- The production attach role is named after the app (`otto_cxp`) and owns nothing outside the
  `otto_cxp` database. The runbook checks both and refuses to proceed otherwise.
- `otto-db` runs Postgres 15 or later (16 per `docs/deploy/fly.md`), so schema `public` is owned
  by `pg_database_owner` and follows the database owner.
- The other services' attach roles are superusers too (so the `PUBLIC` revoke does not affect
  them); the runbook lists any non-superuser login role so it is granted `CONNECT` explicitly
  first.

## Goal & success criteria

- A deployment cannot boot as a role that is, or can become, a superuser or BYPASSRLS, and says
  why; a developer's machine still can, with a warning.
- A test proves a role with none of those privileges, owning a fresh database, can migrate it,
  pass the boot checks, serve, apply `org.deleted`, sweep, and ship usage.
- Rob has a copy-paste runbook that moves production to such a role with no window in which the
  running release cannot connect.

## Scope

In: the boot check (`cxp_core::db_role`, `cxp-server` `boot.rs`, `config.rs`); the tests; the
runbook in `docs/deploy/fly.md`; `fly.toml`, `.env.example`, and `CLAUDE.md` wording.

Out: running the runbook (Rob); least-privilege roles for the other otto services; refusing a
deployment whose role cannot assume `otto_app` (isolation then still holds through FORCE, and
`verify_tenant_isolation` reports it); revoking `CONNECT` on `postgres` and `repmgr`.

## Architecture

### What "production" means

`Config::is_deployment()` (`crates/cxp-server/src/config.rs`): the public URL's host is not
loopback (`localhost`, `127.0.0.0/8`, `::1`), reusing the `is_loopback` that already gates
`CXP_PLATFORM_URL`'s plain http. Chosen over a new `CXP_*` key because a key can be unset or
mistyped in production, silently disabling the check, while a production public URL that were
loopback would also advertise an audience no platform token carries and fail every request.
Local development (`.env.example`: `http://localhost:8080`), CI, and the existing e2e boot tests
(public URL `http://127.0.0.1:<port>`) are loopback and keep working with the test cluster's
superuser. The cost: running locally with a non-loopback public URL (a tunnel) needs a
non-superuser local role, which the runbook's R1 to R3 show how to make.

### The check

- `cxp_core::db_role::inspect(&Db) -> DbRole` (new, `crates/cxp-core/src/db_role.rs`; SQL stays in
  cxp-core): `current_user`, and every role with `rolsuper OR rolbypassrls` that
  `pg_has_role(current_user, oid, 'MEMBER')`, itself first, at most 8 (a superuser is a member of
  every role, so for one only itself is kept). `DbRole::summary()` is the log line.
- `boot::check_db_role(&DbRole, &Config)` (`crates/cxp-server/src/boot.rs`), called right after
  connecting and before migrations, so a refused role runs nothing:
  - not privileged: `info`, `database role "cxp_server" is neither a superuser nor BYPASSRLS, nor
    a member of one`;
  - privileged in a deployment: `error`, and `run` returns `refusing to serve: database role
    "otto_cxp" is SUPERUSER; ...` (the process exits non-zero, the Fly health check fails);
  - privileged on loopback: `warn`, `... allowed only because CXP_PUBLIC_URL is a loopback
    origin`.
- `Boot` gains `database_role: String` (the summary), as it carries `isolation`.

### The production role

`cxp_server`: LOGIN, NOSUPERUSER, NOBYPASSRLS, NOCREATEDB, NOCREATEROLE, NOREPLICATION; member
of `otto_app` with INHERIT and SET (otto-tenant decides whether to `SET LOCAL ROLE` with
`pg_has_role(..., 'USAGE')`, which needs INHERIT); owner of `otto_cxp` and, via `REASSIGN OWNED`,
of every object in it; default privileges for it re-granted to `otto_app`. `CONNECT` on the four
service databases revoked from `PUBLIC` and granted back to `cxp_server` on `otto_cxp` only. The
runbook is `docs/deploy/fly.md` "Database role", R0 to R7.

Ordering: R1 to R4 (create, transfer ownership, revoke, verify) cannot affect the running
release, which is a superuser. R5 sets `DATABASE_URL` without `--stage`, restarting the running
(pre-#16) release on the new role before this PR merges, so the switch is proven apart from the
code change. R6 is the merge. R7 makes the attach role NOLOGIN.

## Invariants impact

- Tenant isolation: tightened. The connecting role is now proven bound by RLS in a deployment.
- No SQL outside cxp-core: the catalog query lives in `cxp_core::db_role`.
- Small shared DB: the query is bounded (`LIMIT 8`).
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
- `otto_app` membership is not a privilege (it is neither SUPERUSER nor BYPASSRLS); the test
  asserts a role holding only that passes.
- A REASSIGN guard failure aborts its transaction; nothing changes.

## Testing approach

- `cxp-core` `tests/db.rs`: `inspect` under a plain role, a BYPASSRLS role, a member of a
  BYPASSRLS role, and the superuser test pool.
- `cxp-server` `config.rs` unit test: loopback hosts are not deployments; everything else is.
- `cxp-server` `boot.rs` unit test: a deployment refuses superuser, BYPASSRLS, and membership; a
  loopback public URL only warns.
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
- The runbook SQL was dry-run against a local Postgres 16 with a simulated superuser attach role,
  including the guard refusing an `old_role` that owns other databases.

## Risks & open questions

- **Merge order is load-bearing.** Merged before R5, the deploy boots as the attach role, is
  refused, and fails its health check. The runbook and the PR hand-off say so.
- A non-superuser role of another service that connects through `PUBLIC` and is missed in R3
  fails at its next connection; R3's queries list them first, and a `GRANT CONNECT` fixes it
  immediately.
- The other services' attach roles stay superusers; a compromise of one of them still reaches
  `otto_cxp`. Their own issues.
