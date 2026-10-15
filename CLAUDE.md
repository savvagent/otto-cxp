# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

otto-cxp is an HTTP-only MCP and REST resource server of the otto platform (identity, OAuth, orgs,
teams, and billing live there). Design: `docs/specs/2026-10-14-otto-resource-server-design.md`.
Deploy and ops: `docs/deploy/fly.md`.

## Build & Run Commands

### Rust server
```bash
podman compose up -d                     # Postgres 16 on :15435 (cxp/cxp/otto_cxp)
cp .env.example .env                     # fill in the platform settings
cargo run -p cxp-server                  # serves on 0.0.0.0:8080 (CXP_BIND)
cargo build --workspace
DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked
cargo test -p cxp-core test_name         # one test
```
`cargo test` needs `DATABASE_URL` (a local Postgres only) and panics without it. Migrations run at
server start (`cxp_core::migrate`).

### TypeScript/React (pnpm monorepo: `sdk/typescript`, `packages/react`, `apps/web`)
```bash
pnpm install --frozen-lockfile
pnpm build   # tsup / vite
pnpm test    # vitest
pnpm lint    # currently a no-op: no package defines a `lint` script
```
Per-package: `cd sdk/typescript && pnpm build`.

## Architecture

One binary, `cxp-server`, on `:8080`: `/mcp` (streamable HTTP, stateless), `/api/v1/...`,
`POST /platform/webhooks`, `/.well-known/oauth-protected-resource`, `/healthz`, `/readyz`, and `/`
(a connector install page for browsers only; JSON 404 for everything else).

**Crates (`crates/`):**
- `cxp-core` - everything that touches SQL, plus domain logic: `migrations/` (append-only), tenancy
  (`otto_tenant`), the per-domain `*Ext` traits (customers, support, feedback, catalog, dashboard),
  analytics (NPS, CSAT, sentiment: pure functions), `scopes`, `platform_events` (lifecycle webhooks,
  `begin_live`), `usage` (metering, outbox shipper), `maintenance` (the only cross-org access).
- `cxp-http` - the axum side of auth, shared by `/mcp` and `/api/v1`: `Authenticator`, `require_bearer`,
  `refuse`, and the `/.well-known/oauth-protected-resource` router. Holds no SQL. A new credential
  source (the console session cookie) is opt-in per surface: `/api` only, same-origin check on
  writes; `/mcp` stays bearer-only.
- `cxp-mcp` - the `rmcp` 2.0.0 server: tools, resources, prompts, and the `/mcp` route behind
  `cxp_http::require_bearer`. Holds no SQL.
- `cxp-api` - REST `/api/v1` and `/platform/webhooks`. Holds no SQL.
- `cxp-server` - the binary: `config.rs`, boot sequence (migrate, verify tenant isolation, check the
  platform credential, spawn the usage shipper and sweeper), health routes, router (mounts
  `/.well-known/oauth-protected-resource`), `tests/e2e.rs`.

**SDKs:**
- `sdk/typescript/` - `@cxp/sdk` (MIT): `connectCxp({ url, token | authProvider })` returns `{ cxp }` with
  `.support`, `.feedback`, `.cdp`.
- `packages/react/` - `@cxp/react` (MIT): components and hooks over `@cxp/sdk`. `apps/web/` - web app.
- `sdk/swift/`, `sdk/kotlin/` - stubs with no gate.

## Critical Patterns

### rmcp 2.0.0 tool registration (cxp-mcp)
Tools silently don't appear unless all of these are in place:
1. `#[tool_router(router = <name>_router)]` on the `impl Cxp` block of each tool file (one file per
   domain), with `#[tool]` on each tool method; parameters use `Parameters<T>` structs.
