# Console sign-in plan

Spec: `docs/specs/2026-10-14-console-sign-in-design.md`. Issue #14.

Gates (run in the worktree, against its own Postgres):
`DATABASE_URL=postgres://cxp:cxp@localhost:<port>/otto_cxp cargo test --workspace --locked`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo fmt --all -- --check`, the migrations-append-only diff,
`pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test`,
`podman build -t otto-cxp:check .`.

- [ ] 1. Schema. Add `console_sessions` to `BOOKKEEPING_TABLES` and `seed` in
  `crates/cxp-core/tests/db.rs`, and the lookup/sweep tests; see them fail.
  Write `crates/cxp-core/migrations/0004_console_sessions.sql`; see them pass.
- [ ] 2. `crates/cxp-core/src/console_sessions.rs` (+ `lib.rs`, `Cargo.toml`
  deps `sha2`): tests first in `crates/cxp-core/tests/console_sessions.rs`
  (create/load/rotate/delete/sweep, create refused for a deleted org).
- [ ] 3. Lifecycle: `console_sessions` into `TENANT_TABLES` and `seed` in
  `crates/cxp-core/tests/lifecycle.rs` plus a member-removed test; fail; add it
  to `PURGED_TABLES`, delete in `member_removed`, add the sweep step.
- [ ] 4. `cxp_core::auth::authenticate_token` (split from `authenticate`,
  behaviour unchanged).
- [ ] 5. `crates/cxp-http/src/console.rs` (cookies, flow, `safe_next`, PKCE,
  token client, `Console`, `/auth/*` router) with unit tests first; the
  `Authenticator::with_console` opt-in, `Source`, and the origin check in
  `require_bearer` (unit tests for the origin matrix).
- [ ] 6. cxp-api: `AppState.console`, enable it on `/api/v1` only. Tests in
  `crates/cxp-api/tests/api.rs` (login flow, cookie read/write/origin, bearer
  precedence, expired/deleted session, refresh single-flight, invalid_grant,
  logout, disabled, session endpoint) written first against a stub platform with
  `/oauth/token` and `/oauth/revoke`.
- [ ] 7. cxp-mcp: add the live-session-cookie test in
  `crates/cxp-mcp/tests/mcp.rs`; the existing cookie test unchanged.
- [ ] 8. cxp-server: config keys (`CXP_CONSOLE_CLIENT_ID`, `CXP_ENCRYPTION_KEY`,
  `CXP_CONSOLE_DIR`) with tests; build the `Console`, mount `/auth`, the SPA
  fallback with tests (`/nope` JSON, HTML navigation `index.html`, reserved
  prefixes never HTML).
- [ ] 9. Image: `Dockerfile` console stage, `.dockerignore`; `podman build`.
- [ ] 10. Console: `apps/web` api-client credentials + 401 redirect, session
  hook, sign-in screen, sign-out, vite proxy `/auth`, PWA denylist, vitest
  tests (api-client, built service worker).
- [ ] 11. Docs: `docs/deploy/fly.md` (secret row, client registration, order),
  `.env.example`, `fly.toml`, CLAUDE.md config table and architecture line.
- [ ] 12. All gates green; commit `feat(console): sign the web console in with
  the otto platform` with `Fixes #14`.
