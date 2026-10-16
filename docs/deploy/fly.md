# Hosting: Fly.io

otto-cxp runs as one Fly app, `otto-cxp` (org `savvagent`, region `iad`), serving every surface
on one port: the MCP endpoint agents use, the REST API apps use, the web console (signed in
through the platform), and the platform's webhooks.
Design: `docs/specs/2026-10-14-otto-resource-server-design.md`.

## Topology

```
  otto-platform  (https://otto.savvagent.com)          otto-cxp  (https://otto-cxp.savvagent.com)
  OAuth AS, orgs, members, teams, billing               POST /mcp               agents, bearer tokens
  db: otto_platform ◄──── same instance ────►           /api/v1/...             apps, bearer tokens
                                                        db: otto_cxp            customers, tickets, feedback,
        ▲  POST /oauth/introspect, GET /internal/orgs/... ◄── per request (cached)   surveys, catalog
        │  POST /internal/usage                          ◄── background shipper (usage_outbox)
        └─► POST /platform/webhooks (org.deleted, member.removed, team.deleted) ──►
```

Both `/mcp` and `/api/v1` require a platform-issued token whose audience is exactly
`CXP_RESOURCE_URI`. Identity, orgs, teams, plans, and usage totals belong to the platform; this
database holds only opaque `org_id`, user, and `team_id` uuids.

## Infrastructure

- **App** `otto-cxp`: shared-cpu-1x, 256 MB, `auto_stop_machines = "suspend"`,
  `min_machines_running = 0`. Idle costs nothing; the first request after idle resumes the
  machine. Raising `min_machines_running` is a cost decision (Rob).
  While the machine is suspended its background tasks are suspended with it: the usage shipper
  does not post `usage_outbox` rows to the platform and the hourly platform-event sweep does not
  run. Both resume with the machine. A suspend sends the process no signal, so nothing is
  flushed then: usage recorded before it stays in `usage_outbox` and ships on resume. A stop
  (a deploy, `fly machine stop`) sends SIGTERM, and the final flush ships what it can within
  its 10 s bound. Either way billing is delayed, never lost; the platform's usage totals can
  trail by up to the idle period.
