# Console sign-in plan

Spec: `docs/specs/2026-10-14-console-sign-in-design.md`. Issue #14.

Gates (run in the worktree, against its own Postgres):
`DATABASE_URL=postgres://cxp:cxp@localhost:<port>/otto_cxp cargo test --workspace --locked`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo fmt --all -- --check`, the migrations-append-only diff,
`pnpm install --frozen-lockfile && pnpm lint && pnpm build && pnpm test`,
`podman build -t otto-cxp:check .`.

- [x] 1. Schema. Add `console_sessions` to `BOOKKEEPING_TABLES` and `seed` in
  `crates/cxp-core/tests/db.rs`, and the lookup/sweep tests; see them fail.
  Write `crates/cxp-core/migrations/0003_console_sessions.sql`; see them pass.
- [x] 2. `crates/cxp-core/src/console_sessions.rs` (+ `lib.rs`, `Cargo.toml`
  deps `sha2`): tests first in `crates/cxp-core/tests/console_sessions.rs`
  (create/load/rotate/delete/sweep, create refused for a deleted org).
- [x] 3. Lifecycle: `console_sessions` into `TENANT_TABLES` and `seed` in
  `crates/cxp-core/tests/lifecycle.rs` plus a member-removed test; fail; add it
  to `PURGED_TABLES`, delete in `member_removed`, add the sweep step.
- [x] 4. `cxp_core::auth::authenticate_token` (split from `authenticate`,
  behaviour unchanged).
- [x] 5. `crates/cxp-http/src/console/` (cookies, flow, `safe_next`, PKCE,
  token client, `Console`, `/auth/*` router) with unit tests first; the
  `Authenticator::with_console` opt-in, `Source`, and the origin check in
  `require_bearer` (unit tests for the origin matrix).
- [x] 6. cxp-api: `AppState.console`, enable it on `/api/v1` only. Tests in
  `crates/cxp-api/tests/api.rs` (login flow, cookie read/write/origin, bearer
  precedence, expired/deleted session, refresh single-flight, invalid_grant,
  logout, disabled, session endpoint) written first against a stub platform with
  `/oauth/token` and `/oauth/revoke`.
- [x] 7. cxp-mcp: add the live-session-cookie test in
  `crates/cxp-mcp/tests/mcp.rs`; the existing cookie test unchanged.
- [x] 8. cxp-server: config keys (`CXP_CONSOLE_CLIENT_ID`, `CXP_ENCRYPTION_KEY`,
  `CXP_CONSOLE_DIR`) with tests; build the `Console`, mount `/auth`, the SPA
  fallback with tests (`/nope` JSON, HTML navigation `index.html`, reserved
  prefixes never HTML).
- [x] 9. Image: `Dockerfile` console stage, `.dockerignore`; `podman build`.
- [x] 10. Console: `apps/web` api-client credentials + 401 redirect, session
  hook, sign-in screen, sign-out, vite proxy `/auth`, PWA denylist, vitest
  tests (api-client, built service worker).
- [x] 11. Docs: `docs/deploy/fly.md` (secret row, client registration, order),
  `.env.example`, `fly.toml`, CLAUDE.md config table and architecture line.
- [x] 12. All gates green; commit `feat(console): sign the web console in with
  the otto platform` with `Fixes #14`.

## Review round (PR #30)

- [x] 13. Refresh without holding anything: migration columns
  `refresh_lease_until` and `generation`; `claim_refresh`, `store_refreshed`
  (compare-and-swap), `release_lease`, `delete_generation`, `take` in
  `cxp_core::console_sessions` with tests; the in-process single flight and
  bounded lease polling in `cxp_http::console`; tests with an app pool of five
  (eight concurrent requests, one refresh), a waited-out lease, and an
  `org.deleted` webhook during a slow refresh.
- [x] 14. Logout and re-sign-in: delete and commit, then revoke with nothing
  held; a callback ends the browser's previous session.
- [x] 15. Migration renamed to `0003`; the lookup policy admits the function's
  current owner (read from `pg_proc`, no role list); the non-superuser owner test in `tests/db.rs` (roles dropped whatever the
  test does).
- [x] 16. The console under `/console/` (Vite base, router basename, PWA scope,
  `safe_next`, the binary's fallback, `/` left to #29), CSP, HSTS, dotfiles,
  file-server redirects and `5xx` as JSON.
- [x] 17. HKDF subkeys (`ConsoleKeys`), the `cxp_core::sealed` helper, the
  session bound to its user, short-lived tokens clamped, no `expect` in cookie
  building, the dead sweep arm removed.
- [x] 18. Web client: no navigation from `apiFetch`, no retry on `401`/`403`,
  sign-out errors toasted, zod session, a11y, the service-worker test under
  `tsc`.
- [x] 19. Docs: CLAUDE.md (Dockerfile gate paths, Auth invariant, security
  focus, Build & Run, the lookup policy), `docs/deploy/fly.md`, this plan and
  the spec; `CXP_CONSOLE_DIR` set only in the Dockerfile.

## Review round 2 (PR #30)

- [x] 20. The lookup policy follows the function's owner (`pg_proc`), the
  function created first; the non-superuser test moves ownership with
  `ALTER ... OWNER TO` alone; the `ALTER POLICY` notes removed.
- [x] 21. The lease on the database's clock, 45 s; store and release match the
  claimed lease; no platform call unless the call and the store fit in what is
  left; tests for a lapsed lease taken over and a stale holder changing
  nothing.
- [x] 22. The single-flight entry freed by a drop guard, with a panic test.
- [x] 23. Suggestions: an unsaved pair revoked on a store error, encoded
  slashes refused, `nosniff` on every response, `SKIP LOCKED` in the sweep,
  the grant and module-doc comments.