2. The per-file routers are added together into one `ToolRouter<Self>` stored on the server struct.
3. `#[tool_handler]` on the `impl ServerHandler`.
Follow the existing files in `crates/cxp-mcp/src/` rather than the rmcp 0.15 style: tools return
errors as results through the crate's error mapping, never panic, and never put database text in a
response. Every tool checks `cxp:read` or `cxp:write`, except `whoami` and `usage`, which need no
scope (`tools::UNSCOPED`).

### Tenancy: `Tx` extension traits and `begin_live`
Domain operations are methods on extension traits implemented for `otto_tenant::Tx<'_>` (for example
`impl CustomersExt for Tx<'_>`), binding `self.org()` and executing on `self.conn()`. Request paths open
the transaction with `platform_events::begin_live` (it also refuses revoked orgs and members), never
`Db::begin`. Cross-org access exists only in `cxp_core::maintenance`. Never take an org id from
anywhere but the verified token.

### PostgreSQL enum handling
The DB has 6 custom enum types (`ticket_status`, `ticket_priority`, `ticket_channel`, `feedback_type`,
`interaction_type`, `message_role`). sqlx cannot decode PG enums to Rust `String` directly:
- **SELECT/RETURNING**: cast with `::text` - `status::text as status`
- **INSERT/UPDATE params**: cast with `::enum_name` - `$5::ticket_priority`
- Use column list constants (e.g., `TICKET_COLS`) to avoid repeating casts across queries

Model structs store enum columns as `String`, not Rust enums.

