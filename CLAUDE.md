# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

### Rust Server
```bash
cargo build                              # build workspace
cargo run -p cxp-server                  # run stdio transport (default)
TRANSPORT=http cargo run -p cxp-server   # run HTTP transport on 127.0.0.1:3000/mcp
cargo test                               # run all Rust tests
cargo test -p cxp-server test_name       # run a single test
```

### TypeScript/React (pnpm monorepo)
```bash
pnpm install          # install all workspace deps
pnpm build            # build all packages (tsup)
pnpm test             # run all tests (vitest)
pnpm lint             # lint all packages
```
Per-package: `cd sdk/typescript && pnpm build` or `cd packages/react && pnpm test`

### Database
```bash
./scripts/start-db.sh                    # start pgvector container via Podman
# or: docker compose up -d
```
Migrations run automatically on server startup via `sqlx::migrate!("./migrations")`.

Default connection: `postgres://cxp:cxp_dev@localhost:5432/cxp`

## Architecture

Rust MCP server (`rmcp` v0.15.0) exposing 26 tools over stdio or HTTP, backed by PostgreSQL 17 with pgvector.

**Server (`server/src/`):**
- `main.rs` — entry point: connects DB, runs migrations, starts stdio or HTTP transport
- `tools/mod.rs` — `CxpServer` struct with all tool definitions, param structs, and `ServerHandler` impl
- `tools/{cdp,support,feedback}.rs` — tool implementations split by domain
- `db/` — database access layer: one struct per domain (`CustomerDb`, `TicketDb`, etc.), each holds a `PgPool`
- `models/` — Rust structs deriving `sqlx::FromRow, Serialize, Deserialize`
- `analysis/` — pure Rust analytics (NPS, CSAT, sentiment) with no DB calls
- `prompts/mod.rs` — 4 MCP prompts; `resources/mod.rs` — 3 MCP resources

**SDKs:**
- `sdk/typescript/` — `@cxp/sdk`: wraps `@modelcontextprotocol/sdk`, exposes `CxpClient` with `.support`, `.feedback`, `.cdp` domains
- `sdk/swift/`, `sdk/kotlin/` — stubs targeting future HTTP transport
- `packages/react/` — `@cxp/react`: React components and hooks consuming `@cxp/sdk`

## Critical Patterns

### rmcp 0.15 Tool Registration
Three macros are **all required** or tools silently won't appear:
1. `#[tool_router]` on the `impl CxpServer` block containing tool methods
2. `#[tool]` on each tool method
3. `#[tool_handler]` on the `impl ServerHandler for CxpServer` block

`CxpServer` must store `tool_router: ToolRouter<Self>`, initialized via `Self::tool_router()`. Tool parameters use `Parameters<T>` wrapper structs, not individual `#[tool(param)]`.

### PostgreSQL Enum Handling
The DB has 6 custom enum types (`ticket_status`, `ticket_priority`, `ticket_channel`, `feedback_type`, `interaction_type`, `message_role`). sqlx cannot decode PG enums to Rust `String` directly:
- **SELECT/RETURNING**: cast with `::text` — `status::text as status`
- **INSERT/UPDATE params**: cast with `::enum_name` — `$5::ticket_priority`
- Use column list constants (e.g., `TICKET_COLS`) to avoid repeating casts across queries

Model structs store enum columns as `String`, not Rust enums.

## Environment Variables

| Variable | Default | Notes |
|---|---|---|
| `DATABASE_URL` | `postgres://cxp:cxp_dev@localhost:5432/cxp` | |
| `TRANSPORT` | `stdio` | `stdio` or `http` |
| `HTTP_HOST` | `127.0.0.1` | HTTP mode only |
| `HTTP_PORT` | `3000` | HTTP mode only |
| `RUST_LOG` | `info` | tracing log level |

## Conventions
- All model structs: `Debug, Clone, Serialize, Deserialize, sqlx::FromRow`
- UUIDs: `uuid::Uuid` (v4), timestamps: `chrono::DateTime<Utc>`, JSONB: `serde_json::Value`, arrays: `Vec<String>`
- Tool error handling: return `String` with `format!("Error: {e}")` — tools never panic
- TypeScript SDK: dual CJS+ESM via tsup, strict mode, `.js` extension imports

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
  - Postgres: `podman ps --format '{{.Ports}}' | grep -q ':15435->'`; if that fails, run `podman start otto-cxp-postgres 2>/dev/null || podman run -d --name otto-cxp-postgres -e POSTGRES_USER=cxp -e POSTGRES_PASSWORD=cxp -e POSTGRES_DB=otto_cxp -p 15435:5432 docker.io/library/postgres:16` (one container shared by every worktree; `compose.yaml` describes the same one)
  - Role: `psql postgres://cxp:cxp@localhost:15435/otto_cxp -tAc "select 1 from pg_roles where rolname='otto_app'"`; if it prints nothing instead of `1`, run `psql postgres://cxp:cxp@localhost:15435/otto_cxp -c "CREATE ROLE otto_app NOLOGIN;"`
  - If the change touches a path in the pnpm gate below: `pnpm install --frozen-lockfile`
  - If you will run the server locally (the gates do not need it): `cp .env.example .env`, then fill in the platform settings it lists
