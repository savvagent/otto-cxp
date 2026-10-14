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
  which FORCE ROW LEVEL SECURITY binds too. A deployment (a non-loopback `CXP_PUBLIC_URL`)
  refuses to serve when the connecting role is, or can `SET ROLE` to, a superuser or BYPASSRLS
  role ([#16](https://github.com/savvagent/otto-cxp/issues/16)), so the role `fly postgres
  attach` creates, a superuser on the whole instance, is refused.
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
NOCREATEROLE, a member of `otto_app` (so tenant transactions can `SET LOCAL ROLE otto_app`), and
the owner of database `otto_cxp` and every object in it. Owning the database lets it run
otto-cxp's migrations (`pg_trgm` is a trusted extension a database owner may create); FORCE ROW
LEVEL SECURITY binds it as the tables' owner, so its background work reaches other orgs' rows
only through the maintenance policies. `CONNECT` on `otto_cxp`, `otto_platform`,
`otto_factory`, and `otto_flags` is revoked from `PUBLIC`, so `cxp_server` can open no other
service's database. `otto_app` is shared by every otto service, which is one more reason the
revoke matters: a role that could connect to `otto_platform` would hold whatever `otto_app`
holds there.

A deployment (a non-loopback `CXP_PUBLIC_URL`) refuses to serve as a role that is, or is a
member of, a superuser or BYPASSRLS role, and logs
`database role "..." is SUPERUSER: refusing to serve`. A developer's machine (`localhost`) only
warns, since a local Postgres hands out a superuser.

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
CREATE ROLE cxp_server LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOREPLICATION;
\password cxp_server
-- INHERIT and SET, the defaults: otto-tenant needs both to SET LOCAL ROLE otto_app.
GRANT otto_app TO cxp_server;
```

**R2. Give it `otto_cxp`.** Still as postgres, in `otto_cxp` (`REASSIGN OWNED` acts on the
current database's objects plus shared ones; the guard checks there are no others):

```sql
\c otto_cxp
\set ON_ERROR_STOP on
\set old_role otto_cxp
BEGIN;
SELECT set_config('runbook.old_role', :'old_role', true);
DO $$
DECLARE
  old regrole := current_setting('runbook.old_role')::regrole;
BEGIN
  IF current_database() <> 'otto_cxp' THEN
    RAISE EXCEPTION 'run this connected to otto_cxp, not %', current_database();
  END IF;
  IF old::text IN ('cxp_server', 'postgres') THEN
    RAISE EXCEPTION 'old_role must be the attach role, not %', old;
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
FROM pg_database d WHERE d.datname = 'otto_cxp' AND d.datdba <> 'cxp_server'::regrole;
```

**R3. Close the other databases.** A role that connects to `otto_platform`, `otto_factory`, or
`otto_flags` only through `PUBLIC` loses access at the revoke, so first find every non-superuser
login role that can connect today (superusers, such as the other services' attach roles, skip
privilege checks and need nothing):

```sql
SELECT r.rolname, string_agg(d.datname, ', ' ORDER BY d.datname) AS can_connect_today
FROM pg_roles r CROSS JOIN pg_database d
WHERE r.rolcanlogin AND NOT r.rolsuper AND r.rolname <> 'cxp_server'
  AND d.datname IN ('otto_platform', 'otto_factory', 'otto_flags')
  AND has_database_privilege(r.oid, d.oid, 'CONNECT')
GROUP BY r.rolname ORDER BY r.rolname;
```

For each row, grant the one database that role's service uses (the database in its
`DATABASE_URL`, e.g. `fly ssh console -a otto-flags -C 'printenv DATABASE_URL'`), and only that
one, e.g. `GRANT CONNECT ON DATABASE otto_flags TO <role>;`. Then:

```sql
REVOKE CONNECT ON DATABASE otto_platform, otto_factory, otto_flags, otto_cxp FROM PUBLIC;
GRANT CONNECT ON DATABASE otto_cxp TO cxp_server;

-- Who may connect now: superusers, the grants just made, and cxp_server on otto_cxp only.
SELECT d.datname, string_agg(r.rolname, ', ' ORDER BY r.rolname) AS may_connect
FROM pg_database d CROSS JOIN pg_roles r
WHERE d.datname IN ('otto_platform', 'otto_factory', 'otto_flags', 'otto_cxp')
  AND r.rolcanlogin AND has_database_privilege(r.oid, d.oid, 'CONNECT')
GROUP BY d.datname ORDER BY d.datname;
```

Existing sessions are not dropped by a revoke; a role that was missed fails at its next
connection, and `GRANT CONNECT` fixes it at once. The `postgres` and `repmgr` databases are
left as they are (no service data; Fly's tooling uses them).

**R4. Prove the role before using it (read-only).** In a second terminal
`fly proxy 15499:5432 -a otto-db`, then:

```bash
psql "postgres://cxp_server@localhost:15499/otto_cxp"     # prompts for the R1 password
```

```sql
-- f, f, f, f
SELECT rolname, rolsuper, rolbypassrls, rolcreaterole, rolcreatedb
FROM pg_roles WHERE rolname = current_user;
-- 0 rows: what boot checks
SELECT r.rolname FROM pg_roles r
WHERE (r.rolsuper OR r.rolbypassrls) AND pg_has_role(current_user, r.oid, 'MEMBER');
-- t, t: it can assume otto_app, and migrate
SELECT pg_has_role(current_user, 'otto_app', 'USAGE') AS assumes_otto_app,
       has_schema_privilege('public', 'CREATE') AS can_migrate;
-- 0: row-level security binds it with no org pinned
SELECT count(*) FROM customers;
-- t only for otto_cxp (and postgres, repmgr)
SELECT datname, has_database_privilege(current_user, oid, 'CONNECT') AS may_connect
FROM pg_database WHERE NOT datistemplate ORDER BY datname;
-- must fail: permission denied for database "otto_platform"
\c otto_platform
```

**R5. Switch the running release to it.** Keep the host, port, and query string from R0's URL
(for an attach, `otto-db.flycast:5432` and `?sslmode=disable`):

```bash
fly secrets set -a otto-cxp \
  DATABASE_URL='postgres://cxp_server:<R1 password>@<host>:<port>/otto_cxp<query>'
```

Not `--stage`: this restarts the release already running, before #16 merges, so the role switch
is proven apart from the code change (that release runs every database path the same way; the
e2e test `the_service_runs_whole_as_a_least_privilege_database_owner` proves them as such a
role). Then `curl -s https://otto-cxp.savvagent.com/readyz` answers ready, `fly logs -a otto-cxp
--no-tail` shows `tenant isolation enforced as role "otto_app"`, and in `fly postgres connect -a
otto-db`:

```sql
-- cxp_server only (plus your own session)
SELECT usename, count(*) FROM pg_stat_activity WHERE datname = 'otto_cxp' GROUP BY 1;
```

To undo R5: in `fly postgres connect -a otto-db`, `\password <attach role>` with a fresh
password, then `fly secrets set -a otto-cxp DATABASE_URL=...` with that user. The attach role
keeps LOGIN until R7 for this.

**R6. Merge #16.** Its deploy logs
`database role "cxp_server" is neither a superuser nor BYPASSRLS, nor a member of one` before
`applying migrations` (see [Verify](#verify)).

**R7. Retire the attach role** once R6 is verified:

```sql
ALTER ROLE otto_cxp NOLOGIN;   -- the attach role from R0
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
#    setup its step R5 sets the secret with --stage, and R6 and R7 follow the first deploy).
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

- `database role "cxp_server" is neither a superuser nor BYPASSRLS, nor a member of one`
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
