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
