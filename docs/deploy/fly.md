# Hosting: Fly.io

otto-cxp runs as one Fly app, `otto-cxp` (org `savvagent`, region `iad`), serving every surface
on one port: the MCP endpoint agents use, the REST API apps use, and the platform's webhooks.
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
  schema uses only the stock `pg_trgm`). otto-cxp connects as `cxp_server`, a dedicated role
  that owns `otto_cxp` and nothing else: not a superuser, not BYPASSRLS, no CREATEDB or
  CREATEROLE, a member of `otto_app`, and the only role besides superusers that may connect to
  `otto_cxp` (see [Database role](#database-role)). Tenant transactions run as `otto_app`
  under row-level security, which `cxp-server` proves at boot
  (`tenant isolation enforced as role "otto_app"`) and refuses to start without; everything
  else (migrations, the sweep and usage shipper, tombstones, `/readyz`) runs as `cxp_server`,
  which FORCE ROW LEVEL SECURITY binds too, as long as it does not alter or truncate its own
  tables. A deployment refuses to serve when the connecting role is, or can `SET ROLE` to, a
  superuser-equivalent role, or can connect to another service's database
  ([#16](https://github.com/savvagent/otto-cxp/issues/16)), so the role `fly postgres attach`
  creates, a superuser on the whole instance, is refused.
  Customer and knowledge-base search run under RLS, whose per-row policy checks keep the
  planner off some index paths; fine at today's volumes, slow at scale. Making them fast is
  tracked in [#17](https://github.com/savvagent/otto-cxp/issues/17). The pool is
  capped at 5 connections (`CXP_DB_MAX_CONNECTIONS`) because the instance is small and shared.
  Split onto a dedicated instance before real customers.
- **Migrations** are otto-cxp's own (`crates/cxp-core/migrations`) and run at boot under an
  advisory lock. Never run the platform's migrations here.

## Secrets

| Secret | What | Where from |
|---|---|---|
| `DATABASE_URL` | Postgres URL for `otto_cxp`, as `cxp_server` | [Database role](#database-role) (replaces the one `fly postgres attach` set) |
| `CXP_INTROSPECTION_SECRET` | `otto_rs_...`, authenticates every call to the platform | `resource rotate-secret` |
| `CXP_PLATFORM_WEBHOOK_SECRET` | `otto_whsec_...`, verifies platform webhooks | `resource set-webhook` |

Everything else is in `fly.toml`'s `[env]`. A new secret needs `fly secrets set -a otto-cxp
--stage NAME=...` before the release that reads it, plus a row here and in `.env.example`.

## Database role

otto-cxp connects to `otto-db` as `cxp_server`: LOGIN, NOSUPERUSER, NOBYPASSRLS, NOCREATEDB,
NOCREATEROLE, NOREPLICATION, CONNECTION LIMIT 15, a member of `otto_app` (so tenant
transactions can `SET LOCAL ROLE otto_app`), and the owner of database `otto_cxp` and every
object in it. Owning the database
lets it run otto-cxp's migrations (`pg_trgm` is a trusted extension a database owner may
create). FORCE ROW LEVEL SECURITY binds it as the tables' owner, so its background work reaches
other orgs' rows only through the maintenance policies, but only while it leaves its own tables
alone: an owner can `ALTER TABLE ... NO FORCE ROW LEVEL SECURITY`, `DISABLE ROW LEVEL
SECURITY`, or `DROP POLICY`, and `TRUNCATE` ignores RLS altogether, so code running as
`cxp_server` could switch RLS off on, or empty, `otto_cxp`'s tables (and nothing beyond them).
Splitting the migration owner from the runtime role closes that:
[#27](https://github.com/savvagent/otto-cxp/issues/27). `CONNECT` on `otto_cxp`,
`otto_platform`, `otto_factory`, `otto_flags`, and `repmgr` is revoked from `PUBLIC`, so
`cxp_server` can open no other service's database.

`otto_app` is shared by every otto service, which is one more reason the revoke matters: a role
that could connect to `otto_platform` would hold whatever `otto_app` holds there. So, a rule for
every service on `otto-db`: **no service may grant `CONNECT`, or any other database-level
privilege, to `otto_app`**. A grant to it would hand that database to every service's login
role. Giving each service its own tenant role is
[#28](https://github.com/savvagent/otto-cxp/issues/28).

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

It logs, for example,
`database role "otto_cxp" is SUPERUSER, BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION: refusing to serve`,
`database role "cxp_server" (session user "otto_cxp"): session user "otto_cxp" is SUPERUSER`
for a superuser login whose connections `SET ROLE cxp_server`, or
`database role "cxp_server" can connect to database "otto_platform" (through "otto_app")`.
A developer's machine (loopback URLs, no `FLY_APP_NAME`, none of those databases on its
Postgres) only warns, since a local Postgres hands out a superuser.

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

-- Everything in otto_cxp is cxp_server's now. Must be 0 rows.
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
```

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
--no-tail` shows `tenant isolation enforced as role "otto_app"`, and in `fly postgres connect -a
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
#    setup its step R5 imports the secret with --stage, and R6 and R7 follow the first deploy).
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

## Verify

```bash
curl -s https://otto-cxp.savvagent.com/healthz
curl -s https://otto-cxp.savvagent.com/readyz
curl -s https://otto-cxp.savvagent.com/.well-known/oauth-protected-resource
fly logs -a otto-cxp --no-tail
```

The logs must show, in order:

- `database role "cxp_server" holds no superuser-equivalent privilege (...) ... and can connect
  to no other service's database; the instance also hosts ...`
- `applying migrations`
- `tenant isolation enforced as role "otto_app"`
- `the otto platform accepted this service's credential`

If the platform REJECTED the credential, or `CXP_PLATFORM_URL` is not the platform, `/readyz`
answers 503 with `"reason": "platform credential rejected"` (or `"platform misconfigured"`), so a
deploy with a bad secret fails its health check. The check runs once at boot: after fixing the
secret, URL, or registration, restart the machines (`fly secrets set` already does). Readiness
reflects only that boot check, so a credential the platform starts refusing after boot shows as
500s on authenticated requests and `error` logs (`the platform rejected this resource server's
credential`), not as a failing `/readyz`.

Then point an MCP client at `https://otto-cxp.savvagent.com/mcp`: it is sent to the platform to
sign in, and `whoami` answers with the platform's records.

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
rollback of the image works), which means every migration must be additive. They run at boot;
confirm with the `applying migrations` log line.

## Local seeding (human-only against otto-db)

`scripts/seed-demo.sql` loads demo data for one org:

```bash
psql "$DATABASE_URL" -v org=<org-uuid> -f scripts/seed-demo.sql
```

With the local container (`podman compose up -d`, port 15435) this is safe and idempotent per
org. Pointed at `otto-db` it is a production write: run it only deliberately, with a real org
uuid from the platform, and only by hand (Rob).