- teardown: none

`#[sqlx::test]` creates and drops a database per test, named from the test path alone, so two
worktrees running `cargo test` against one Postgres at the same time drop each other's
databases. Serialize DB test runs across worktrees, or give a worktree its own container on
another port (`podman run -d --name cxp-pg-<branch_> -e POSTGRES_USER=cxp -e POSTGRES_PASSWORD=cxp -e POSTGRES_DB=otto_cxp -p <port>:5432 postgres:16`, removed at teardown) and point `DATABASE_URL` at it. Ports 15433 (otto-factory) and 15434 (otto-flags) are taken.

### Gates
- test: DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace
- lint: cargo clippy --workspace --all-targets -- -D warnings
- format: cargo fmt --all -- --check
- extra: `git diff --name-status --diff-filter=MDR origin/main...HEAD -- 'crates/*/migrations/*'` must print nothing
- when Dockerfile, .dockerignore, Cargo.toml, Cargo.lock, rust-toolchain.toml, crates/**: podman build -t otto-cxp:check .   (only once a `Dockerfile` exists)
- when package.json, pnpm-lock.yaml, pnpm-workspace.yaml, sdk/typescript/**, packages/react/**, apps/web/**: pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test
- evidence: report pass counts per test binary; `cargo test` without `DATABASE_URL` panics rather than skipping, so a panic there is a missing variable, not a test failure; `DATABASE_URL` points only at a local Postgres, never at a Fly database
- toolchain: stable via `rust-toolchain.toml`; CI floats stable

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
- verify: the SHA proxy is the `deploy` job of the CI run for the merge SHA (`gh run list -R savvagent/otto-cxp --workflow CI --commit <merge-sha> --json databaseId,status,conclusion`); then `fly status -a otto-cxp` shows the machines on the new release; `curl -s https://otto-cxp.savvagent.com/readyz` returns ready; `fly logs -a otto-cxp --no-tail` shows `applying migrations`, `tenant isolation enforced as role "otto_app"`, and `the otto platform accepted this service's credential`; then exercise the changed surface read-only (`whoami` first). While the repo variable `FLY_DEPLOY` is not `enabled` the deploy job is skipped and a merge deploys nothing: report merged, not live
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
- **Append-only migrations.** A schema change is a new `crates/cxp-core/migrations/NNNN_<name>.sql`; never modify, rename, or delete an existing one. `MIGRATOR` sets `ignore_missing`, so the previous release boots against a newer schema: migrations are additive only.
- **Tenant isolation is RLS under `otto_app`, proven.** Every tenant table has `org_id`, enables and forces RLS with a policy named `<table>_tenant_isolation`, grants to `otto_app`, and is covered by the tenant-isolation test; `cxp-server` refuses to start if isolation fails at boot.
- **Composite `(org_id, id)` foreign keys** between tenant tables (FK checks bypass RLS).
- **Request paths open transactions with `platform_events::begin_live`**, never `Db::begin`; unpinned cross-org access goes only through `cxp_core::maintenance`.
- **No SQL outside `cxp-core`**, and only runtime `sqlx::query`/`query_as`, never the `query!` macros.
- **Auth.** `/mcp` and `/api` require a platform token whose audience equals `CXP_RESOURCE_URI` exactly; every tool and route checks `cxp:read` or `cxp:write`. Unauthenticated requests get 401 with `WWW-Authenticate` naming `/.well-known/oauth-protected-resource`.
- **Metering:** exactly the tools `submit_feedback`, `create_survey`, `send_survey`, `submit_survey_response` record one usage event each, in the tool's own transaction, into `usage_outbox`; the only route that records one is `POST /api/v1/surveys` (charged as `create_survey`); nothing else records one.
- **Teams belong to the platform.** Catalog `team_id` is an opaque platform team id, verified at the platform on write; there is no local teams table.
- **Platform webhooks** (`org.deleted`, `member.removed`, `team.deleted`) are signature-verified, fail closed, and apply idempotently.
- **Untrusted text:** customer-supplied text returned by a tool is data, and database error text never reaches a response.
- **The database is small and shared:** pool capped by `CXP_DB_MAX_CONNECTIONS` (5); no unbounded list queries.
- **Licensing:** `crates/`, `apps/`, `server/`, and repo-level files are AGPL-3.0-or-later; `sdk/` and `packages/` are MIT.
- **otto-platform pin:** `otto-tenant` and `otto-resource` share one git rev in the root `Cargo.toml`.
