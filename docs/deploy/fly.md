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
- **Database** `otto_cxp` on `otto-db`, the unmanaged Fly Postgres shared with `otto_platform`,
  `otto_factory`, and `otto_flags`. Plain Postgres 16 is enough: pgvector is NOT needed (the
  schema uses only the stock `pgcrypto` and `pg_trgm`). The attach role is a superuser on the
  whole instance, so isolation inside this database rests on row-level security under
  `SET LOCAL ROLE otto_app`, which `cxp-server` proves at boot
  (`tenant isolation enforced as role "otto_app"`) and refuses to start without. The pool is
  capped at 5 connections (`CXP_DB_MAX_CONNECTIONS`) because the instance is small and shared.
  Split onto a dedicated instance before real customers.
- **Migrations** are otto-cxp's own (`crates/cxp-core/migrations`) and run at boot under an
  advisory lock. Never run the platform's migrations here.

## Secrets

| Secret | What | Where from |
|---|---|---|
| `DATABASE_URL` | Postgres URL for `otto_cxp` | `fly postgres attach` |
| `CXP_INTROSPECTION_SECRET` | `otto_rs_...`, authenticates every call to the platform | `resource rotate-secret` |
| `CXP_PLATFORM_WEBHOOK_SECRET` | `otto_whsec_...`, verifies platform webhooks | `resource set-webhook` |

Everything else is in `fly.toml`'s `[env]`. A new secret needs `fly secrets set -a otto-cxp
--stage NAME=...` before the release that reads it, plus a row here and in `.env.example`.

## First deploy

Every step below is human-only (Rob): each one creates infrastructure, writes to the shared
database or the platform, or sets a repo credential. The agent that wrote this repo has run
none of them. Do them in order. The first deploy is a hand `fly deploy`; the CI deploy job
stays off (`FLY_DEPLOY` unset) until that deploy is verified.

```bash
# 1. App, then its hostname first so the certificate issues while you do the rest.
fly apps create otto-cxp --org savvagent
fly certs add otto-cxp.savvagent.com -a otto-cxp
# DNS (Namecheap): A and AAAA records for otto-cxp -> the app's IPs (`fly ips list -a otto-cxp`)

# 2. Database. Attach creates database otto_cxp and sets DATABASE_URL.
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

- `applying migrations`
- `tenant isolation enforced as role "otto_app"`
- `the otto platform accepted this service's credential`

Then point an MCP client at `https://otto-cxp.savvagent.com/mcp`: it is sent to the platform to
sign in, and `whoami` answers with the platform's records.

Usage backlog (should drain to 0 within seconds of activity). Connect with
`fly postgres connect -a otto-db -d otto_cxp` and run SELECTs only; production diagnostics
never write:

```sql
SELECT count(*), max(attempts), max(last_error) FROM usage_outbox;
```

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
