# The web console signs in with the otto platform

**Status:** IMPLEMENTED (#30)

## Brief

After #1 every `/api/v1` route needs a platform token, and the web console
(`apps/web`) calls them with no credential, so every call is `401`. The console
needs a sign-in: an OAuth authorization-code + PKCE flow against the platform
with a first-party public client (`CXP_CONSOLE_CLIENT_ID`), sessions held
server-side with the platform tokens sealed at rest (an HKDF subkey of
`CXP_ENCRYPTION_KEY`), a session cookie `/api` accepts (with a same-origin
check on writes), and the binary serving the built console under `/console/`. Modeled on otto-factory's console
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
- `otto_tenant::crypto::Cipher` takes no associated data, so a sealed token is
  not bound to its row (a sealed value copied between rows by someone who can
  already write the table would open). Binding it needs AAD support in
  otto-platform, out of scope here; the row's org and user are checked against
  the token's introspection on every use instead.

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
- The binary serves `apps/web`'s build under `/console/` only, with an
  `index.html` fallback for browser navigations there (`/console` redirects to
  `/console/`). `/`, every other unknown path, and any non-HTML miss keep the
  JSON `404`: PR #29 (the connector install page, #25) claims `/`, so neither
  PR collides with the other whichever merges first. The PWA's scope is
  `/console/`, and its service worker never answers `/auth`, `/api`, `/mcp`,
  `/.well-known`, or `/platform`.
- Merging with neither variable set deploys safely: console sign-in is off
  (`/auth/login` answers `503 console_login_disabled`), the key is not required,
  and everything else is unchanged.

## Scope

In: migration `0003_console_sessions.sql`; `cxp_core::console_sessions`;
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
   │ GET /auth/login?next=/console/x  state + PKCE verifier -> sealed cookie
   │◄─ 303 + __Host-cxp_oauth ──┤                                          │
   │ GET /oauth/authorize?client_id&code_challenge&resource&scope&state ──►│ sign in, pick org
   │◄──────────────────────────────────── 303 /auth/callback?code&state ───┤
   │ GET /auth/callback ───────►│ POST /oauth/token (code, verifier) ─────►│
   │                            │◄── access + refresh ─────────────────────┤
   │                            │ introspect (audience, org, user) + tombstones
   │                            │ store session (tokens sealed)            │
   │◄─ 303 next (under /console/) + __Host-cxp_session                     │
   │ GET /api/v1/... (cookie) ─►│ cookie -> org (definer fn) -> pinned row  │
   │                            │ refresh if expiring: lease, call with ───►│
   │                            │ nothing held, compare-and-swap store     │
   │                            │ introspect stored token, like a bearer ─►│
```

**Data** (`crates/cxp-core/migrations/0003_console_sessions.sql`,
`crates/cxp-core/src/console_sessions.rs`): `console_sessions (id_hash bytea
PK = SHA-256 of the cookie, org_id, user_id, access_token_enc,
refresh_token_enc bytea = nonce || AES-256-GCM ciphertext, access_expires_at,
created_at, expires_at, refresh_lease_until, generation)`, index `(org_id,
user_id)`. The cookie is 32 random bytes (base64url); only its hash is stored,
so a dump cannot be replayed. Tokens are sealed with `otto_tenant::crypto::Cipher`
(the platform's shared AEAD primitive, the same one otto-factory uses) under
the session-token subkey, in the one `nonce || ciphertext` layout
`cxp_core::sealed` checks. Operations: `org_of` (the definer function),
`create` (through `begin_live`, so a sign-in racing an `org.deleted` leaves
nothing), `load`, `claim_refresh`, `store_refreshed`, `release_lease`,
`delete_generation`, `take` (sign-out), `delete`, all pinned through
`begin_live` in short transactions; `delete_for_user`, `sweep_expired`.

**Lifecycle** (`crates/cxp-core/src/platform_events.rs`): `console_sessions`
joins `PURGED_TABLES` (`org_deleted`, :206); `member_removed` (:290) deletes the
user's sessions in the same pinned transaction as its tombstone, on the first
delivery only (a redelivery must not sign out a re-invited member);
`sweep` (:153) gains a `ConsoleSessions` step that deletes expired rows in
maintenance transactions of at most `SESSION_SWEEP_BATCH` rows each.

**HTTP** (`crates/cxp-http`):

- `console::Console` (`crates/cxp-http/src/console/{mod,cookies,oauth,routes}.rs`)
  holds the database, platform client, a token-endpoint client (`reqwest`, 10 s
  timeout, no redirects), the two subkeys, and its settings.
  `Console::authenticate(cookie)` gets a platform access token good for this
  request and authenticates it like a bearer, then requires the token's org
  and user to be the session's: `org_of` -> pinned `load` -> if it expires
  within 60 s (or half way through a token shorter-lived than two minutes), a
  refresh. **The refresh holds no database resource across the platform
  call** (the pool is five connections for every org, and `org.deleted` takes
  the org's lifecycle lock exclusively):
  1. In-process single flight: one refresh per session runs in a spawned task
     (it outlives a cancelled request, because a rotated pair must be stored);
     every other request for that session awaits the same shared result,
     holding no connection.
  2. Lease: `claim_refresh` sets `refresh_lease_until` to the database's
     `clock_timestamp()` + 45 s and reads the sealed refresh token in one short
     transaction, returning the lease it took (generation and expiry), or
     answers *fresh* (another refresh won), *leased* (another replica is
     refreshing), or *gone*. The lease is set and compared on the database's
     clock only; the app's clock decides only whether a token needs a refresh.
  3. The platform call, nothing held, made only if the claim left room for it:
     `elapsed since the claim began + 10 s platform timeout + 20 s store
     allowance <= 45 s`; otherwise the lease is given back unspent and the
     request is `503`.
  4. `store_refreshed`: a second short transaction writes the new pair only
     where the row still holds the claimed lease (generation and
     `refresh_lease_until`), bumping the generation and clearing the lease.
     A sign-out, a purge, or a newer claim after a lapse wins: nothing is
     written, the new pair is revoked, and the request is `401`.
     `release_lease` matches the same pair, so a stale holder never frees a
     newer claim. A database error on the store also revokes the new pair; the
     stored refresh token is then spent, and the next claim's `invalid_grant`
     signs the person out.
  The arithmetic is in the `cxp_http::console` module docs: the holder's call
  and store fit in its lease, so no other replica can claim and present the
  same refresh token while the holder could still store its result. A
  replica that finds a live lease polls the row every 250 ms, at most 190
  times (47.5 s, past one lease, so a dead holder's lease lapses and is taken
  over), then answers `503`. The refresh task frees its single-flight entry
  from a drop guard, so a panic in it does not leave the session on `503`. `invalid_grant` deletes the session at that generation
  (`401`); an unreachable platform gives the lease back and leaves the session
  untouched (`503`).
- Keys: `CXP_ENCRYPTION_KEY` is never used directly. `ConsoleKeys::derive`
  takes two subkeys with HKDF-SHA256 (no salt; `info`
  `cxp.console.session-tokens.v1` for the stored tokens,
  `cxp.console.oauth-flow.v1` for the oauth cookie), so a value sealed for one
  use never opens as the other.
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
- `console::router(Option<Console>)`: `GET /auth/login`, `GET
  /auth/callback` (a browser that already held a session has it ended: deleted,
  and its refresh token revoked), `POST /auth/logout` (same-origin check;
  deletes the row and commits, then revokes the refresh token best effort with
  nothing held; clears the cookie; `204`), `GET /auth/session` (`200 {signedIn,
  loginEnabled, userId, orgId, role, scopes}`, `no-store`; `503` only when the
  answer cannot be known). Login and callback failures redirect to
  `/console/?login_error=<code>` from a fixed vocabulary; nothing from the query
  string is reflected. A `next` path is accepted only under `/console/`, with
  no `\`, control or whitespace character, or `.`/`..` segment (plain or
  percent-encoded); the default landing is `/console/`.
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
fallback with `console_or_not_found`, which serves only under `/console/`: a
`GET`/`HEAD` there is a file from `CXP_CONSOLE_DIR` if one exists, and
`index.html` if the request `Accept`s `text/html`; `/console` redirects (`308`)
to `/console/`. A dotfile or dot segment is never served: the check runs on
the percent-decoded path split on `/` and `\`, so `x%2F.env` is refused as
`x/.env` is, and a path that does not decode is refused; directories are neither listed nor redirected, and a file-server
redirect or `5xx` becomes the JSON error shape. Everything else, `/` included,
is the JSON `404` as today. `CXP_CONSOLE_DIR` defaults to `apps/web/dist` (a
`cargo run` from the repo root, after `pnpm build`); the image's Dockerfile
`ENV` sets `/srv/console`, the one place it is set (a deployment's own
environment would win). Console responses carry `Cache-Control` (`immutable`
for hashed `/console/assets/`, `no-cache` otherwise, so a deploy is seen at
once), `X-Frame-Options: DENY`,
`Referrer-Policy: same-origin`, and the CSP `default-src 'self'; script-src
'self'; connect-src 'self'; style-src 'self' 'unsafe-inline'; object-src
'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'` (inline
styles only because the toast and dialog libraries inject `<style>` elements;
no inline script). Every response, JSON included, carries
`X-Content-Type-Options: nosniff`, and `Strict-Transport-Security:
max-age=31536000` when the public URL is `https`. With no `index.html` there,
the console is off (logged at `warn`) and `/console/` is the JSON `404`.

**Image** (`Dockerfile`): a `console` stage on `node:22-bookworm-slim` (pinned by
digest) installs pnpm 9 through corepack, installs only `@cxp/web`'s
dependencies from the lockfile, builds it, and the runtime copies `dist` to
`/srv/console`.

**Console** (`apps/web`): built with Vite `base: '/console/'` and a router
`basename` of `/console`. `apiFetch` sends `credentials: "same-origin"`; on
`401` it never navigates: it marks the session query signed out, and the root
layout shows the sign-in screen (so a session the server signs out, for
example a locally revoked member, cannot loop through a silent platform
sign-in). Queries are not retried on `401` or `403`. A `useSession` hook reads
`/auth/session`, parsed with zod as a union discriminated on `signedIn`; the
root layout shows a sign-in screen (`<main>`, `<h1>`, the `login_error`
reason) when signed out, and `aria-busy` loading and error states with a retry
button. The header's sign-out button `POST`s `/auth/logout`, and on a non-ok
answer or a network error shows a toast and stays signed in. The vite dev proxy
forwards `/auth` too. vite-plugin-pwa: `scope` and `start_url` `/console/`,
`navigateFallback` `/console/index.html` allowed only under `/console/`, and
`navigateFallbackDenylist` covering `/auth`, `/api`, `/mcp`, `/.well-known`,
`/platform`; a vitest test checks the lists (`src/pwa.test.ts`) and, after a
build, that the generated `dist/sw.js` carries them
(`test/service-worker.test.ts`, type-checked by `tsc -b` through
`tsconfig.test.json` with Node's types).

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
     which the function's own `SET` clause sets for the duration of the call,
     and only when `current_user` is the function's owner, read at query time
     (`pg_get_userbyid(proowner)` from `pg_proc` for
     `'public.console_session_org(bytea)'::regprocedure`). The policy names no
     role, so it follows the function through `ALTER FUNCTION ... OWNER TO`
     and #26's `REASSIGN OWNED` with nothing else to move, and another role,
     `otto_app` included, gains nothing by setting the marker itself. The
     function is created before the policy, which names it. The full row
     (tokens included) is then read pinned through `begin_live`. Tests prove an
     unpinned `otto_app` sees nothing with or without that setting, the
     function's `prosecdef`, `proconfig`, and grants, and, after a
     `NOSUPERUSER NOBYPASSRLS` role takes over the table and the function with
     `ALTER ... OWNER TO` alone (no `ALTER POLICY`), that the lookup works
     through the policy, that role's own plain `SELECT` sees nothing,
     `otto_app` setting the marker by hand sees nothing, and with the policy
     dropped the lookup finds nothing.
- **Request paths use `begin_live`:** session create, load, claim, store,
  release, and delete all do, each in a short transaction; the sweep uses
  `cxp_core::maintenance`.
- **No SQL outside cxp-core:** cxp-http calls `cxp_core::console_sessions`.
- **Auth:** the cookie's token is introspected with the configured audience and
  checked against the tombstones, the same function as a bearer. Scope checks
  are unchanged. `/mcp` stays bearer-only.
- **Composite FKs:** none; the table references nothing.
- **Untrusted text:** login errors are a fixed vocabulary; nothing from the
  callback's query string or the platform's error body reaches a response.
- **Small shared DB:** one indexed row read per cookie request (plus one
  function call); refresh at most once an hour per session, two short
  transactions with no connection or lock held across the platform call; sweep
  and purge are batched.
- **Metering:** nothing here meters.
- **Licensing:** AGPL headers/fields as siblings (crates and apps).

## Migration

`0003_console_sessions.sql`: table, index, RLS, the function, two policies
(the lookup one admits the function's current owner), grants. Whichever of this PR and #17
merges first takes 0003; the other renumbers before it merges (an unmerged
migration may be renamed). Additive only: the previous release ignores the
table and the function, so a rollback boots.

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
  `503`, session untouched and its lease given back; on logout: row deleted,
  revoke skipped (logged).
- Signed out or purged while a refresh waited on the platform: the
  compare-and-swap writes nothing, the new pair is revoked, `401`.
- Another replica holds the lease: poll for its result; `503` if it never
  comes (its lease lapses first and is taken over).
- A stored token whose introspected org or user is not the session's: session
  deleted, `401`.
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
  create refused for a deleted org, the lease (claim, live lease on the
  database's clock whatever the caller's clock says, lapsed lease taken over,
  release, a stale holder neither releasing nor storing over a newer claim, a
  lapsed but unclaimed lease still storing) and the compare-and-swap store (a
  spent lease and a deleted session write nothing), `take`, sweep in batches.
- `cxp-core/tests/lifecycle.rs`: `TENANT_TABLES` gains `console_sessions` (so
  the purge must cover it); `member.removed` deletes only that user's sessions
  in that org, only on first delivery.
- `cxp-http` unit tests: cookie parsing and attributes, `safe_next`, PKCE
  vector, flow seal/open/expiry, origin check matrix.
- `cxp-api/tests/api.rs` (stub platform gains `/oauth/token` and
  `/oauth/revoke`): the full login (redirect, callback, cookie), cookie `GET`
  works, cookie write with no / wrong `Origin` is `403`, with the right `Origin`
  works, a bearer still works (and decides over a cookie), a deleted or expired
  session is `401`, eight concurrent requests at refresh time through an app
  pool capped at five connections all succeed with exactly one platform
  refresh, a request waits out another replica's lease, an `org.deleted`
  webhook is not delayed by a slow (4 s) platform refresh and the refresh then
  stores nothing and revokes its pair, a token for another user is refused,
  signing in again ends the previous session, `invalid_grant` drops the
  session, logout, the login-disabled `503`, and `/auth/session`.
- `cxp-mcp/tests/mcp.rs`: a live session cookie alone on `/mcp` is `401`
  (the existing test is untouched).
- `cxp-server`: config parsing (console off/on, key required only with the
  client id, key length), the console under `/console/` (HTML navigation gets
  `index.html`, `/console` redirects, `/` and every other path stay the JSON
  `404`, dotfiles and directories are never served, encoded slashes included,
the CSP and HSTS headers, `nosniff` on every response),
  `/auth/login` disabled by default.
- `apps/web`: `apiFetch` sends credentials and marks the session signed out on
  `401` without navigating; no retry on `401`/`403`; sign-out failures toast;
  the session schema; the built service worker's fallback, allowlist, and
  denylist.

## Risks & open questions

- Rotating `CXP_ENCRYPTION_KEY` signs every console user out (sessions that do
  not open are dropped). Documented in `fly.md`.
- A replica that dies after the platform rotated a pair but before storing it
  leaves the session's refresh token spent; the next refresh gets
  `invalid_grant` and the person signs in again. Accepted: it needs a crash in a
  window of milliseconds.
- The sweep scans the table without an index; fine while sessions number in the
  thousands. An `(expires_at)` index would need an exception in `tests/db.rs`.
