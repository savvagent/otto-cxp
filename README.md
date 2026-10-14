# otto-cxp

otto-cxp is the customer-experience service of the [otto platform](https://otto.savvagent.com):
customers and their interactions, support tickets, a knowledge base, chat, feedback and
surveys (with NPS, CSAT, and sentiment analysis), and an experience catalog. It is built for AI
agents to work through, over MCP, and for apps to reach over a REST API.

It is an HTTP-only resource server. The platform owns identity, OAuth, orgs, teams, and
billing; otto-cxp validates the platform's bearer tokens and keeps only its own data.

## What it serves

One binary, `cxp-server`, on port 8080:

| Path | What | Auth |
|---|---|---|
| `POST /mcp` | MCP tools, resources, and prompts (streamable HTTP, stateless) | platform bearer token |
| `/api/v1/...` | REST API for apps | platform bearer token |
| `POST /platform/webhooks` | org, member, and team lifecycle events from the platform | `Otto-Signature` HMAC |
| `/.well-known/oauth-protected-resource` | points MCP clients at the platform to sign in | none |
| `/healthz`, `/readyz` | liveness and readiness | none |

Tokens must carry the `cxp:read` or `cxp:write` scope and be issued for this service's
audience (`CXP_RESOURCE_URI`).

**Tenancy.** Every row belongs to an org. Isolation is Postgres row-level security under the
`otto_app` role, with composite `(org_id, id)` foreign keys, and `cxp-server` refuses to boot if
the isolation check fails. Org and user ids are opaque uuids issued by the platform.

## Connecting an MCP client

The endpoint is `https://otto-cxp.savvagent.com/mcp` (streamable HTTP). You need an account in
an org on the [otto platform](https://otto.savvagent.com); the platform handles sign-in.

**Claude Code:**

```bash
claude mcp add --transport http --scope user otto-cxp https://otto-cxp.savvagent.com/mcp
```

`--scope user` makes it available in every project; leave it out to add it to the current
project only. Then, in a Claude Code session, run `/mcp`, pick `otto-cxp`, and choose
**Authenticate**. Your browser opens the platform's sign-in and consent page; approve it and
return to Claude Code. Ask it to call the otto-cxp `whoami` tool: it answers with your org and
user as the platform knows them.

**Other MCP clients:** add a remote (streamable HTTP) server with the URL above. No client id or
API key is needed: an unauthenticated request gets `401` with a `WWW-Authenticate` header naming
`/.well-known/oauth-protected-resource`, which points the client at the platform for OAuth.

**Scopes:** `cxp:read` covers the read tools, `cxp:write` the tools that create or change data;
the platform's consent page shows which ones a sign-in grants. A tool called without the scope
it needs is refused with `insufficient_scope`. `whoami` and `usage` need no scope.

## The server

A Rust workspace under `crates/`:

| Crate | What |
|---|---|
| `cxp-core` | All SQL and domain logic: schema and migrations, tenancy, auth core, metering, lifecycle |
| `cxp-mcp` | The MCP server: tools, resources, prompts, and the `/mcp` routes |
| `cxp-api` | The REST API and the platform's webhooks |
| `cxp-server` | The binary: config, boot, health, background tasks, router |

## Local development

Prerequisites: Rust (stable, via `rust-toolchain.toml`), Podman or Docker, Node 22 and pnpm 9.

```bash
podman compose up -d                     # Postgres 16 on :15435 (cxp/cxp/otto_cxp)
cp .env.example .env                     # then fill in the platform settings
cargo run -p cxp-server                  # or ./scripts/start-server.sh
```

Migrations run at startup. `cxp-server` needs a reachable otto platform
(`CXP_PLATFORM_URL`) and the secrets from registering this service there; see
[docs/deploy/fly.md](./docs/deploy/fly.md).

Demo data for one org (local Postgres only):

```bash
psql "$DATABASE_URL" -v org=<org-uuid> -f scripts/seed-demo.sql
```

## Tests

```bash
DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test
```

`pnpm lint` is currently a no-op: no package defines a `lint` script.

The Rust tests run against a real Postgres (`#[sqlx::test]`), never mocks, and panic when
`DATABASE_URL` is unset. CI runs the same gates (`.github/workflows/ci.yml`).

## SDKs and packages

- **[@cxp/sdk](./sdk/typescript)** (`sdk/typescript`): TypeScript client over MCP. `connectCxp({ url, token })`
  (or an OAuth `authProvider`) returns `{ cxp }` with `cxp.support`, `cxp.feedback`, and `cxp.cdp`.
  See [SURVEYS.md](./sdk/typescript/SURVEYS.md) for the survey model.
- **[@cxp/react](./packages/react)** (`packages/react`): React components and hooks on top of `@cxp/sdk`.
- **`sdk/swift`, `sdk/kotlin`**: stubs, not built or tested in CI.
- **`apps/web`**: the web app.

## Deployment

Fly.io, app `otto-cxp`, database `otto_cxp` on the shared `otto-db`. Every push to `main` that
passes CI deploys once the repo variable `FLY_DEPLOY` is `enabled`. Runbook, secrets, and the
human-only first-deploy steps: [docs/deploy/fly.md](./docs/deploy/fly.md). Design:
[docs/specs/2026-10-14-otto-resource-server-design.md](./docs/specs/2026-10-14-otto-resource-server-design.md).

## License

AGPL-3.0-or-later for the server (`crates/`), `apps/`, and repo-level files; MIT for `sdk/` and
`packages/`. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
