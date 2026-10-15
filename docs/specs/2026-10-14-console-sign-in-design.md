# The web console signs in with the otto platform

**Status:** DRAFT (#14)

## Brief

After #1 every `/api/v1` route needs a platform token, and the web console
(`apps/web`) calls them with no credential, so every call is `401`. The console
needs a sign-in: an OAuth authorization-code + PKCE flow against the platform
with a first-party public client (`CXP_CONSOLE_CLIENT_ID`), sessions held
server-side with the platform tokens sealed at rest (`CXP_ENCRYPTION_KEY`), a
session cookie `/api` accepts (with a same-origin check on writes), and the
binary serving the built console. Modeled on otto-factory's console
(`crates/of-web/src/routes/auth.rs`, `of-core/src/console_sessions.rs`).

## Premise corrections

- otto-factory's session lookup is an unpinned read of a policy branch that
  admits any unpinned transaction (`org_id = current_org() OR current_org() IS
  NULL`). That breaks this repo's invariant (unpinned access only with the
  maintenance marker). This design looks the session's org up through a
  `SECURITY DEFINER` function instead (see Invariants impact).
- otto-factory's sweep index on `expires_at` would be the third non-unique
  index that does not lead with `org_id`, which `tests/db.rs` forbids for a
  tenant table. There is no such index: the hourly sweep scans
  `console_sessions` (one row per signed-in browser, small) in bounded batches.