- **Database** `otto_cxp` on `otto-db`, the unmanaged Fly Postgres shared with `otto_platform`,
  `otto_factory`, and `otto_flags`. Plain Postgres 16 is enough: pgvector is NOT needed (the
  schema uses only the stock `pg_trgm`). otto-cxp serves as `cxp_server`, a dedicated role
  that owns nothing: not a superuser, not BYPASSRLS, no CREATEDB or CREATEROLE, a member of
  `cxp_app` and `cxp_runtime` (and of the shared `otto_app` until
  [runbook T5](#runbook-give-otto-cxp-its-own-tenant-role-human-only-rob)), and with `cxp_owner` the only roles besides superusers that may
  connect to `otto_cxp`. `cxp_owner` owns `otto_cxp` and everything in it, and logs in only from
  the release command, to migrate (see [Database role](#database-role)). Tenant transactions run
  as `cxp_app`, otto-cxp's own tenant role ([#28](https://github.com/savvagent/otto-cxp/issues/28)),
  under row-level security, which `cxp-server` proves at boot
  (`tenant isolation enforced as role "cxp_app"`) and refuses to start without; everything
  else (the sweep and usage shipper, tombstones, `/readyz`) runs as `cxp_server`, which RLS binds
  like any non-owner and which can neither alter, drop, nor truncate a table
  ([#27](https://github.com/savvagent/otto-cxp/issues/27)). A deployment refuses to serve when
  the connecting role is, or can `SET ROLE` to, a superuser-equivalent role, or can connect to
  another service's database ([#16](https://github.com/savvagent/otto-cxp/issues/16)), or owns
  anything in `otto_cxp` or may `TRUNCATE` or add a `TRIGGER` to a table (#27), so the role
  `fly postgres attach` creates, a superuser on the whole instance, is refused, and so is the
  owner.
  Customer and knowledge-base search run under RLS, whose per-row policy checks keep the
  planner off index paths for `ILIKE`, `%`, and `@@`; since
  [#17](https://github.com/savvagent/otto-cxp/issues/17) they look candidates up in their own
  search index tables (migration 0004), read a bounded number of rows per query, and count
  matches up to 1000 (`docs/specs/2026-10-14-search-under-rls-design.md`). The index costs about
  7 kB per customer and 25 kB per published article as measured on a bulk-loaded benchmark;
  indexes grown a row at a time run 25-30% larger. When to act on that size, and the query that
  watches it, are in [Search index storage](#search-index-storage-34). The pool is
  capped at 5 connections (`CXP_DB_MAX_CONNECTIONS`) because the instance is small and shared.
  Split onto a dedicated instance before real customers.
- **Migrations** are otto-cxp's own (`crates/cxp-core/migrations`). They run in Fly's release
  command (`[deploy] release_command = "migrate"`, so `cxp-server migrate` under the image's
  entrypoint), as `cxp_owner` through `CXP_MIGRATION_DATABASE_URL`, in a temporary machine of the
  new image before any machine is updated, under sqlx's advisory lock; a failure aborts the
  deploy and leaves the old release serving. The serving machines run with
  `CXP_RUN_MIGRATIONS=false` and never migrate. Never run the platform's migrations here.

## Secrets

| Secret | What | Where from |
|---|---|---|
| `DATABASE_URL` | Postgres URL for `otto_cxp`, as `cxp_server` (the runtime role, which owns nothing) | [Database role](#database-role) (replaces the one `fly postgres attach` set) |
| `CXP_MIGRATION_DATABASE_URL` | Postgres URL for `otto_cxp`, as `cxp_owner`. Read only by the release command (`cxp-server migrate`); a deployment's `migrate` refuses to run without it, and the serving process never reads it | [Split the migration owner](#runbook-split-the-migration-owner-from-the-runtime-role-human-only-rob), S6 (staged, so only a release that sheds it receives it) |
| `CXP_RUN_MIGRATIONS` | `false`. Also in `fly.toml` `[env]`; kept as a secret (which wins over `[env]`) so a rollback to an image from before #27, whose `fly.toml` lacks it, does not try to migrate as the runtime role | Same runbook, S1 |
| `CXP_INTROSPECTION_SECRET` | `otto_rs_...`, authenticates every call to the platform | `resource rotate-secret` |
| `CXP_PLATFORM_WEBHOOK_SECRET` | `otto_whsec_...`, verifies platform webhooks | `resource set-webhook` |
| `CXP_ENCRYPTION_KEY` | 32 bytes, base64. Never used directly: two HKDF-SHA256 subkeys seal the web console's session tokens at rest and its in-flight sign-in cookie. Required only once `CXP_CONSOLE_CLIENT_ID` is set; rotating it signs every console user out | `openssl rand -base64 32` |

Everything else is in `fly.toml`'s `[env]`. A new secret needs `fly secrets set -a otto-cxp
--stage NAME=...` before the release that reads it, plus a row here and in `.env.example`.

## Database role

Four roles, since #27 ([split runbook](#runbook-split-the-migration-owner-from-the-runtime-role-human-only-rob)) and #28 ([tenant role runbook](#runbook-give-otto-cxp-its-own-tenant-role-human-only-rob)):

| Role | Login | Owns | Holds | Used by |
|---|---|---|---|---|
| `cxp_server` | yes | nothing | member of `cxp_app` and `cxp_runtime` (and of `otto_app` until T5); `CONNECT` on `otto_cxp` | the serving machines (`DATABASE_URL`) |
| `cxp_owner` | yes | database `otto_cxp` and every object in it | `SET` on the parameter `app.console_session_lookup` | the release command, `cxp-server migrate` (`CXP_MIGRATION_DATABASE_URL`) |
| `cxp_runtime` | no | nothing | `INSERT` on `deleted_orgs` (migration 0005) | the runtime's grants beyond the tenant role |
| `cxp_app` | no | nothing | `otto_cxp`'s tenant grants, the same as `otto_app`'s there (migration 0007) | `SET LOCAL ROLE` in tenant transactions; inherited by `cxp_server` for the background work |

All four are NOSUPERUSER, NOBYPASSRLS, NOCREATEDB, NOCREATEROLE, NOREPLICATION; `cxp_server`
has CONNECTION LIMIT 15, `cxp_owner` 3. `cxp_server` is a member of `cxp_app` so tenant
transactions can `SET LOCAL ROLE cxp_app`, and of `cxp_runtime` for the one thing it does as
itself that a tenant transaction may not: write the `org.deleted` tombstone. Tenant transactions
use `cxp_app` and not `cxp_runtime` because `cxp_runtime` holds what the runtime does as itself
(the tombstone, and every future background-only privilege), and making it the `SET LOCAL ROLE`
target would hand all of that to every request. Two roles keep the sets apart: `cxp_app` is the
request's identity and holds only the tenant grants, `cxp_runtime` the background extras, and
the login inherits both. A migration cannot name a
deployment's login, but it can name `cxp_runtime`; a later migration that gives background work
a new privilege grants it there too. `cxp_owner` owns the database so it can migrate (`pg_trgm`
is a trusted extension a database owner may create; the parameter grant lets it define
migration 0003's `console_session_org`, whose `SET` clause names that parameter).

Why the split: an owner can `ALTER TABLE ... NO FORCE ROW LEVEL SECURITY`, `DISABLE ROW LEVEL
SECURITY`, or `DROP POLICY`, and `TRUNCATE` ignores RLS altogether, so any SQL running as an
owning runtime role (the sweep, the shipper, the tombstone, `/readyz`, a future injection bug)
could switch tenant isolation off for every org. `cxp_server` owns nothing and holds no
`TRUNCATE` or `TRIGGER`, so it cannot. The owner never serves: its credential is used only by the
release command. (A NOLOGIN owner that `cxp_server` could assume would not do: assuming a role is
a `SET ROLE`, which any SQL the runtime login runs could repeat; and a NOLOGIN owner assumed by
a second login only moves the exposure to that login's credential.)

**The owner's credential and the serving machines.** Fly secrets are app-wide, so
`CXP_MIGRATION_DATABASE_URL` reaches every serving machine, not only the release command's. A
serving `cxp-server` therefore sheds it before doing anything else: finding it in its
environment, it re-executes itself with the same arguments and the environment minus that
variable, before reading its configuration or starting a thread (`unsetenv` alone would leave the
original environment block in memory and in `/proc/<pid>/environ`). It logs `removed
CXP_MIGRATION_DATABASE_URL from the serving process's environment; only cxp-server migrate uses
it`, never reads or logs the value, and refuses to start if the re-exec fails
(`crates/cxp-server/tests/secret_env.rs` proves it on the built binary). So SQL run as
`cxp_server` cannot reach the owner (it is a different login), and code execution or an
environment dump in the serving process does not find its credential either.

What remains: Fly's init (PID 1, root) still holds the app's secrets for the machine's life, so
root on a serving machine (`fly ssh console`, or an exploit that escalates from the
unprivileged `cxp` user to root) can read the owner's credential, and so can anyone with the
app's Fly deploy token (`fly ssh`, or a machine they start). Closing that means the owner's
credential living outside this app: a separate Fly app (say `otto-cxp-migrate`) holding only
`CXP_MIGRATION_DATABASE_URL` and running `cxp-server migrate` as a one-off machine from the
deploy job before `fly deploy`. That is a follow-up,
[#45](https://github.com/savvagent/otto-cxp/issues/45), not done here: it adds an app, its own
deploy token, and ordering in CI.

The `SECURITY DEFINER` functions run as their owner, `cxp_owner`: `console_session_org` (the
console's cookie lookup, which `cxp_server` may call through `cxp_app`'s `EXECUTE`) and
`expire_removed_members` (the sweep). FORCE ROW LEVEL SECURITY binds `cxp_owner` too, and the
`console_sessions_lookup` policy admits whichever role owns `console_session_org` at query time,
so the lookup followed the ownership transfer with nothing to change. The maintenance policies
name no role, so they bind the non-owning `cxp_server` exactly as they bound the owner.

`CONNECT` on `otto_cxp`, `otto_platform`, `otto_factory`, `otto_flags`, and `repmgr` is revoked
from `PUBLIC`, so neither login can open another service's database.

`otto_app` is shared by every otto service, which is one more reason the revoke matters: a role
that could connect to `otto_platform` would hold whatever `otto_app` holds there. So, a rule for
every service on `otto-db`: **no service may grant `CONNECT`, or any other database-level
privilege, to `otto_app`**. A grant to it would hand that database to every service's login
role. Since [#28](https://github.com/savvagent/otto-cxp/issues/28) otto-cxp no longer uses
`otto_app`: its tenant transactions assume its own `cxp_app`, and boot refuses to serve
otherwise. `cxp_server` leaves `otto_app` in runbook T5; the rule stays for every service while
`otto_app` exists.

A deployment refuses to serve on a connection that is superuser-equivalent or can reach
another service's database. The process is a deployment when any of these holds:

- `CXP_PUBLIC_URL` or `CXP_RESOURCE_URI` has a non-loopback host;
- `FLY_APP_NAME` is set (Fly sets it on every machine);
- the Postgres instance also hosts `otto_platform`, `otto_factory`, or `otto_flags`, so a local
  run pointed at `otto-db` through `fly proxy` counts, whatever its URLs say.

Boot reads both the session user (who logged in, and who `RESET ROLE` returns to) and the
current user, and refuses when either:

- is, or is a member of, a role with SUPERUSER, BYPASSRLS, CREATEROLE, CREATEDB, or REPLICATION;
- is a member of a cross-service predefined role: `pg_execute_server_program`,
  `pg_read_server_files`, `pg_write_server_files`, `pg_read_all_data`, `pg_write_all_data`,
  `pg_signal_backend`, `pg_read_all_stats`, `pg_monitor`, `pg_create_subscription`, or
  `pg_maintain` (Postgres 17; each is looked up by name, so a version without one is fine).
  Membership counts whether it was granted with INHERIT or only SET. `pg_database_owner`, which
  every database owner belongs to, is allowed;
- can `CONNECT` to `otto_platform`, `otto_factory`, or `otto_flags` in any way, `PUBLIC`
  included, or to any other database through a grant to, or ownership by, a role it belongs to.
  Templates, `postgres`, and `repmgr` are not counted, and neither is `PUBLIC`'s default
  `CONNECT` on a database that is not another service's (every database created without a
  revoke has it, such as each test database `cargo test` creates).

A serving process (not `cxp-server migrate`, whose role is meant to own) also refuses when either
user owns, or is a member of a role that owns, anything in `otto_cxp`: the database, a schema
(`public` follows the database owner through `pg_database_owner`), a table, sequence, index or
other relation, a function, a type, or an extension; or may `TRUNCATE` or add a `TRIGGER` to any
table, view, or foreign table; or may `CREATE` in the database or one of its schemas, or create
`TEMPORARY` tables there (a temporary table named like a tenant table would shadow it, with no
RLS, for every later transaction on that pooled connection; `CREATE` on a schema would let it
plant an overload a `SECURITY DEFINER` function resolves) — each through any role it inherits or
can only `SET ROLE` to, `PUBLIC` included. And it refuses `CXP_RUN_MIGRATIONS=true` (the runtime
role cannot migrate), and a runtime role that lacks what its own work needs: `INSERT` on
`deleted_orgs` (the tombstone) and inheriting `cxp_app` (the background tasks); the message
names the fix (`GRANT INSERT ON deleted_orgs TO cxp_runtime; GRANT cxp_runtime TO "<role>"`,
`GRANT cxp_app TO "<role>" WITH INHERIT TRUE`). Migration 0006 also pins both `SECURITY DEFINER`
functions to `search_path = pg_catalog, public, pg_temp` with every call schema-qualified.

Every boot, deployment or not, also refuses to serve unless tenant transactions assumed
`cxp_app` (`refusing to serve: tenant transactions run as "<effective>", not otto-cxp's own
tenant role cxp_app (it does not exist, or "<login>" cannot assume it): as a superuser, CREATE
ROLE cxp_app NOLOGIN; GRANT cxp_app TO "<login>" WITH INHERIT TRUE; then migrate (docs/deploy/fly.md, runbook
T1)`), and a deployment warns while the login is still a member of `otto_app` (`database role
"cxp_server" is still a member of otto_app, the tenant role the otto services share; this
release no longer uses it: REVOKE otto_app FROM "cxp_server" once it is verified (docs/deploy/fly.md,
runbook T5)`).

It logs, for example,
`database role "otto_cxp" is SUPERUSER, BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION: refusing to serve`,
`database role "cxp_owner" ...; it owns database "otto_cxp", schema "public", table ... (N objects in all)`
for the owner given as `DATABASE_URL`,
`database role "cxp_server" (session user "otto_cxp"): session user "otto_cxp" is SUPERUSER`
for a superuser login whose connections `SET ROLE cxp_server`, or
`database role "cxp_server" can connect to database "otto_platform" (through "otto_app")`.
A developer's machine (loopback URLs, no `FLY_APP_NAME`, none of those databases on its
Postgres) only warns, since a local Postgres hands out a superuser.

**The check runs again every hour,** after the retention sweep, because a later `ALTER ROLE cxp_server BYPASSRLS`, `GRANT pg_read_all_data TO
cxp_server`, or `GRANT TRUNCATE ... TO cxp_server` would otherwise go unnoticed until a restart.
When the role is one a deployment would refuse, the machine logs at `error`
`the database role changed after boot: <summary>; /readyz answers 503 until it is fixed`, and
`/readyz` answers 503 `{"status":"unready","reason":"database role refused"}` (each probe logs a
`warn` with that fixed reason). `/readyz` is the app's only health check, so with one machine
**this takes the service out of rotation**, deliberately: it is the role boot refuses. The outage
lasts until the next hourly pass after the fix or a restart. To recover, undo the change (revoke
the grant or attribute); the next pass clears the alarm and logs `the database role re-check
passes again`, or restart the machines (`fly apps restart otto-cxp`) to re-run the boot check at
once. The re-check gives up after 30 seconds; a failed or timed-out re-check leaves the alarm as
it was and logs `warn`, and from the third failure in a row `error` (`the role is not being
watched`). The hourly pass also re-checks the runtime grants and logs `error` if one went
missing (without the alarm: org.deleted would fail and be retried by the platform, nothing is
exposed).

### Runbook: from the attach role to `cxp_server` (human-only, Rob)

Every step is production DDL on the shared `otto-db`, or a production secret; the agent that
wrote this ran it only against a local container. Do the steps in order: until R5 the running
release still connects as the attach role, a superuser, which none of R1 to R4 can lock out, and
R5 switches it only after R4 has proven the new role. **R1 to R5 must be done before the PR for
#16 merges**: merging deploys, and that release refuses to boot as the attach role. Paste each
SQL block whole; a failure inside `BEGIN ... COMMIT` rolls the block back.

**R0. Look first (read-only).** Wake the app (`curl -s https://otto-cxp.savvagent.com/readyz`),
then note the user, host, port, and query string of the URL it connects with (the password is
masked):

```bash
fly ssh console -a otto-cxp -C 'printenv DATABASE_URL' | sed -E 's#(//[^:/@]+):[^@]*@#\1:***@#'
fly postgres connect -a otto-db        # psql as postgres, the instance superuser
```

```sql
\set ON_ERROR_STOP on
-- The attach role: the user in the URL above (usually otto_cxp).
\set old_role otto_cxp
-- Every role. otto_app must exist and be f/f/f/f below; if it is missing, run
-- CREATE ROLE otto_app NOLOGIN; before R1.
SELECT rolname, rolsuper, rolbypassrls, rolcreaterole, rolcreatedb, rolcanlogin
FROM pg_roles WHERE rolname !~ '^pg_' ORDER BY rolname;
-- Databases, their owners, and explicit privileges.
SELECT datname, pg_get_userbyid(datdba) AS owner, datacl
FROM pg_database WHERE NOT datistemplate ORDER BY datname;
-- Who is connected where right now.
SELECT datname, usename, count(*) FROM pg_stat_activity
WHERE datname IS NOT NULL GROUP BY 1, 2 ORDER BY 1, 2;
-- What the attach role owns outside otto_cxp. Must be 0 rows: REASSIGN OWNED (R2) moves
-- shared objects (databases, tablespaces) too. R2 refuses to run otherwise.
SELECT 'database' AS kind, datname::text AS name FROM pg_database
WHERE datdba = :'old_role'::regrole AND datname <> 'otto_cxp'
UNION ALL
SELECT 'tablespace', spcname::text FROM pg_tablespace WHERE spcowner = :'old_role'::regrole;
```

**R1. Create the role.** Generate its password locally, where only you see it, and keep it
in your password manager until R5 (hex, so it needs no escaping in a URL):

```bash
openssl rand -hex 32
```

In the `fly postgres connect` session (`\password` hashes it client-side, so the password never
reaches the server's statement log):

```sql
-- CONNECTION LIMIT: two machines' pools (CXP_DB_MAX_CONNECTIONS = 5 each), a third during a
-- rolling deploy, and a psql session, so a misconfigured pool cannot exhaust the shared
-- instance. Raise it with the pool size or the machine count.
CREATE ROLE cxp_server LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION
  CONNECTION LIMIT 15;
\password cxp_server
-- INHERIT and SET, the defaults: otto-tenant needs both to SET LOCAL ROLE otto_app.
GRANT otto_app TO cxp_server;
-- Migration 0003's console_session_org sets this custom parameter in its SET clause, which only
-- a superuser or a role granted SET on it may define (Postgres 15+). Without the grant, a
-- migration run as cxp_server (a fresh setup, or a release reaching 0003 after R5) fails with
-- `permission denied to set parameter "app.console_session_lookup"`. Calling the function needs
-- nothing: a non-superuser may set any custom parameter at run time.
GRANT SET ON PARAMETER app.console_session_lookup TO cxp_server;
```

**R2. Give it `otto_cxp`.** Still as postgres, in `otto_cxp` (`REASSIGN OWNED` acts on the
current database's objects plus shared ones; the guard checks there are no others).
`REASSIGN OWNED` takes an exclusive lock on every table, so the block waits at most 5 seconds
for each (`lock_timeout`) rather than queueing the running release's requests behind it. If it
fails with `canceling statement due to lock timeout`, nothing changed: paste it again, or run
it while the app is idle (suspended, `fly status -a otto-cxp`).

```sql
\c otto_cxp
\set ON_ERROR_STOP on
\set old_role otto_cxp
BEGIN;
SET LOCAL lock_timeout = '5s';
SELECT set_config('runbook.old_role', :'old_role', true);
DO $$
DECLARE
  old regrole := current_setting('runbook.old_role')::regrole;
BEGIN
  IF current_database() <> 'otto_cxp' THEN
    RAISE EXCEPTION 'run this connected to otto_cxp, not %', current_database();
  END IF;
  IF old::text IN ('cxp_server', 'postgres', 'repmgr', 'flypgadmin', 'otto_app') THEN
    RAISE EXCEPTION 'old_role must be the attach role, not %', old;
  END IF;
  -- The attach role ran the migrations, so it owns the tables (a fresh setup has none yet).
  IF to_regclass('public.customers') IS NULL THEN
    IF EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
               WHERE n.nspname = 'public') THEN
      RAISE EXCEPTION 'public.customers is missing but public is not empty: is this otto_cxp?';
    END IF;
  ELSIF (SELECT relowner FROM pg_class WHERE oid = 'public.customers'::regclass) <> old THEN
    RAISE EXCEPTION 'old_role % does not own public.customers (%): is it the attach role?',
      old, (SELECT pg_get_userbyid(relowner) FROM pg_class
            WHERE oid = 'public.customers'::regclass);
  END IF;
  IF EXISTS (SELECT 1 FROM pg_database WHERE datdba = old AND datname <> 'otto_cxp')
     OR EXISTS (SELECT 1 FROM pg_tablespace WHERE spcowner = old) THEN
    RAISE EXCEPTION '% owns a database or tablespace besides otto_cxp, which REASSIGN OWNED would move too', old;
  END IF;
END $$;
REASSIGN OWNED BY :"old_role" TO cxp_server;
ALTER DATABASE otto_cxp OWNER TO cxp_server;
-- The baseline's default privileges were set for the attach role; tables a later migration
-- creates as cxp_server get the same grants.
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO otto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO otto_app;
COMMIT;

-- Everything in otto_cxp is cxp_server's now, the console's sign-in objects (0003) included:
-- table console_sessions and the SECURITY DEFINER function console_session_org, which runs as
-- its owner. Must be 0 rows.
SELECT 'relation' AS kind, c.relname::text AS name, pg_get_userbyid(c.relowner) AS owner
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relowner <> 'cxp_server'::regrole
UNION ALL
SELECT 'type', t.typname::text, pg_get_userbyid(t.typowner)
FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
WHERE n.nspname = 'public' AND t.typowner <> 'cxp_server'::regrole
UNION ALL
SELECT 'function', p.proname::text, pg_get_userbyid(p.proowner)
FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname = 'public' AND p.proowner <> 'cxp_server'::regrole
UNION ALL
SELECT 'extension', e.extname::text, pg_get_userbyid(e.extowner)
FROM pg_extension e WHERE e.extname <> 'plpgsql' AND e.extowner <> 'cxp_server'::regrole
UNION ALL
SELECT 'database', d.datname::text, pg_get_userbyid(d.datdba)
FROM pg_database d WHERE d.datname = 'otto_cxp' AND d.datdba <> 'cxp_server'::regrole
UNION ALL
-- Schema public follows the database owner through pg_database_owner (Postgres 15+).
SELECT 'schema', n.nspname::text, pg_get_userbyid(n.nspowner)
FROM pg_namespace n WHERE n.nspname = 'public'
  AND n.nspowner NOT IN ('cxp_server'::regrole, 'pg_database_owner'::regrole);

-- cxp_server: the web console's session lookup runs as this owner (0 rows on a fresh setup,
-- before the first deploy has migrated).
SELECT pg_get_userbyid(proowner) AS console_session_org_owner FROM pg_proc
WHERE oid = to_regprocedure('public.console_session_org(bytea)');
```

The `console_sessions_lookup` policy needs no `ALTER POLICY`: it names no role, and admits
whichever role owns `console_session_org` at query time, so `REASSIGN OWNED` above moved it too.

**R3. Close the other databases.** A role that connects to `otto_platform`, `otto_factory`,
`otto_flags`, or `repmgr` only through `PUBLIC` loses access at the revoke, so first find every
non-superuser login role that can connect today (superusers, such as the other services' attach
roles, skip privilege checks and need nothing):

```sql
SELECT r.rolname, string_agg(d.datname, ', ' ORDER BY d.datname) AS can_connect_today
FROM pg_roles r CROSS JOIN pg_database d
WHERE r.rolcanlogin AND NOT r.rolsuper AND r.rolname <> 'cxp_server'
  AND d.datname IN ('otto_platform', 'otto_factory', 'otto_flags', 'repmgr')
  AND has_database_privilege(r.oid, d.oid, 'CONNECT')
GROUP BY r.rolname ORDER BY r.rolname;
```

For each row, find which of those databases that role legitimately uses (the one in its
service's `DATABASE_URL`, e.g. `fly ssh console -a otto-flags -C 'printenv DATABASE_URL'`, plus
any other its owner confirms it needs) and grant `CONNECT` on each of them, e.g.
`GRANT CONNECT ON DATABASE otto_flags TO <role>;`. Never grant to `otto_app` (see above). The
grants are a prerequisite, not a follow-up: before the revoke, this must list only roles you
have confirmed do not use that database (and `repmgr` on database `repmgr`, which the block
below grants):

```sql
-- Login roles that would lose CONNECT at the revoke: they hold it only through PUBLIC.
SELECT r.rolname, d.datname
FROM pg_roles r CROSS JOIN pg_database d
WHERE r.rolcanlogin AND NOT r.rolsuper AND r.rolname <> 'cxp_server'
  AND d.datname IN ('otto_platform', 'otto_factory', 'otto_flags', 'repmgr')
  AND has_database_privilege(r.oid, d.oid, 'CONNECT')
  AND NOT EXISTS (
    SELECT 1 FROM aclexplode(coalesce(d.datacl, acldefault('d', d.datdba))) a
    WHERE a.privilege_type = 'CONNECT' AND a.grantee <> 0
      AND pg_has_role(r.oid, a.grantee, 'USAGE'))
ORDER BY 1, 2;
```

Then:

```sql
\set ON_ERROR_STOP on
REVOKE CONNECT ON DATABASE otto_platform, otto_factory, otto_flags, otto_cxp FROM PUBLIC;
-- Belt and braces: as otto_cxp's owner since R2, cxp_server already holds CONNECT there.
GRANT CONNECT ON DATABASE otto_cxp TO cxp_server;
-- repmgr (Fly's replication manager, which connects to database repmgr as role repmgr): grant
-- the role explicitly first when it is not a superuser, then revoke PUBLIC's.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_database WHERE datname = 'repmgr') THEN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'repmgr' AND NOT rolsuper) THEN
      GRANT CONNECT ON DATABASE repmgr TO repmgr;
    END IF;
    REVOKE CONNECT ON DATABASE repmgr FROM PUBLIC;
  END IF;
END $$;

-- Who may connect now: superusers, the grants just made, and cxp_server on otto_cxp only.
SELECT d.datname, string_agg(r.rolname, ', ' ORDER BY r.rolname) AS may_connect
FROM pg_database d CROSS JOIN pg_roles r
WHERE d.datname IN ('otto_platform', 'otto_factory', 'otto_flags', 'otto_cxp', 'repmgr')
  AND r.rolcanlogin AND has_database_privilege(r.oid, d.oid, 'CONNECT')
GROUP BY d.datname ORDER BY d.datname;
```

Existing sessions are not dropped by a revoke; a role that was missed fails at its next
connection, and `GRANT CONNECT ON DATABASE <db> TO <role>;` fixes it at once. To undo R3 whole
(the `repmgr` line only if that database exists):

```sql
GRANT CONNECT ON DATABASE otto_platform, otto_factory, otto_flags, otto_cxp TO PUBLIC;
GRANT CONNECT ON DATABASE repmgr TO PUBLIC;
```

The `postgres` database is left as it is: no service data, and tools connect to it by default.
A database created later starts with `CONNECT` for `PUBLIC` again, so a new service on `otto-db`
must run `REVOKE CONNECT ON DATABASE <its db> FROM PUBLIC;` and grant its own role as part of
its setup (and, if it is a new otto service, be added to `cxp_core::db_role`'s
`OTHER_SERVICE_DATABASES`).

**R4. Prove the role before using it (read-only).** In a second terminal
`fly proxy 15499:5432 -a otto-db`, then:

```bash
psql "postgres://cxp_server@localhost:15499/otto_cxp"     # prompts for the R1 password
```

```sql
-- cxp_server, cxp_server: nothing switched roles on connect
SELECT session_user, current_user;
-- f, f, f, f, f, 15
SELECT rolname, rolsuper, rolbypassrls, rolcreaterole, rolcreatedb, rolreplication, rolconnlimit
FROM pg_roles WHERE rolname = current_user;
-- 0 rows: the privileges boot checks, for the session user and the current user
SELECT u.who, u.usr, r.rolname
FROM (VALUES ('session_user', session_user), ('current_user', current_user)) AS u(who, usr)
CROSS JOIN pg_roles r
WHERE (r.rolsuper OR r.rolbypassrls OR r.rolcreaterole OR r.rolcreatedb OR r.rolreplication
       OR r.rolname IN ('pg_execute_server_program', 'pg_read_server_files',
                        'pg_write_server_files', 'pg_read_all_data', 'pg_write_all_data',
                        'pg_signal_backend', 'pg_read_all_stats', 'pg_monitor',
                        'pg_create_subscription', 'pg_maintain'))
  AND pg_has_role(u.usr, r.oid, 'MEMBER');
-- 0 rows: the other databases boot checks (the first query proved both users are cxp_server)
SELECT d.datname, coalesce(g.through, 'PUBLIC') AS through
FROM pg_database d
LEFT JOIN LATERAL (
  SELECT CASE WHEN a.grantee = 0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee)::text END
           AS through
  FROM aclexplode(coalesce(d.datacl, acldefault('d', d.datdba))) a
  WHERE a.privilege_type = 'CONNECT'
    AND (a.grantee = 0 OR pg_has_role(current_user, a.grantee, 'USAGE'))
  ORDER BY a.grantee = 0, a.grantee LIMIT 1) g ON true
WHERE d.datallowconn AND NOT d.datistemplate
  AND d.datname NOT IN (current_database(), 'postgres', 'repmgr')
  AND has_database_privilege(current_user, d.oid, 'CONNECT')
  AND (g.through IS DISTINCT FROM 'PUBLIC'
       OR d.datname IN ('otto_platform', 'otto_factory', 'otto_flags'));
-- t, t: it can assume otto_app, and migrate
SELECT pg_has_role(current_user, 'otto_app', 'USAGE') AS assumes_otto_app,
       has_schema_privilege('public', 'CREATE') AS can_migrate;
-- 0: row-level security binds it with no org pinned
SELECT count(*) FROM customers;
-- 0, then NULL with no error: the sessions are bound too, and the console's lookup runs as
-- cxp_server (skip both on a fresh setup, before the first deploy has migrated)
SELECT count(*) FROM console_sessions;
SELECT console_session_org(decode(repeat('00', 32), 'hex'));
-- t for otto_cxp and postgres; f for the other services' databases and repmgr
SELECT datname, has_database_privilege(current_user, oid, 'CONNECT') AS may_connect
FROM pg_database WHERE NOT datistemplate ORDER BY datname;
-- must fail: permission denied for database "otto_platform"
\c otto_platform
```

If the other-databases query lists a row, or that `\c` succeeds, something still grants
`cxp_server` `CONNECT` there. It has no grant of its own and the R3 revoke removed `PUBLIC`'s,
so the grant is to a role it belongs to (the `through` column), almost certainly `otto_app`.
Boot would refuse it. Every login role in `otto_app` may be connecting through that grant, so
before revoking it, in the `fly postgres connect` session, list them and grant `CONNECT`
explicitly to each that legitimately uses that database (its service's `DATABASE_URL` names
it), as in R3:

```sql
SELECT datname, datacl FROM pg_database WHERE datname = 'otto_platform';
SELECT rolname FROM pg_roles
WHERE rolcanlogin AND NOT rolsuper AND rolname <> 'cxp_server'
  AND pg_has_role(oid, 'otto_app', 'USAGE') ORDER BY 1;
-- For each that uses otto_platform:  GRANT CONNECT ON DATABASE otto_platform TO <role>;
REVOKE CONNECT ON DATABASE otto_platform FROM otto_app;
```

and tell that database's service, which must not depend on a grant to `otto_app`.

Then repeat R4 from a fresh `psql`.

**R5. Switch the running release to it.** Keep the host, port, and query string from R0's URL
(for an attach, `otto-db.flycast:5432` and `?sslmode=disable`). Read the password into a shell
variable without echoing it, and hand the secret to `fly secrets import` on stdin: `read` and
`printf` are shell builtins, so the password reaches neither the shell history nor the process
list, and nothing is written to disk:

```bash
read -rs PW     # paste the R1 password, then Enter; nothing is shown
printf 'DATABASE_URL=postgres://cxp_server:%s@otto-db.flycast:5432/otto_cxp?sslmode=disable\n' \
  "$PW" | fly secrets import -a otto-cxp
unset PW
```

(Replace the host, port, and query string if R0's URL had others.) Not `--stage`: this restarts the release already running, before #16 merges, so the role switch
is proven apart from the code change (that release runs every database path the same way; the
e2e test `the_service_runs_whole_as_a_least_privilege_database_owner` proves them as such a
role). Then `curl -s https://otto-cxp.savvagent.com/readyz` answers ready, `fly logs -a otto-cxp
--no-tail` shows `tenant isolation enforced as role "otto_app"` (the release running then, before #28), and in `fly postgres connect -a
otto-db`:

```sql
-- cxp_server only (plus your own session)
SELECT usename, count(*) FROM pg_stat_activity WHERE datname = 'otto_cxp' GROUP BY 1;
```

To undo R5, **only before R6**: in `fly postgres connect -a otto-db`, `\password <attach
role>` with a fresh password, then the same `read -rs PW` and `printf ... | fly secrets import`
with that user in the URL. The attach role keeps LOGIN until R7 for this.

**R6. Merge #16.** Its deploy logs
`database role "cxp_server" holds no superuser-equivalent privilege (...), directly or through
membership, and can connect to no other service's database; the instance also hosts
"otto_factory", "otto_flags", "otto_platform"` before `applying migrations` (see
[Verify](#verify)).

Rolling back after R6 means rolling back the code: `fly releases -a otto-cxp --image` lists
the previous image, then `fly deploy -a otto-cxp --image <previous image>`, or revert the PR.
Never undo R5 while a release with the check is deployed: it refuses the attach role, so the
app would stop booting.

**R7. Retire the attach role** once R6 is verified, in `fly postgres connect -a otto-db`. The
guard refuses a mistyped `old_role` (on the shared instance, `otto_flags` or `otto_factory`
would take another service down): a role this setup depends on, one that owns any database,
one with a live session, or one that still owns relations in `otto_cxp`. It changes nothing
when it refuses.

```sql
\c otto_cxp
\set ON_ERROR_STOP on
\set old_role otto_cxp
SELECT set_config('runbook.old_role', :'old_role', false);
DO $$
DECLARE
  old regrole := current_setting('runbook.old_role')::regrole;
  role_name text := (SELECT rolname FROM pg_roles WHERE oid = old);
BEGIN
  IF current_database() <> 'otto_cxp' THEN
    RAISE EXCEPTION 'run this connected to otto_cxp, not %', current_database();
  END IF;
  IF role_name IN ('cxp_server', 'postgres', 'repmgr', 'flypgadmin', 'otto_app')
     OR role_name = current_user OR role_name = session_user THEN
    RAISE EXCEPTION 'old_role must be the retired attach role, not %', role_name;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_database WHERE datdba = old) THEN
    RAISE EXCEPTION '% owns database(s) %: not otto-cxp''s attach role, or R2 did not run', role_name,
      (SELECT string_agg(datname, ', ') FROM pg_database WHERE datdba = old);
  END IF;
  IF EXISTS (SELECT 1 FROM pg_stat_activity WHERE usesysid = old) THEN
    RAISE EXCEPTION '% has % live session(s): something still connects as it', role_name,
      (SELECT count(*) FROM pg_stat_activity WHERE usesysid = old);
  END IF;
  IF EXISTS (SELECT 1 FROM pg_class WHERE relowner = old) THEN
    RAISE EXCEPTION '% still owns relations in otto_cxp: R2 has not run', role_name;
  END IF;
  EXECUTE format('ALTER ROLE %I NOLOGIN NOSUPERUSER', role_name);
END $$;
-- f, f
SELECT rolsuper, rolcanlogin FROM pg_roles WHERE rolname = :'old_role';
```

Do not `fly postgres detach`: it would unset `DATABASE_URL`. Dropping the role later (`\c
otto_cxp`, `DROP OWNED BY <attach role>;` to clear its leftover grants and default privileges
there, then `DROP ROLE <attach role>;`) is optional; `DROP ROLE` refuses while it still holds
anything elsewhere.

### Runbook: split the migration owner from the runtime role (human-only, Rob)

[#27](https://github.com/savvagent/otto-cxp/issues/27). It starts from the state R1 to R7 left:
`cxp_server` owns `otto_cxp` and everything in it and is the role the app connects as. It ends
with `cxp_owner` owning everything, migrating in the release command, and `cxp_server` owning
nothing. Every step is production DDL on the shared `otto-db`, or a production secret; the agent
that wrote this ran the SQL only against a local container put in the same state. Do the steps in
order. **S0 to S4, and S6's staging of the secret, must be done before the PR for #27 merges**:
that release refuses to serve as a role that owns anything or may create temporary tables, and
its release command needs `CXP_MIGRATION_DATABASE_URL`. Until S3, nothing changes for the running
release but a restart; from S3 on it runs as a non-owner, which it can do because S1 turned its
migrations off, and S4 proves it. Paste each SQL block whole; a failure inside `BEGIN ... COMMIT` rolls
the block back.

**S0. Look first (read-only).** In `fly postgres connect -a otto-db`:

```sql
\set ON_ERROR_STOP on
-- 150000 or more: schema public follows the database owner (pg_database_owner).
SHOW server_version_num;
-- cxp_server owns otto_cxp; cxp_owner and cxp_runtime do not exist yet; otto_app does.
SELECT datname, pg_get_userbyid(datdba) AS owner, datacl FROM pg_database WHERE datname = 'otto_cxp';
SELECT rolname, rolcanlogin, rolsuper, rolbypassrls FROM pg_roles
WHERE rolname IN ('cxp_server', 'cxp_owner', 'cxp_runtime', 'otto_app') ORDER BY 1;
\c otto_cxp
-- One row: cxp_server, owner of every table.
SELECT tableowner, count(*) FROM pg_tables WHERE schemaname = 'public' GROUP BY 1;
-- The parameter grant R1 made.
SELECT parname, paracl FROM pg_parameter_acl;
-- Schema public: {pg_database_owner=UC/pg_database_owner,=U/pg_database_owner} on a cluster
-- initialised on 15+. A "=UC" (PUBLIC may CREATE, as on a cluster upgraded from 14 or earlier)
-- is removed in S3.
SELECT nspname, nspacl FROM pg_namespace WHERE nspname = 'public';
```

and the secret names (values are never shown): `fly secrets list -a otto-cxp`.

**S1. Create the roles, and turn migrations off on the running release.** First the roles, in the
`fly postgres connect -a otto-db` session. Generate `cxp_owner`'s password locally and keep it in
your password manager (hex, so it needs no escaping in a URL): `openssl rand -hex 32`.

```sql
\set ON_ERROR_STOP on
-- The migration owner. It logs in only from the release command: CONNECTION LIMIT covers its
-- 2-connection pool and a psql session.
CREATE ROLE cxp_owner LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION
  CONNECTION LIMIT 3;
\password cxp_owner
-- 0003's console_session_org names this parameter in its SET clause; a fresh database migrated
-- by cxp_owner needs the grant (R1 gave it to cxp_server, which no longer migrates).
GRANT SET ON PARAMETER app.console_session_lookup TO cxp_owner;
-- The runtime's grants beyond otto_app (migration 0005 and later grant to this role).
CREATE ROLE cxp_runtime NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION;
GRANT cxp_runtime TO cxp_server;
```

Then one secret, **not staged**: it restarts the running release, which is still the owner and
boots as before, with migrations off from now on (`CXP_RUN_MIGRATIONS is off` in its log).
`CXP_RUN_MIGRATIONS` stays a secret for good (a secret wins over `fly.toml`'s `[env]`): after #27
merges `fly.toml` sets it too, but a rollback to an image from before #27 (whose `fly.toml` does
not) needs it, or that release would try to migrate as a non-owner and fail to boot. The owner's
credential is **not** set here: the running release cannot shed it, so it is staged in S6 and
reaches only the release that does.

```bash
fly secrets set -a otto-cxp CXP_RUN_MIGRATIONS=false
curl -s https://otto-cxp.savvagent.com/readyz      # ready
```

From here until #27 deploys, nothing runs migrations: merge no other PR that adds one before
#27 (its release would ship without migrating).

**S2. Prove `cxp_owner` before giving it anything (read-only).** `fly proxy 15499:5432 -a otto-db`
in a second terminal, then log in to the `postgres` database (it cannot open `otto_cxp` until it
owns it in S3: R3 revoked `PUBLIC`'s `CONNECT` there), with the password from S1:
`psql "postgres://cxp_owner@localhost:15499/postgres"`

```sql
-- cxp_owner, cxp_owner: nothing switched roles on connect
SELECT session_user, current_user;
-- 0 rows: no privilege boot refuses, directly or through membership
SELECT r.rolname FROM pg_roles r
WHERE (r.rolsuper OR r.rolbypassrls OR r.rolcreaterole OR r.rolcreatedb OR r.rolreplication
       OR r.rolname IN ('pg_execute_server_program', 'pg_read_server_files',
                        'pg_write_server_files', 'pg_read_all_data', 'pg_write_all_data',
                        'pg_signal_backend', 'pg_read_all_stats', 'pg_monitor',
                        'pg_create_subscription', 'pg_maintain'))
  AND pg_has_role(current_user, r.oid, 'MEMBER');
-- t for postgres only (otto_cxp follows in S4)
SELECT datname, has_database_privilege(current_user, oid, 'CONNECT') AS may_connect
FROM pg_database WHERE NOT datistemplate ORDER BY datname;
```

**S3. Move ownership to `cxp_owner`.** As postgres, in `otto_cxp`. `REASSIGN OWNED` takes an
exclusive lock on every table, so the block waits at most 5 seconds for each; on `canceling
statement due to lock timeout` nothing changed: paste it again, or run it while the app is idle.

```sql
\c otto_cxp
\set ON_ERROR_STOP on
BEGIN;
SET LOCAL lock_timeout = '5s';
DO $$
BEGIN
  IF current_database() <> 'otto_cxp' THEN
    RAISE EXCEPTION 'run this connected to otto_cxp, not %', current_database();
  END IF;
  -- A fresh setup has no tables yet; otherwise cxp_server, which ran the migrations, owns them.
  IF to_regclass('public.customers') IS NOT NULL
     AND (SELECT relowner FROM pg_class WHERE oid = 'public.customers'::regclass)
         <> 'cxp_server'::regrole THEN
    RAISE EXCEPTION 'cxp_server does not own public.customers: S3 already ran, or R2 did not';
  END IF;
  IF EXISTS (SELECT 1 FROM pg_database WHERE datdba = 'cxp_server'::regrole AND datname <> 'otto_cxp')
     OR EXISTS (SELECT 1 FROM pg_tablespace WHERE spcowner = 'cxp_server'::regrole) THEN
    RAISE EXCEPTION 'cxp_server owns a database or tablespace besides otto_cxp, which REASSIGN OWNED would move too';
  END IF;
  IF NOT pg_has_role('cxp_server', 'cxp_runtime', 'MEMBER') THEN
    RAISE EXCEPTION 'cxp_runtime is not granted to cxp_server: run S1 first';
  END IF;
END $$;
REASSIGN OWNED BY cxp_server TO cxp_owner;
ALTER DATABASE otto_cxp OWNER TO cxp_owner;
-- The transfer rewrote cxp_server's own CONNECT grant (made while it owned the database) to the
-- new owner; without this it could open no new connection.
GRANT CONNECT ON DATABASE otto_cxp TO cxp_server;
-- What the old release does as itself, now as a non-owner: the org.deleted tombstone. Migration
-- 0005 grants the same; this covers the release still running until #27 deploys (and does
-- nothing on a fresh setup, where 0005 will).
DO $$
BEGIN
  IF to_regclass('public.deleted_orgs') IS NOT NULL THEN
    GRANT INSERT ON public.deleted_orgs TO cxp_runtime;
  END IF;
END $$;
-- Default privileges belong to a role and do not move: tables a later migration creates as
-- cxp_owner get otto_app's grants; cxp_server creates nothing any more.
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_owner IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO otto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_owner IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO otto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  REVOKE SELECT, INSERT, UPDATE, DELETE ON TABLES FROM otto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  REVOKE USAGE, SELECT ON SEQUENCES FROM otto_app;
-- Only a migrating role needs it.
REVOKE SET ON PARAMETER app.console_session_lookup FROM cxp_server;
-- No temporary tables: one named like a tenant table would shadow it (unqualified names resolve
-- through pg_temp first) for every later transaction on that pooled connection, with no RLS.
-- cxp_owner keeps TEMPORARY as the owner; nothing in otto-cxp uses temporary tables.
REVOKE TEMPORARY ON DATABASE otto_cxp FROM PUBLIC;
-- No-op on a cluster initialised on 15+; on one upgraded from earlier, PUBLIC could create in
-- public and plant an overload a SECURITY DEFINER function would resolve.
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
COMMIT;
```

The `console_sessions_lookup` policy needs nothing: it admits whichever role owns
`console_session_org`, now `cxp_owner`, and the function runs as that owner.

**S4. Prove both roles (read-only).** As postgres in `otto_cxp`:

```sql
-- 0 rows: cxp_server owns nothing, directly or through a role it can SET ROLE to.
SELECT 'relation' AS kind, c.relname::text AS name FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%'
  AND pg_has_role('cxp_server', c.relowner, 'MEMBER')
UNION ALL
SELECT 'function', p.proname::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') AND pg_has_role('cxp_server', p.proowner, 'MEMBER')
UNION ALL
SELECT 'schema', n.nspname::text FROM pg_namespace n
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_t%'
  AND pg_has_role('cxp_server', n.nspowner, 'MEMBER')
UNION ALL
SELECT 'database', d.datname::text FROM pg_database d
WHERE pg_has_role('cxp_server', d.datdba, 'MEMBER')
UNION ALL
SELECT 'extension', e.extname::text FROM pg_extension e
WHERE e.extname <> 'plpgsql' AND pg_has_role('cxp_server', e.extowner, 'MEMBER');
-- 0 rows: no table it, or a role it can SET ROLE to, may TRUNCATE or add a trigger to.
SELECT c.relname, r.rolname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
CROSS JOIN pg_roles r
WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
  AND pg_has_role('cxp_server', r.oid, 'MEMBER')
  AND (has_table_privilege(r.oid, c.oid, 'TRUNCATE')
       OR has_table_privilege(r.oid, c.oid, 'TRIGGER'));
-- 0 rows: nowhere it, or a role it can SET ROLE to, may create a schema, an object, or a
-- temporary table (PUBLIC counts).
SELECT r.rolname, 'database' AS on_, has_database_privilege(r.oid, 'otto_cxp', 'CREATE') AS create_,
       has_database_privilege(r.oid, 'otto_cxp', 'TEMPORARY') AS temporary_
FROM pg_roles r WHERE pg_has_role('cxp_server', r.oid, 'MEMBER')
  AND (has_database_privilege(r.oid, 'otto_cxp', 'CREATE')
       OR has_database_privilege(r.oid, 'otto_cxp', 'TEMPORARY'))
UNION ALL
SELECT r.rolname, 'schema ' || n.nspname, true, false
FROM pg_roles r CROSS JOIN pg_namespace n
WHERE pg_has_role('cxp_server', r.oid, 'MEMBER')
  AND n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_t%'
  AND has_schema_privilege(r.oid, n.oid, 'CREATE');
-- t, t, t, t, t: it connects, inherits otto_app, writes the tombstone, runs the console lookup,
-- and runs the sweep's tombstone expiry.
SELECT has_database_privilege('cxp_server', 'otto_cxp', 'CONNECT') AS connects,
       pg_has_role('cxp_server', 'otto_app', 'USAGE') AS assumes_otto_app,
       has_table_privilege('cxp_server', 'public.deleted_orgs', 'INSERT') AS tombstone,
       has_function_privilege('cxp_server', 'public.console_session_org(bytea)', 'EXECUTE') AS lookup,
       has_function_privilege('cxp_server', 'public.expire_removed_members()', 'EXECUTE') AS sweep;
-- NULL, with no error: the console's cookie lookup as the runtime role, running as its new owner
-- (it needs cxp_owner's SET grant from S1). Read-only.
SET ROLE cxp_server;
SELECT console_session_org(decode(repeat('00', 32), 'hex'));
RESET ROLE;
-- cxp_owner for each: it owns the database, the tables, and the console lookup.
SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = 'otto_cxp';
SELECT DISTINCT tableowner FROM pg_tables WHERE schemaname = 'public';
SELECT pg_get_userbyid(proowner) FROM pg_proc
WHERE oid = 'public.console_session_org(bytea)'::regprocedure;
```

Then, through the proxy, `psql "postgres://cxp_owner@localhost:15499/otto_cxp"` and repeat S2's
queries (now `t` for `otto_cxp` and `postgres` only), plus `SELECT has_schema_privilege('public',
'CREATE');` (`t`: it can migrate). And prove the running release boots as a non-owner:

```bash
fly apps restart otto-cxp
curl -s https://otto-cxp.savvagent.com/readyz      # ready
fly logs -a otto-cxp --no-tail                     # CXP_RUN_MIGRATIONS is off; tenant isolation enforced as role "otto_app" (the release running then, before #28)
```

**S5. Undo, only before S6.** S3 alone, back to `cxp_server` owning everything (as postgres; the
`\c` matters: run from the `postgres` database, `REASSIGN OWNED` would move only the database and
leave the tables with `cxp_owner`). The `TEMPORARY` and `CREATE` revokes stay: the old release
needs neither. `CXP_RUN_MIGRATIONS=false` stays too; unset it (`fly secrets unset -a otto-cxp
CXP_RUN_MIGRATIONS`) only if the old release should migrate at boot again.

```sql
\c otto_cxp
\set ON_ERROR_STOP on
BEGIN;
SET LOCAL lock_timeout = '5s';
DO $$
BEGIN
  IF current_database() <> 'otto_cxp' THEN
    RAISE EXCEPTION 'run this connected to otto_cxp, not %', current_database();
  END IF;
  IF EXISTS (SELECT 1 FROM pg_database WHERE datdba = 'cxp_owner'::regrole AND datname <> 'otto_cxp') THEN
    RAISE EXCEPTION 'cxp_owner owns another database, which REASSIGN OWNED would move too';
  END IF;
  -- Migration 0005 runs only in #27's release command: once it has, S6 happened, and the new
  -- release refuses an owning runtime role.
  IF EXISTS (SELECT 1 FROM public._sqlx_migrations WHERE version >= 5) THEN
    RAISE EXCEPTION '#27 is deployed (migration 0005 applied): roll the code back first, never S3';
  END IF;
END $$;
REASSIGN OWNED BY cxp_owner TO cxp_server;
ALTER DATABASE otto_cxp OWNER TO cxp_server;
GRANT CONNECT ON DATABASE otto_cxp TO cxp_server;
GRANT SET ON PARAMETER app.console_session_lookup TO cxp_server;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO otto_app;
ALTER DEFAULT PRIVILEGES FOR ROLE cxp_server IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO otto_app;
COMMIT;
```

**S6. Stage the owner's credential, then merge the PR for #27.** Staged, so the running release
(which cannot shed it) never receives it; it applies with the next deploy, to the release command
and to the new release, which re-executes itself without it:

```bash
read -rs PW     # paste cxp_owner's password, then Enter; nothing is shown
printf 'CXP_MIGRATION_DATABASE_URL=postgres://cxp_owner:%s@otto-db.flycast:5432/otto_cxp?sslmode=disable\n' \
  "$PW" | fly secrets import -a otto-cxp --stage
unset PW
fly secrets list -a otto-cxp        # CXP_MIGRATION_DATABASE_URL listed as staged
```

(Same host, port, and query string as `DATABASE_URL`; R0 noted them. `read` and `printf` are shell
builtins: the password reaches neither the history nor the process list.) Then merge; with
`FLY_DEPLOY` enabled the merge deploys, otherwise `fly deploy -a otto-cxp --remote-only`. Fly
first runs the release command, `cxp-server migrate`, in a temporary machine as `cxp_owner`; it
applies 0005 and 0006 (0006 recreates `console_session_org`, whose `SET` clause needs the
parameter grant S1 gave `cxp_owner`); a failure there aborts the deploy and leaves the old release
serving. Then see [Verify](#verify): the release command's log shows `database role "cxp_owner"
holds no superuser-equivalent privilege ...`, `applying migrations`, `migrations applied`; each
machine's shows `removed CXP_MIGRATION_DATABASE_URL from the serving process's environment ...`,
`database role "cxp_server" ... it owns nothing in this database, can TRUNCATE or add a TRIGGER
to no table, and can create no schema, object, or temporary table in it ...`, `not migrating`,
`tenant isolation enforced as role "otto_app"` (releases before #28; from #28 on, `"cxp_app"`).

Rolling back after S6 means rolling back the code, and the release command is the catch: an image
from before #27 does not know `migrate` and would start serving inside the release machine until
Fly's release-command timeout (5 minutes by default) aborts the deploy. Revert the PR (the reverted `fly.toml` has no release command), or deploy the
previous image with the previous commit's `fly.toml` (`git show <previous commit>:fly.toml >
/tmp/fly.toml && fly deploy -a otto-cxp --image <previous image> -c /tmp/fly.toml`). Either way
the `CXP_RUN_MIGRATIONS` secret keeps the old binary from migrating, and S4 proved it serves as
a non-owner. Never undo S3 after S6: the new release refuses an owning runtime role.

**S7. Nothing to retire.** `cxp_server` keeps its password and `DATABASE_URL`. `cxp_owner`'s
password lives only in `CXP_MIGRATION_DATABASE_URL` and your password manager; rotate it with
`\password cxp_owner` and the same `read -rs PW ... | fly secrets import -a otto-cxp` (unstaged
is fine once #27 is deployed: the serving release sheds it on restart).

### Runbook: give otto-cxp its own tenant role (human-only, Rob)

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
-- What cxp_server is a member of: otto_app and cxp_runtime (and who granted each: T5 needs it).
SELECT r.rolname, m.grantor::regrole FROM pg_auth_members m JOIN pg_roles r ON r.oid = m.roleid
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
-- 0 rows: it owns nothing in otto_cxp and can create nothing there (the split runbook's S3
-- revoked PUBLIC's TEMPORARY and CREATE).
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
  AND has_function_privilege('cxp_app', f.oid, 'EXECUTE') <> has_function_privilege('otto_app', f.oid, 'EXECUTE')
UNION ALL
SELECT 'schema public', p.priv FROM unnest(ARRAY['USAGE','CREATE']) AS p(priv)
WHERE has_schema_privilege('cxp_app', 'public', p.priv) <> has_schema_privilege('otto_app', 'public', p.priv);
-- r: cxp_app=arwd/cxp_owner; S: cxp_app=rU/cxp_owner (tables and sequences a later
-- migration creates as cxp_owner reach cxp_app).
SELECT d.defaclobjtype, d.defaclacl FROM pg_default_acl d
JOIN pg_namespace n ON n.oid = d.defaclnamespace
WHERE d.defaclrole = 'cxp_owner'::regrole AND n.nspname = 'public';
-- f, t: a tenant transaction may not write a tombstone; cxp_server may (through cxp_runtime).
SELECT has_table_privilege('cxp_app', 'public.deleted_orgs', 'INSERT') AS tenant_tombstone,
       has_table_privilege('cxp_server', 'public.deleted_orgs', 'INSERT') AS runtime_tombstone;
```

If the parity query returns rows (production's `otto_app` grants can differ from a fresh
migration's), stop here: do not run T5, keep `cxp_server` in `otto_app`, and report the rows. The
release serves as `cxp_app`, but requests that need a privilege it lacks fail; re-run the
missing GRANT as `cxp_owner` (in `otto_cxp`) and report the difference.

**T5. Leave `otto_app`.** Once T3 and T4 pass, as postgres:

```sql
REVOKE otto_app FROM cxp_server;
-- f
SELECT pg_has_role('cxp_server', 'otto_app', 'MEMBER');
-- If that still says t (Postgres 16 revokes only the grants the revoking role made), revoke the
-- grant by its grantor, from T0's second query:
-- REVOKE otto_app FROM cxp_server GRANTED BY <grantor>;
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

## Search index storage (#34)

Customer and knowledge-base search keep their own index tables, `customer_search_grams` and
`knowledge_search_terms` (migration 0004, design in
`docs/specs/2026-10-14-search-under-rls-design.md`). They cost about 7 kB per customer and 25 kB
per published article when bulk-loaded, and 25 to 30% more when grown a row at a time.
`otto-db` has a single **1 GB** volume
(`fly volumes list -a otto-db`), shared with `otto_platform`, `otto_factory`, and `otto_flags`.

### When to act

Act when **any** of these holds:

- the two index tables together pass **100 MB**, a tenth of the volume. That is roughly 10k
  customers or 3k published articles in all orgs combined, and fewer once grown a row at a time;
- `otto_cxp` passes **250 MB**;
- the volume passes **60%** used (`fly ssh console -a otto-db -C "df -h /data"`, read-only),
  leaving headroom for WAL spikes on the shared volume and for a rebuild (below).

What to do, in order of preference:

1. **Move otto-cxp to a dedicated Postgres instance.** This adds recurring cost, so it is Rob's
   decision. This doc already says to split before real customers arrive.
2. **Slim the index** with the LEAKPROOF route (below), which lets a later migration serve fuzzy
   name search from a GIN index and drop the trigram rows: about a quarter of
   `customer_search_grams`. Dropping the article lexeme and title rows as well would need `@@`
   and `ILIKE` leakproof too, which this route does not cover.

Growing the volume is a stopgap, not a fix. It also adds recurring cost (Rob).

### Watch query (SELECT-only)

Run it as the superuser in a read-only transaction. It reads statistics and sizes and writes
nothing. It must be the superuser: as `cxp_server` (bound by forced RLS) the two "missing"
counts silently read 0, and `pg_database_size` fails on the other services' databases.

```sh
fly postgres connect -a otto-db -d otto_cxp
```

```sql
BEGIN TRANSACTION READ ONLY;
SELECT s.relname, s.n_live_tup AS live_rows,
       pg_size_pretty(pg_total_relation_size(s.relid)) AS total,
       pg_size_pretty(pg_relation_size(s.relid)) AS heap,
       pg_size_pretty(pg_indexes_size(s.relid)) AS indexes
FROM pg_stat_user_tables s
WHERE s.relname IN ('customer_search_grams', 'knowledge_search_terms',
                    'customers', 'knowledge_articles')
ORDER BY pg_total_relation_size(s.relid) DESC;
SELECT i.relname, i.indexrelname, i.idx_scan,
       pg_size_pretty(pg_relation_size(i.indexrelid)) AS size
FROM pg_stat_user_indexes i
WHERE i.relname IN ('customer_search_grams', 'knowledge_search_terms')
   OR i.indexrelname LIKE '%search_backlog%'  -- 0004's partial indexes; unused since #34, kept (additive)
ORDER BY pg_relation_size(i.indexrelid) DESC;
SELECT pg_size_pretty(sum(pg_total_relation_size(relid))) AS search_index_total
FROM pg_stat_user_tables
WHERE relname IN ('customer_search_grams', 'knowledge_search_terms');
SELECT datname, pg_size_pretty(pg_database_size(datname)) AS size
FROM pg_database WHERE NOT datistemplate ORDER BY pg_database_size(datname) DESC;
SELECT count(*) AS customers_missing_from_index FROM customers c
WHERE NOT EXISTS (SELECT 1 FROM customer_search_grams g
                  WHERE g.org_id = c.org_id AND g.customer_id = c.id);
SELECT count(*) AS articles_missing_from_index FROM knowledge_articles a
WHERE a.published AND (a.title <> '' OR a.search_tsv <> ''::tsvector)
  AND NOT EXISTS (SELECT 1 FROM knowledge_search_terms t
                  WHERE t.org_id = a.org_id AND t.article_id = a.id);
ROLLBACK;
```

The baseline on 2026-10-15: 0 customers and 0 articles in every org, the index tables 56 kB in
all, `otto_cxp` 9.5 MB, and 0 rows missing from the index. `n_live_tup` is an estimate; the sizes are exact.

### No backlog

Every row is in the index. Migration 0004's `BEFORE INSERT OR UPDATE` trigger
(`search_mark_indexed`) sets `search_indexed` on every write, and the sync triggers write the
index rows. Only rows that existed when 0004 ran could read `false`, and production had none.
Since #34, search no longer reads or writes `search_indexed`: a row is found through the index
only if its index rows exist (a customer without them can still turn up through the newest-first
scan of a broad query, but not reliably). The column and its two partial indexes stay only
because migrations are additive.

The way to lose index rows is loading data with triggers disabled: a restore with
`--disable-triggers` or `session_replication_role = replica`, or a data-only `COPY` of the base
tables without the index tables. Such rows usually still read `search_indexed = true` (the
column's default, or the dumped value), so the flag does not show them; the last two statements
of the watch query find them by their missing index rows. If either count is nonzero, rebuild.
This is a human-only production write (Rob), run as the superuser in `otto_cxp`. Step 1 marks
the rows unindexed with triggers off (the `BEFORE` trigger would otherwise force the flag back
to `true`); step 2, with triggers on, sets it again, which fires the sync triggers (their `WHEN`
includes `NOT OLD.search_indexed`) to write the index rows.

Before you start: both steps rewrite every affected row (not HOT: the flag is in the partial
indexes' predicate), and step 2 writes about 50 index rows per customer and 160 per article, all
as WAL. Check free space on the volume first: at least three times the expected index size
(about 9 kB per customer and 30 kB per published article). For more than a few thousand rows,
run step 2 one org at a time (add `AND org_id = '<org>'` to both statements) with a `VACUUM` of
the two base tables between orgs, rather than as one statement.

```sql
BEGIN;
SET LOCAL session_replication_role = replica;
UPDATE customers c SET search_indexed = false
WHERE NOT EXISTS (SELECT 1 FROM customer_search_grams g
                  WHERE g.org_id = c.org_id AND g.customer_id = c.id);
UPDATE knowledge_articles a SET search_indexed = false
WHERE a.published AND (a.title <> '' OR a.search_tsv <> ''::tsvector)
  AND NOT EXISTS (SELECT 1 FROM knowledge_search_terms t
                  WHERE t.org_id = a.org_id AND t.article_id = a.id);
COMMIT;
UPDATE customers SET search_indexed = true WHERE NOT search_indexed;
UPDATE knowledge_articles SET search_indexed = true WHERE NOT search_indexed;
```

Then re-run the watch query; both counts read 0. (The article count leaves out published articles
with an empty title and only stop words in their content: correctly indexed, they have no index
rows.)

### Search timeouts

Each search runs under `statement_timeout = 2s`. A search past it answers `query_too_broad`,
which is not retriable (REST 422 with no `Retry-After`; MCP `INVALID_PARAMS` with `data.code`
and `retriable: false`): repeating a broad query would hold one of the five pooled connections
for the whole timeout again, so the caller is told to narrow it. Expect it rarely: from a broad
query in a very large org, or occasionally from a search that reads a cold cache (after
`otto-db` restarts, or after a long idle stretch on the shared instance). A cold-cache timeout is
acceptable: the caller's next, narrower search runs warm. The service logs each one at `warn` as
`search hit its statement_timeout`, naming the search but never the query (an operator's
`pg_cancel_backend` on a search logs the same way). If these appear often, or an org nears the
threshold above, time one cold search from the console or an agent.

### LEAKPROOF re-evaluation (human-only, Rob)

Under forced row-level security, Postgres uses an index only through leakproof operators, and
pg_trgm's `%` (`similarity_op`) is not marked leakproof. Marking it LEAKPROOF in `otto_cxp`
would let a GIN `(name gin_trgm_ops)` index serve fuzzy search directly. That makes long fuzzy
queries exact in large orgs, and lets the trigram rows go. Reasoning:
`docs/specs/2026-10-14-search-under-rls-design.md` ("Human hand-off").

**Re-evaluate when** the index passes the threshold above, or users report fuzzy name search
missing customers (results with `totalCapped: true` on long name queries in a large org).

**The change** is made by a superuser, connected to `otto_cxp` only:

```sql
ALTER FUNCTION public.similarity_op(text, text) LEAKPROOF;
```

It must be followed by a migration that adds the GIN index and a release whose search uses it.

**What loses the flag:** `ALTER EXTENSION pg_trgm UPDATE`, dropping and recreating the extension,
and a dump and restore. Each can recreate the function without the flag (an extension update
does when its script replaces `similarity_op`). RLS then silently stops
using the GIN index, and every fuzzy search scans the org again.

**The boot check that release must add:** at startup, as `boot::check_db_role` does for the
role, run

```sql
SELECT proleakproof FROM pg_proc WHERE oid = 'public.similarity_op(text,text)'::regprocedure;
```

and refuse to boot if it is false while the GIN path is compiled in. A lost flag then fails the
deploy loudly instead of degrading search to scans.

**Setting the flag changes query planning at once**, whether or not that release exists: the
planner may then evaluate `name % $3` (today's customer search uses it) on other orgs' rows
before the RLS policy filters them. So before Rob marks it, check on the installed pg_trgm
version that `similarity_op` and the trigram functions it calls cannot raise an error that
quotes their input, since its return value must be its only signal. Set it in the same change
window as the GIN release, not earlier.

## First deploy

Every step below is human-only (Rob): each one creates infrastructure, writes to the shared
database or the platform, or sets a repo credential. The agent that wrote this repo has run
none of them. Do them in order. The first deploy is a hand `fly deploy`; the CI deploy job
stays off (`FLY_DEPLOY` unset) until that deploy is verified.

```bash
# 1. App, then its hostname first so the certificate issues while you do the rest.
fly apps create otto-cxp --org savvagent
# A new app has no IPs until its first deploy; allocate them now so DNS can point at
# them. The IPv4 is the free shared one; a dedicated IPv4 is a recurring cost.
fly ips allocate-v6 -a otto-cxp
fly ips allocate-v4 --shared -a otto-cxp
fly certs add otto-cxp.savvagent.com -a otto-cxp
# DNS (Namecheap): A and AAAA records for otto-cxp -> the IPs `fly ips list -a otto-cxp` shows

# 2. Database. Attach creates database otto_cxp and sets DATABASE_URL to a superuser,
#    which a deployment refuses: run "Database role" below before step 5 (on a fresh
#    setup its step R5 imports the secret with --stage, and R6 and R7 follow the first deploy),
#    then the split runbook's S1 to S4 and S6's staged secret (with nothing deployed yet,
#    S1's secret restarts nothing, and S4's checks of tables and console_session_org wait
#    until after the first deploy, as does its restart). Then the tenant-role runbook's T1
#    (cxp_app), before the first release command runs migration 0007.
#    The first deploy's release command then migrates as cxp_owner.
fly postgres attach otto-db -a otto-cxp --database-name otto_cxp

# 3. Register the resource server at the platform (run on the platform's machine).
fly ssh console -a otto-platform -C 'otto-platform-server resource register \
  https://otto-cxp.savvagent.com/mcp --name otto-cxp \
  --scopes cxp:read,cxp:write --default-scopes cxp:read'
fly ssh console -a otto-platform -C 'otto-platform-server resource rotate-secret \
  https://otto-cxp.savvagent.com/mcp'                 # prints otto_rs_... once
fly ssh console -a otto-platform -C 'otto-platform-server resource set-webhook \
  https://otto-cxp.savvagent.com/mcp https://otto-cxp.savvagent.com/platform/webhooks'
                                                      # prints otto_whsec_... once

# 4. Secrets (staged: they apply with the first deploy).
fly secrets set -a otto-cxp --stage \
  CXP_INTROSPECTION_SECRET='otto_rs_...' CXP_PLATFORM_WEBHOOK_SECRET='otto_whsec_...'

# 5. First deploy, by hand (the CI deploy job is still off).
fly deploy -a otto-cxp --remote-only
```

Run [Verify](#verify). Only when it passes, enable CI deploys:

```bash
# 6. A deploy token scoped to this one app, then the repo credentials.
fly tokens create deploy -a otto-cxp                  # prints the token
gh secret set FLY_API_TOKEN --repo savvagent/otto-cxp # paste the token
gh variable set FLY_DEPLOY --body enabled --repo savvagent/otto-cxp
```

Until `FLY_DEPLOY` is `enabled`, merging to `main` deploys nothing. Then merge
`savvagent/otto-platform` PR #39 (the marketing listing for otto-cxp): it deploys the platform
site, so it goes last, once the service is live.

The scope list is `cxp_core::scopes::KNOWN` and the defaults `cxp_core::scopes::DEFAULT`;
re-run the `resource register` command when a release adds a scope. `CXP_RESOURCE_URI` must be
exactly the registered URI: it is the token audience and the HTTP Basic user on every platform
call. `CXP_ALLOWED_HOSTS` lists EXTRA accepted Hosts (here the `fly.dev` name); the host of
`CXP_PUBLIC_URL` is always accepted.

## Web console sign-in

The console (`apps/web`, built into the image at `/srv/console`, the Dockerfile's
`CXP_CONSOLE_DIR`, and served under `/console/`; `/` is not the console's) signs people in with
an OAuth authorization-code + PKCE flow against the platform, then keeps its own session cookie
(`__Host-cxp_session`) over a platform token pair held server-side in `console_sessions`
(tokens sealed with a subkey derived from `CXP_ENCRYPTION_KEY`, labels
`cxp.console.session-tokens.v1` and, for the sign-in cookie, `cxp.console.oauth-flow.v1`; the
cookie is stored only as its SHA-256). Design: `docs/specs/2026-10-14-console-sign-in-design.md`.
Turning it on in production is tracked in #31.

It is **off until `CXP_CONSOLE_CLIENT_ID` is set**: the server boots and serves everything else,
`/auth/login` answers `503 console_login_disabled`, and the console shows that sign-in is not
configured. Bearer tokens and `/mcp` do not depend on it. Turning it on is human-only (Rob), in
this order, because a client id without the key fails the boot (naming the key):

```bash
# 1. Register the console as a public, first-party client (no secret, no consent screen)
#    whose only redirect URI is {CXP_PUBLIC_URL}/auth/callback. Run on the platform's machine.
fly ssh console -a otto-platform -C 'otto-platform-server client register \
  --name "otto-cxp console" \
  --redirect-uri https://otto-cxp.savvagent.com/auth/callback --first-party'
#    -> prints the client_id

# 2. The encryption key, staged so it applies with the next release.
fly secrets set -a otto-cxp --stage CXP_ENCRYPTION_KEY="$(openssl rand -base64 32)"

# 3. Set the client id in fly.toml [env] (it is not a secret: a public client has nothing else
#    to authenticate with, and PKCE binds each code to the browser that asked for it):
#      CXP_CONSOLE_CLIENT_ID = "<client_id from step 1>"
#    and merge that commit (a PR referencing #31); the deploy turns sign-in on. Step 2 must be
#    staged before this deploys, or the boot refuses, naming CXP_ENCRYPTION_KEY.
```

The redirect URI is registered **exactly**: a deployment that moves to another hostname registers
a new client.

**Step 4: verify live, after the deploy.**

```bash
# Sign-in is on: "loginEnabled": true ("signedIn": false without a cookie).
curl -s https://otto-cxp.savvagent.com/auth/session
```

- Open `https://otto-cxp.savvagent.com/console/`, sign in at the platform, and check that the
  customers list loads; `GET /auth/session` from that browser answers `"signedIn": true`.
- A cross-site write is refused. Copy the `__Host-cxp_session` cookie's value from the browser's
  dev tools and read it into the shell without echoing it or leaving it in history:

  ```bash
  read -rs CXP_COOKIE
  curl -s -i -X POST https://otto-cxp.savvagent.com/api/v1/customers \
    -H "cookie: __Host-cxp_session=$CXP_COOKIE" -H 'origin: https://example.com' \
    -H 'content-type: application/json' -d '{}'
  # -> 403 with "code": "cross_site_request". The empty body is invalid, so even a broken
  #    origin check writes nothing (it would answer a 4xx validation error instead).
  ```

- Sign out in the console (do not skip this: it is what ends the session behind the copied
  cookie). `GET /auth/session` from that browser answers `"signedIn": false`, and the copied
  cookie no longer works:

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -H "cookie: __Host-cxp_session=$CXP_COOKIE" \
    https://otto-cxp.savvagent.com/api/v1/customers   # -> 401
  unset CXP_COOKIE
  ```

  The row is deleted, and its refresh token revoked at the platform (best effort: a platform
  outage at that moment leaves the token to expire on its own).

**Rotating `CXP_ENCRYPTION_KEY` signs every console user out** (sessions that no longer open are
deleted on their next use). The platform rotates a refresh token on every use and revokes the
whole sign-in if a spent one is replayed, so each refresh happens once, across machines: a
machine coalesces its own requests in memory, claims a short lease on the session row (45 s,
on the database's clock), calls the platform holding no database connection or lock only if the
call and the store still fit in the lease, and stores the new pair only if the row still holds
that lease (a compare-and-swap, its waits cut off in the database: 2 s for a lock, 3 s per
statement); another machine that finds the lease waits for the result. A sign-out or
`org.deleted` during that call wins, and the new pair is revoked; so is a pair whose store fails
or times out (that person signs in again). Sessions are deleted on sign-out, by the
`org.deleted` and `member.removed` webhooks, and by the hourly sweep once expired (30 days
without a refresh).

## Verify

```bash
curl -s https://otto-cxp.savvagent.com/healthz
curl -s https://otto-cxp.savvagent.com/readyz
curl -s https://otto-cxp.savvagent.com/.well-known/oauth-protected-resource
fly logs -a otto-cxp --no-tail
```

The logs must show, in order:

- from the release command's machine: `database role "cxp_owner" holds no superuser-equivalent
  privilege (...) ... and can connect to no other service's database; it owns database
  "otto_cxp", ...`, then `applying migrations`, then `migrations applied`;
- from each serving machine: `removed CXP_MIGRATION_DATABASE_URL from the serving process's
  environment; only cxp-server migrate uses it`, then `database role "cxp_server" holds no superuser-equivalent
  privilege (...) ... and can connect to no other service's database; it owns nothing in this
  database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or
  temporary table in it; the instance also hosts ...`
- `not migrating: migrations run as the database owner through cxp-server migrate`
- `tenant isolation enforced as role "cxp_app" (assumed via SET LOCAL ROLE cxp_app)`
- `the otto platform accepted this service's credential`

If the platform REJECTED the credential, or `CXP_PLATFORM_URL` is not the platform, `/readyz`
answers 503 with `"reason": "platform credential rejected"` (or `"platform misconfigured"`), so a
deploy with a bad secret fails its health check. The check runs once at boot: after fixing the
secret, URL, or registration, restart the machines (`fly secrets set` already does). Readiness
reflects only that boot check, so a credential the platform starts refusing after boot shows as
500s on authenticated requests and `error` logs (`the platform rejected this resource server's
credential`), not as a failing `/readyz`.

Then point an MCP client at `https://otto-cxp.savvagent.com/mcp`: it is sent to the platform to
sign in, and `whoami` answers with the platform's records. `curl -s -H 'accept: text/html'
https://otto-cxp.savvagent.com/console/` returns the console's `index.html` (`/` serves
browsers the connector install page, #29), and
`curl -s https://otto-cxp.savvagent.com/auth/session` answers `loginEnabled` true once
`CXP_CONSOLE_CLIENT_ID` is set (false before).

Usage backlog (should drain to 0 within seconds of activity). Connect with
`fly postgres connect -a otto-db -d otto_cxp` and run SELECTs only; production diagnostics
never write:

```sql
SELECT count(*), max(attempts), max(row_failures), max(last_error) FROM usage_outbox;
SELECT reason, count(*) FROM usage_outbox_rejected GROUP BY reason;
```

A row the platform refuses in its receipt moves to `usage_outbox_rejected` with its reason and
is logged at `error`. A batch it refuses whole is resent in halves until the refused events
stand alone; each refusal of an event sent alone, like each receipt that did not account for a
row, counts against that row (`row_failures`), and after 20 of them it moves to
`usage_outbox_rejected` too. Rejected rows are kept 90 days. If every event sent alone is refused
and nothing is accepted, the platform is refusing the requests themselves (a contract change, a
gateway in the way): the shipper logs `the platform refused every event in a batch; not counting
it against the rows` at `error` and keeps the whole batch, uncounted, until that is fixed.
Splitting stops after half the claim lease (60 s) or 64 requests in one pass; the pieces not
yet sent wait for the next pass, uncounted. Anything else (the platform down, rate limiting,
the credential refused) never gives a row up: it stays in `usage_outbox`, retried with backoff
up to every 15 minutes and still counted toward its period's quota, until it ships. A final
flush on shutdown that cannot reach the platform is logged at `warn` and does not fail the
exit; the next start ships those rows.

## Deploys

Every push to `main` that passes CI deploys (`.github/workflows/ci.yml`, job `deploy`, which
needs `rust`, `docker-build`, and `web`). The job runs only while the repo variable
`FLY_DEPLOY` is `enabled`, which Rob sets after the hand-run first deploy is verified; until
then a merge deploys nothing. `FLY_API_TOKEN` is the deploy token from step 6. A hand deploy is `fly deploy -a otto-cxp --remote-only`.

## Migrations

Migrations are append-only: a schema change is a new `crates/cxp-core/migrations/NNNN_<name>.sql`,
never an edit, rename, or delete of an existing one. sqlx refuses to boot when an applied
migration's checksum changed, and CI's `migrations-append-only` job rejects such a PR.
`MIGRATOR` sets `ignore_missing`, so the previous release boots against a newer schema (a
rollback of the image works), which means every migration must be additive. They run in the
release command as `cxp_owner` (see [Infrastructure](#infrastructure)); confirm with the
`applying migrations` and `migrations applied` lines in the release command's log. A migration
that gives the runtime a privilege beyond the tenant role `cxp_app`'s grants it to `cxp_runtime`, guarded by
`IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'cxp_runtime')` as 0005 does.

## Local seeding (human-only against otto-db)

`scripts/seed-demo.sql` loads demo data for one org:

```bash
psql "$DATABASE_URL" -v org=<org-uuid> -f scripts/seed-demo.sql
```

With the local container (`podman compose up -d`, port 15435) this is safe and idempotent per
org. Pointed at `otto-db` it is a production write: run it only deliberately, with a real org
uuid from the platform, and only by hand (Rob).