### Metering
Exactly `submit_feedback`, `create_survey`, `send_survey`, `submit_survey_response` record one usage
event each (`Meter::charge`, in the tool's own transaction, into `usage_outbox`); the only route that
records one is `POST /api/v1/surveys` (charged as `create_survey`). Nothing else meters.

### Migrations
Append-only: add `crates/cxp-core/migrations/NNNN_<name>.sql`, never edit an applied one. New tenant
tables need `org_id`, forced RLS with a `<table>_tenant_isolation` policy, grants to `otto_app`, and
composite `(org_id, id)` foreign keys.

## Configuration

All `CXP_*`; `cxp-server` `config.rs` is the source of truth, `.env.example` the local template,
`fly.toml` `[env]` production.

| Variable | Default | Notes |
|---|---|---|
| `CXP_PUBLIC_URL` | required | public origin, no path (e.g. `https://otto-cxp.savvagent.com`) |
| `CXP_RESOURCE_URI` | `{public}/mcp` | token audience; must equal the URI registered at the platform |
| `CXP_PLATFORM_URL` | required | the otto platform; `https` unless the host is loopback |
| `CXP_INTROSPECTION_SECRET` | required (secret) | `otto_rs_...` |
| `CXP_PLATFORM_WEBHOOK_SECRET` | required (secret) | `otto_whsec_...` |
| `CXP_DATABASE_URL` | falls back to `DATABASE_URL` | `postgres://cxp:cxp@localhost:15435/otto_cxp` locally |
| `CXP_DB_MAX_CONNECTIONS` | `5` | 2 to 20; the production database is shared and small |
| `CXP_RUN_MIGRATIONS` | `true` | |
| `CXP_BIND` | `0.0.0.0:8080` | |
| `CXP_ALLOWED_HOSTS` | empty | EXTRA Host headers `/mcp` accepts, as `host` or `host:port`; the `CXP_PUBLIC_URL` host is always allowed |
| `CXP_ALLOWED_ORIGINS` | empty | browser origins for `/mcp`, as `scheme://host[:port]`; empty means no check |
| `CXP_ENFORCE_QUOTAS` | `false` | fails open |
| `CXP_LOG_FORMAT` | `text` | `text` or `json` |
| `RUST_LOG` | `info` | |

## Conventions
- All model structs: `Debug, Clone, Serialize, Deserialize, sqlx::FromRow`
- UUIDs: `uuid::Uuid`, timestamps: `chrono::DateTime<Utc>`, JSONB: `serde_json::Value`, arrays: `Vec<String>`
- Runtime `sqlx::query`/`query_as` only, never the `query!` macros; no SQL outside `cxp-core`
- Errors: `cxp_core::Error` with a stable `code()`; its `Display` never includes database text
- Tests use `#[sqlx::test]` against a real Postgres, no mocks
- Dependencies are declared in the root `[workspace.dependencies]`; `otto-tenant` and `otto-resource` share one git rev
- TypeScript SDK: dual CJS+ESM via tsup, strict mode, `.js` extension imports
- Licensing: AGPL-3.0-or-later for `crates/`, `apps/`, repo-level files; MIT for `sdk/` and `packages/`

## Development Workflow

### Repo
- slug: savvagent/otto-cxp
- trunk: main
- tracker: github
- epic tracker: github
- ticketless work: open-issue
- status labels: none

### Tickets
- default assignee: none
- priority labels: none
- estimate labels: none
- types: bug=bug feature=enhancement docs=documentation epic=epic
- skip labels: wontfix, duplicate, invalid

### Branch, commit, PR
- branch: <type>/<issue>-<slug>, type = feat | fix | docs | refactor | test | chore
- commit: conventional commits, `<type>(<scope>): <imperative, lower-case>`; scope optional (e.g. `feat(core): …`); body explains why and ends `Fixes #N` (or `Refs #N` for a story merged into an epic branch)
- PR body must contain: Fixes #N (one line per issue the PR closes)
- merge: gh pr merge <pr> --repo savvagent/otto-cxp --squash --delete-branch
- after merge: git branch -D <branch>

The `main` ruleset (`main-protection`) only blocks deletion and force-pushes; it requires no
reviews or status checks, so the merge gate below is on the agent.

### Worktree
- path: .claude/worktrees/<branch with "/" replaced by "-">
- bootstrap:
  - Postgres: `podman ps --format '{{.Ports}}' | grep -q ':15435->'`; if that fails, run `podman start otto-cxp-postgres 2>/dev/null || podman run -d --name otto-cxp-postgres -e POSTGRES_USER=cxp -e POSTGRES_PASSWORD=cxp -e POSTGRES_DB=otto_cxp -p 127.0.0.1:15435:5432 docker.io/library/postgres:16` (one container shared by every worktree; `compose.yaml` describes the same one)
  - Role: `psql postgres://cxp:cxp@localhost:15435/otto_cxp -tAc "select 1 from pg_roles where rolname='otto_app'"`; if it prints nothing instead of `1`, run `psql postgres://cxp:cxp@localhost:15435/otto_cxp -c "CREATE ROLE otto_app NOLOGIN;"`
  - If the change touches a path in the pnpm gate below: `pnpm install --frozen-lockfile`
  - If you will run the server locally (the gates do not need it): `cp .env.example .env`, then fill in the platform settings it lists
- teardown: none

`#[sqlx::test]` creates and drops a database per test, named from the test path alone, so two
worktrees running `cargo test` against one Postgres at the same time drop each other's
databases. Serialize DB test runs across worktrees, or give a worktree its own container on
another port (`podman run -d --name cxp-pg-<branch_> -e POSTGRES_USER=cxp -e POSTGRES_PASSWORD=cxp -e POSTGRES_DB=otto_cxp -p <port>:5432 postgres:16`, removed at teardown) and point `DATABASE_URL` at it. Ports 15433 (otto-factory) and 15434 (otto-flags) are taken.

### Gates
- test: DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked
- lint: cargo clippy --workspace --all-targets --locked -- -D warnings
- format: cargo fmt --all -- --check
- extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'` must print nothing
- when Dockerfile, .dockerignore, Cargo.toml, Cargo.lock, rust-toolchain.toml, crates/**: podman build -t otto-cxp:check .   (only once a `Dockerfile` exists)
- when package.json, pnpm-lock.yaml, pnpm-workspace.yaml, sdk/typescript/**, packages/react/**, apps/web/**: pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test
- evidence: report pass counts per test binary; `cargo test` without `DATABASE_URL` panics rather than skipping, so a panic there is a missing variable, not a test failure; `DATABASE_URL` points only at a local Postgres, never at a Fly database
- toolchain: stable via `rust-toolchain.toml`; CI floats stable

`pnpm lint` is currently a no-op (no package defines a `lint` script), so the pnpm gate proves build and tests only.

`sdk/swift` and `sdk/kotlin` are stubs with no gate.

### Design docs
- mode: committed
- spec path: docs/specs/YYYY-MM-DD-<slug>-design.md
- plan path: docs/plans/YYYY-MM-DD-<slug>.md
- shipped: same-pr: a final commit before merge sets the spec's `**Status:**` line to `IMPLEMENTED (#<PR>)` and ticks the plan's task boxes

### Reviews
- language reviewers: crates/** -> rust-pro, sdk/typescript/** -> typescript-pro, packages/react/** -> typescript-pro, apps/web/** -> typescript-pro
- extra reviewers: database-optimizer when SQL, migrations, indexes, or RLS policies change
- security focus: tenant isolation (RLS under `otto_app`, composite foreign keys, org ids trusted from the request); bearer-token audience, expiry, and scope checks; team ids verified at the platform before they are stored; platform webhook signature verification and idempotency; untrusted input (SQL built from strings, unbounded bodies, customer-supplied text reaching an agent, database error text in responses); secret handling (logging, echoing, timing-safe compare); customer PII
- automated reviewer: none

### Parallelism
- max agents: 4

### Ship
- deploy: ci-auto
- merge gate: on the PR's head SHA: `rust`, `docker-build`, `migrations-append-only` and `web` (CI workflow) succeeded, plus every other check that ran on the head SHA (the ruleset requires none). Before `.github/workflows/ci.yml` exists on the PR's head, the merge gate is the full local gate run on the head SHA with a clean tree
- verify: the SHA proxy is the `deploy` job of the CI run for the merge SHA (`gh run list -R savvagent/otto-cxp --workflow CI --commit <merge-sha> --json databaseId,status,conclusion`); then `fly status -a otto-cxp` shows the machines on the new release; `curl -s https://otto-cxp.savvagent.com/readyz` returns ready; `fly logs -a otto-cxp --no-tail` shows `applying migrations`, `tenant isolation enforced as role "otto_app"`, and `the otto platform accepted this service's credential`; then exercise the changed surface read-only (`whoami` first). The first deploy is a hand `fly deploy -a otto-cxp --remote-only` (Rob); he sets `FLY_DEPLOY=enabled` only after verifying it. While the repo variable `FLY_DEPLOY` is not `enabled` the deploy job is skipped and a merge deploys nothing: report merged, not live
- release: none (continuous deploy of `main`); `@cxp/sdk` and `@cxp/react` are not published, and a publish is a human decision (Rob)
- version files: crates/*/Cargo.toml (workspace version), sdk/typescript/package.json, packages/react/package.json

### Out-of-band
- Migrations (agent): apply at boot under sqlx's advisory lock; confirm in the boot logs.
- New non-secret config key (agent, in the PR): `fly.toml` `[env]`, `.env.example`, and `crates/cxp-server/src/config.rs`.
- New secret (human: Rob, before merge): `fly secrets set -a otto-cxp --stage NAME=…`; the PR updates `.env.example` and the secrets table in `docs/deploy/fly.md`.
- New scope (human: Rob, before merge): the PR updates `cxp_core::scopes::KNOWN`/`DEFAULT`; Rob re-runs `otto-platform-server resource register …` from `docs/deploy/fly.md`.
- otto-platform pin (agent, own PR): `otto-tenant` and `otto-resource` move to the same rev together.

### Human-only
Hand these to Rob with the exact command; never run them:
- Any production write, DDL, `DROP`, or role change on `otto-db` (the shared Fly Postgres also hosting `otto_platform`, `otto_factory`, and `otto_flags`). Production diagnostics are SELECT-only.
- `fly apps create`, `fly postgres attach`, `fly secrets set`, `fly certs add`, DNS, setting the `FLY_API_TOKEN` secret or `FLY_DEPLOY` variable on the repo, and platform-side `otto-platform-server resource …` / `client register …` commands.
- A hand `fly deploy` (only when Rob asks).
- Anything that adds recurring cost: a new app, bigger VM, `min_machines_running > 0`, a volume, a database, or a paid service.
- Merging a change to another otto repo that deploys it (e.g. the otto-platform marketing entry).
- Package publishes.

### Invariants
Reviewers check every one. Design: `docs/specs/2026-10-14-otto-resource-server-design.md`.
- **Append-only migrations.** A schema change is a new `crates/cxp-core/migrations/NNNN_<name>.sql`; never modify, rename, or delete one that is on `main` (a migration still only on an unmerged branch may be edited; recreate local databases that ran it). `MIGRATOR` sets `ignore_missing`, so the previous release boots against a newer schema: migrations are additive only. A `SECURITY DEFINER` function pins its `search_path` and is `REVOKE ALL … FROM PUBLIC` before being granted to `otto_app`.
- **Tenant isolation is RLS, proven.** Every tenant table has `org_id`, enables and forces RLS with a policy named `<table>_tenant_isolation`, grants to `otto_app`, and is covered by the tenant-isolation test; `cxp-server` refuses to start if isolation fails at boot. Tenant transactions run as `otto_app` (`SET LOCAL ROLE`); background maintenance and the `org.deleted` tombstone run as the connecting role, which the policies bind only while that role is subject to RLS (neither superuser nor `BYPASSRLS`). In production it is still a superuser until #16 gives it a least-privilege login role.
- **Composite `(org_id, id)` foreign keys** between tenant tables (FK checks bypass RLS).
- **Request paths open transactions with `platform_events::begin_live`**, never `Db::begin`; unpinned cross-org access goes only through `cxp_core::maintenance`. The database enforces it: the unpinned policies on `platform_events` and on the usage outbox tables (`usage_outbox`, `usage_outbox_rejected`) admit a transaction only when no org is pinned **and** it carries the maintenance marker (`app.maintenance = 'on'`); an unpinned transaction without the marker sees nothing. Only `cxp_core::maintenance::begin` sets the marker.
- **No SQL outside `cxp-core`**, and only runtime `sqlx::query`/`query_as`, never the `query!` macros.
- **Auth.** `/mcp` and `/api` require a platform token whose audience equals `CXP_RESOURCE_URI` exactly; every route, and every tool except `whoami` and `usage` (no scope), checks `cxp:read` or `cxp:write`. Unauthenticated requests get 401 with `WWW-Authenticate` naming `/.well-known/oauth-protected-resource`.
- **Metering:** exactly the tools `submit_feedback`, `create_survey`, `send_survey`, `submit_survey_response` record one usage event each, in the tool's own transaction, into `usage_outbox`; the only route that records one is `POST /api/v1/surveys` (charged as `create_survey`); nothing else records one.
- **Teams belong to the platform.** Catalog `team_id` is an opaque platform team id, verified at the platform on write; there is no local teams table.
- **Platform webhooks** (`org.deleted`, `member.removed`, `team.deleted`) are signature-verified, fail closed, and apply idempotently.
- **Untrusted text:** customer-supplied text returned by a tool is data, and database error text never reaches a response.
- **The database is small and shared:** pool capped by `CXP_DB_MAX_CONNECTIONS` (5); no unbounded list queries.
- **Licensing:** `crates/`, `apps/`, and repo-level files are AGPL-3.0-or-later; `sdk/` and `packages/` are MIT.
- **otto-platform pin:** `otto-tenant` and `otto-resource` share one git rev in the root `Cargo.toml`.