- The console is a single-org app with no org in its URLs (unlike
  otto-factory's `/o/{org}`). The org is whichever one the person picks on the
  platform's authorize page; switching org is signing out and in again.

## Assumptions

- The platform's endpoints are `/oauth/authorize`, `/oauth/token`, and
  `/oauth/revoke` (otto-platform at the pinned rev), access tokens live 60
  minutes, refresh tokens 30 days, and a refresh rotates the pair and revokes
  the family if a spent refresh token is replayed (as otto-factory relies on).
- The console asks for `cxp:read cxp:write`; the platform grants the subset the
  person may have, and the routes check the grant as they do for bearers.
- `CXP_PUBLIC_URL` is the console's origin too (same origin as `/api`), so the
  redirect URI is `{CXP_PUBLIC_URL}/auth/callback`.

## Goal & Success Criteria

- With `CXP_CONSOLE_CLIENT_ID` and `CXP_ENCRYPTION_KEY` set, a person opening the
  console is sent to the platform, signs in, and lands back with a session
  cookie; every `/api/v1` call then works from the browser.
- `/api` accepts the session cookie (opt-in for that surface only); a
  cookie-authenticated write must carry `Origin` equal to `CXP_PUBLIC_URL`'s
  origin (or, with no `Origin`, `Sec-Fetch-Site: same-origin`), else `403
  cross_site_request`. A bearer request is unaffected and wins over a cookie.
- `/mcp` stays bearer-only: `a_cookie_alone_is_not_a_credential_on_mcp` passes
  unchanged, and a new test proves a *live* session cookie is refused on `/mcp`.
- The org comes only from the stored platform token, introspected exactly like
  a bearer token (audience `CXP_RESOURCE_URI`, tombstones checked).
- Sessions are purged by `org.deleted` (the batched purge) and `member.removed`
  (the user's sessions in that org, first delivery only), and expired sessions by
  the hourly sweep in bounded batches.
- The binary serves `apps/web`'s build with an `index.html` fallback for
  browser navigations; `/api`, `/mcp`, `/auth`, `/.well-known`, `/platform`, and
  any non-HTML request keep the JSON `404`. The PWA service worker never
  answers `/auth`, `/api`, `/mcp`, or `/.well-known`.
- Merging with neither variable set deploys safely: console sign-in is off
  (`/auth/login` answers `503 console_login_disabled`), the key is not required,
  and everything else is unchanged.

## Scope

In: migration `0004_console_sessions.sql`; `cxp_core::console_sessions`;
`console_sessions` in the org purge, the member-removed cleanup, and the sweep;
`cxp_core::auth::authenticate_token` (the bearer path, split so a stored token
takes it too); `cxp_http::console` (cookies, PKCE flow, token endpoint client,
session resolution with single-flight refresh, `/auth/*` routes); the opt-in cookie
source and same-origin check on `cxp_http::Authenticator`; cxp-api enabling it;
cxp-server config (`CXP_CONSOLE_CLIENT_ID`, `CXP_ENCRYPTION_KEY`,
`CXP_CONSOLE_DIR`), `/auth` mounting, and SPA serving; the Dockerfile building
and shipping the console; `apps/web` sign-in, callback, logout, 401 handling;
the PWA denylist; docs (`fly.md`, `.env.example`, `fly.toml`, CLAUDE.md).

Out: org switching inside a session; a session list or "sign out everywhere";
rotating `CXP_ENCRYPTION_KEY` without signing everyone out; publishing anything.

## Architecture

```text
 browser                     otto-cxp                                 otto platform
   │ GET /auth/login?next=/x    │ state + PKCE verifier -> sealed cookie   │
   │◄─ 303 + __Host-cxp_oauth ──┤                                          │
   │ GET /oauth/authorize?client_id&code_challenge&resource&scope&state ──►│ sign in, pick org
   │◄──────────────────────────────────── 303 /auth/callback?code&state ───┤
   │ GET /auth/callback ───────►│ POST /oauth/token (code, verifier) ─────►│
   │                            │◄── access + refresh ─────────────────────┤
   │                            │ introspect (audience, org, user) + tombstones
   │                            │ store session (tokens sealed)            │
   │◄─ 303 next + __Host-cxp_session                                       │
   │ GET /api/v1/... (cookie) ─►│ cookie -> org (definer fn) -> pinned row  │
   │                            │ refresh if expiring (row lock) ─────────►│
   │                            │ introspect stored token, like a bearer ─►│
```

**Data** (`crates/cxp-core/migrations/0004_console_sessions.sql`,
`crates/cxp-core/src/console_sessions.rs`): `console_sessions (id_hash bytea
PK = SHA-256 of the cookie, org_id, user_id, access_token_enc,
refresh_token_enc bytea = nonce || AES-256-GCM ciphertext, access_expires_at,
created_at, expires_at)`, index `(org_id, user_id)`. The cookie is 32 random
bytes (base64url); only its hash is stored, so a dump cannot be replayed. Tokens
are sealed with `otto_tenant::crypto::Cipher` (the platform's shared AEAD
primitive, the same one otto-factory uses). Operations: `org_of` (the definer
function), `create` (through `begin_live`, so a sign-in racing an `org.deleted`
leaves nothing), `load` and `lock` (`FOR UPDATE`) pinned through `begin_live`,
`rotate`, `delete`, `delete_for_user`, `sweep_expired`.

**Lifecycle** (`crates/cxp-core/src/platform_events.rs`): `console_sessions`
joins `PURGED_TABLES` (`org_deleted`, :206); `member_removed` (:290) deletes the
user's sessions in the same pinned transaction as its tombstone, on the first
delivery only (a redelivery must not sign out a re-invited member);
`sweep` (:153) gains a `ConsoleSessions` step that deletes expired rows in
maintenance transactions of at most `SESSION_SWEEP_BATCH` rows each.

**HTTP** (`crates/cxp-http`):

- `console::Console` holds the database, platform client, a token-endpoint
  client (`reqwest`, 10 s timeout, no redirects), the cipher, and its settings.
  `Console::authenticate(cookie)` gets a platform access token good for this
  request and authenticates it like a bearer: `org_of` -> pinned `load` -> if it expires within 60 s, a refresh in a
  spawned task under the row lock (re-checked after the lock: a waiter finds the
  winner's token and refreshes nothing; the task outlives a cancelled request,
  because a rotated pair must be stored). `invalid_grant` deletes the session
  (`401`); an unreachable platform leaves it untouched (`503`).
- `Authenticator::with_console(console)` is the opt-in. `authenticate_request`
  returns the principal and its `Source`. **A bearer, when present, decides the
  request** (a cookie never rescues a bad bearer). Only with no bearer and the
  console enabled is the cookie read; its token goes through
  `cxp_core::auth::authenticate_token`, the same introspection + tombstone path
  as a bearer. A token the platform calls inactive drops the session.
- `require_bearer` refuses a cookie-authenticated non-`GET`/`HEAD`/`OPTIONS`
  request whose `Origin` is not the public origin (or, with no `Origin`, whose
  `Sec-Fetch-Site` is not `same-origin`): `403 cross_site_request`, before the
  handler runs. No `Referer` fallback: browsers send `Origin` on every
  cross-origin and every non-GET fetch.
- `console::router(Option<Console>, public_url)`: `GET /auth/login`, `GET
  /auth/callback`, `POST /auth/logout` (same-origin check, revokes the refresh
  token best effort, deletes the row, clears the cookie, `204`), `GET
  /auth/session` (`200 {signedIn, loginEnabled, userId, orgId, role, scopes}`,
  `no-store`; `503` only when the answer cannot be known). Login and callback failures redirect to `/?login_error=<code>`
  from a fixed vocabulary; nothing from the query string is reflected. A `next`
  path is accepted only if it is certainly a same-origin path (otto-factory's
  `safe_next`).
- Cookies: `__Host-cxp_session` and `__Host-cxp_oauth` (the in-flight flow,
  sealed, 10 minutes), `HttpOnly; Secure; SameSite=Lax; Path=/`, no `Domain`.
  On a plain-http loopback public URL (local dev) the names drop `__Host-` and
  the `Secure` flag, since `__Host-` requires `Secure`. `Lax` not `Strict`: the
  callback is a cross-site top-level navigation back from the platform, and
  must carry the oauth cookie; `Lax` still keeps the session cookie off
  cross-site subresource and POST requests, and the origin check covers
  same-site siblings (`*.savvagent.com`).

**cxp-api** (`crates/cxp-api/src/lib.rs`, `router`): `AppState` carries an
optional `Console`; the `/api/v1` layer's `Authenticator` calls
`.with_console(..)` when it is set. Nothing else in cxp-api changes. cxp-mcp's
`Authenticator` never calls it.

**cxp-server**: `Config` gains `console: Option<ConsoleConfig {client_id,
encryption_key}>` and `console_dir`. `CXP_CONSOLE_CLIENT_ID` set without
`CXP_ENCRYPTION_KEY` is a boot error naming the key; neither set means the
console is off and the key is not read; a key that is set must decode to 32
bytes either way. The console also refuses a plain-http public URL that is not
loopback. `router` builds the `Console`, mounts `/auth/*`, and replaces the
fallback with `console_or_not_found`: a `GET`/`HEAD` for a non-reserved path
serves a file from `CXP_CONSOLE_DIR` if one exists, and `index.html` if the
request `Accept`s `text/html`; everything else is the JSON `404` as today.
`CXP_CONSOLE_DIR` defaults to `apps/web/dist` (a `cargo run` from the repo
root); the image sets `/srv/console` (and `fly.toml` states it). Console
responses carry `Cache-Control` (`immutable` for hashed `/assets/`, `no-cache`
otherwise, so a deploy is seen at once), `X-Content-Type-Options: nosniff`, and
`X-Frame-Options: DENY` plus `frame-ancestors 'none'` (the console acts with an
ambient cookie, so it is never framed). With no `index.html` there, the SPA is
off (logged at `warn`) and every unknown path is the JSON `404`.

**Image** (`Dockerfile`): a `console` stage on `node:22-bookworm-slim` (pinned by
digest) installs pnpm 9 through corepack, installs only `@cxp/web`'s
dependencies from the lockfile, builds it, and the runtime copies `dist` to
`/srv/console`.

**Console** (`apps/web`): `apiFetch` sends `credentials: "same-origin"` and, on
`401`, sends the browser to `/auth/login?next=<current path>`. A `useSession`
hook reads `/auth/session`; the root layout shows a sign-in screen when signed
out (or the `login_error` reason), and the header gets a sign-out button
(`POST /auth/logout`). The vite dev proxy forwards `/auth` too. vite-plugin-pwa:
`navigateFallbackDenylist` covers `/auth`, `/api`, `/mcp`, `/.well-known`,
`/platform`, and a vitest test checks the denylist (`src/pwa.test.ts`) and, after a
build, that the generated `dist/sw.js` carries it
(`test/service-worker.test.ts`, outside `src/` because `tsc` checks `src/`
without Node's types).

## Invariants impact

- **Tenant isolation is RLS, proven.** `console_sessions` has `org_id NOT NULL`,
  RLS enabled and forced, the policy `console_sessions_tenant_isolation`
  (`org_id = current_org()`, plus the maintenance-marker branch for the sweep's
  `DELETE`; `WITH CHECK` is pinned-only), grants to `otto_app`, and is in the
  tenant-isolation and cross-org tests.
- **Looking a session up before its org is known** is the one place a request
  reads this table unpinned. Options considered:
  1. otto-factory's policy branch for any unpinned transaction: rejected, it is
     exactly the "forgot to pin" hole the maintenance marker closes.
  2. `cxp_core::maintenance::begin` on the request path: rejected, the marker is
     reserved for background code.
  3. A non-RLS side table mapping hash -> org: workable, but a second table to
     keep consistent with the first.
  4. **Chosen:** `console_session_org(bytea) RETURNS uuid`, `SECURITY DEFINER`,
     `SET search_path = pg_catalog, public`, `REVOKE ALL ... FROM PUBLIC`, then
     `GRANT EXECUTE ... TO otto_app`. It returns only the org of the one
     unexpired row whose hash is given (a 256-bit secret's hash), never a token.
     Its owner (the migrating role) bypasses RLS today as a superuser; for a
     least-privilege owner (#16) a `SELECT`-only policy `console_sessions_lookup`
     admits an unpinned read only while `app.console_session_lookup = 'on'`,
     which the function's own `SET` clause sets for the duration of the call and
     nothing else sets. The full row (tokens included) is then read pinned
     through `begin_live`. Tests prove an unpinned `otto_app` without that
     setting sees nothing, and the function's `prosecdef`, `proconfig`, and
     grants.
- **Request paths use `begin_live`:** session create, load, lock, rotate, and
  delete all do; the sweep uses `cxp_core::maintenance`.
- **No SQL outside cxp-core:** cxp-http calls `cxp_core::console_sessions`.
- **Auth:** the cookie's token is introspected with the configured audience and
  checked against the tombstones, the same function as a bearer. Scope checks
  are unchanged. `/mcp` stays bearer-only.
- **Composite FKs:** none; the table references nothing.
- **Untrusted text:** login errors are a fixed vocabulary; nothing from the
  callback's query string or the platform's error body reaches a response.
- **Small shared DB:** one indexed row read per cookie request (plus one
  function call); refresh at most once an hour per session, under a row lock
  held for one platform call (10 s timeout); sweep and purge are batched.
- **Metering:** nothing here meters.
- **Licensing:** AGPL headers/fields as siblings (crates and apps).

## Migration

`0004_console_sessions.sql` (0003 is #17's; until #17 merges, this branch alone
fails `migrate::tests::versions_are_contiguous_from_one`, so it merges after
#17, rebased onto it): table, index, RLS, two policies,
the function, grants. Additive only: the previous release ignores the table and
the function, so a rollback boots.

## Out-of-band

Human (Rob), **before** the release that turns the console on; merging this PR
alone turns nothing on:

1. Register the client on the platform:
   `fly ssh console -a otto-platform -C 'otto-platform-server client register --name "otto-cxp console" --redirect-uri https://otto-cxp.savvagent.com/auth/callback --first-party'`
   (prints the `client_id`).
2. `fly secrets set -a otto-cxp --stage CXP_ENCRYPTION_KEY="$(openssl rand -base64 32)"`.
3. Set `CXP_CONSOLE_CLIENT_ID` in `fly.toml` `[env]` (a follow-up commit; it is
   not a secret, and `[env]` is where every non-secret setting lives) and merge.
   Doing 3 before 2 fails the boot with an error naming the key, so the order
   matters.

## Error handling & edge cases

- Platform down during login: redirect `platform_unavailable`; during refresh:
  `503`, session untouched; on logout: revoke skipped (logged), row deleted.
- `invalid_grant` on refresh: session deleted, `401` (the console signs in
  again). `invalid_client`/other refusals: logged at `error` (registration or
  config is wrong), `500` on refresh, `failed` on login.
- A stored token the platform calls inactive: session deleted, `401`.
- A token that will not open (key rotated): session deleted, `401`.
- Org deleted or member removed during sign-in: `create` is refused by
  `begin_live`; login redirects `failed`.
- `CXP_CONSOLE_CLIENT_ID` unset while sessions exist: no `Console` is built, so
  the cookie is no credential at all (`401`); turning it back on revives the
  sessions that have not expired.
- A malformed cookie (not 43 base64url characters) is never looked up.

## Testing approach

- `cxp-core/tests/db.rs`: `console_sessions` in the isolation report and the
  cross-org test (seeded); unpinned `otto_app` sees nothing without the lookup
  setting; the function returns the org for a live hash and nothing for an
  unknown or expired one; its definer/search_path/grant properties; the sweep
  needs the maintenance marker.
- `cxp-core/tests/console_sessions.rs`: seal/open round trip through the table,
  create refused for a deleted org, rotate, delete, sweep in batches.
- `cxp-core/tests/lifecycle.rs`: `TENANT_TABLES` gains `console_sessions` (so
  the purge must cover it); `member.removed` deletes only that user's sessions
  in that org, only on first delivery.
- `cxp-http` unit tests: cookie parsing and attributes, `safe_next`, PKCE
  vector, flow seal/open/expiry, origin check matrix.
- `cxp-api/tests/api.rs` (stub platform gains `/oauth/token` and
  `/oauth/revoke`): the full login (redirect, callback, cookie), cookie `GET`
  works, cookie write with no / wrong `Origin` is `403`, with the right `Origin`
  works, a bearer still works (and decides over a cookie), a deleted or expired
  session is `401`, refresh rotates once under concurrency, `invalid_grant`
  drops the session, logout, the login-disabled `503`, and `/auth/session`.
- `cxp-mcp/tests/mcp.rs`: a live session cookie alone on `/mcp` is `401`
  (the existing test is untouched).
- `cxp-server`: config parsing (console off/on, key required only with the
  client id, key length), the SPA fallback (HTML navigation gets `index.html`,
  `/nope` without `Accept: text/html` stays the JSON `404`, `/api/...` and
  `/auth/...` never get HTML), `/auth/login` disabled by default.
- `apps/web`: `apiFetch` sends credentials and redirects on `401`; the built
  service worker's denylist covers the reserved prefixes.

## Risks & open questions

- Rotating `CXP_ENCRYPTION_KEY` signs every console user out (sessions that do
  not open are dropped). Documented in `fly.md`.
- The refresh holds one pooled connection for the duration of one platform
  call. With five connections that is acceptable for a console used by a few
  people; revisit if console traffic grows.
- The sweep scans the table without an index; fine while sessions number in the
  thousands. An `(expires_at)` index would need an exception in `tests/db.rs`.
