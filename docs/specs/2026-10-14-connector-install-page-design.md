# A connector install page at `/` for browsers

**Status:** IMPLEMENTED (#29)
**Ticket:** #25

## Brief

From #25 (the ticket text is the contract; quoted in summary, AC verbatim):

> People who click "Open console" on otto.savvagent.com land on https://otto-cxp.savvagent.com/,
> which answers with a raw JSON 404 (the router fallback `not_found` in
> `crates/cxp-server/src/lib.rs`). otto-cxp has no web console; people use it from an AI assistant
> (Claude, ChatGPT, Claude Code) as a custom connector, and nothing at the origin tells them how.

Acceptance criteria:

- Given a browser `Accept` (`text/html,application/xhtml+xml,...`), `GET /` returns 200 HTML with
  the headers above and the configured connector address.
- `GET /` with `Accept: */*` (curl's default) or `application/json` returns the existing JSON 404.
- `GET /` with an HTML `Accept` plus an `Authorization` header returns the JSON 404.
- `HEAD /` with an HTML `Accept` returns 200 with no body.
- An unknown path with an HTML `Accept` still returns the JSON 404.
- Unauthenticated `/mcp` still returns 401 with `WWW-Authenticate` naming
  `/.well-known/oauth-protected-resource`.
- A unit test proves the CSP hashes match the embedded style and script.
- All gates green; after deploy (if live), `curl -s -H 'Accept: text/html' https://otto-cxp.savvagent.com/`
  returns the page and `curl -s https://otto-cxp.savvagent.com/` returns the JSON 404.

## Premise corrections

- None to the behaviour. One wiring note: registering `.route("/", get(..))` would make
  `POST /` (and every other method on `/`) a `405` instead of today's JSON 404, which the brief
  forbids ("every other request to `/` keeps the current JSON 404"). The page is therefore served
  from the router's fallback, not from a route, so no method on `/` changes except a
  page-eligible `GET`/`HEAD`.

## Assumptions

- **Both links derive from `CXP_PLATFORM_URL`'s origin:** "Full setup guide" is
  `{origin}/products/cxp#connect` and "Open your Otto console" is `{origin}/`. In production the
  origin is `https://otto.savvagent.com`, giving exactly the brief's URLs.
- **Only an exact `text/html` range counts.** `text/*` and `*/*` do not (curl sends `*/*`; an agent
  that wants HTML says so). Media types and parameter names compare case-insensitively; a `q`
  that does not parse as a number in 0..=1 makes that range not count (falls back to JSON, the
  conservative answer). Several `Accept` headers are read together.
- **Any `Authorization` header** (even empty or non-Bearer) means JSON: a caller presenting
  credentials is a program.
- **The JSON 404 stays identical, headers included.** It gets no `Vary`. The page response carries
  `Vary: Accept, Authorization`, so no cache reuses the page for a JSON request; the JSON 404 has
  no freshness information, so no cache reuses it for a browser. *(Superseded by
  `2026-10-15-root-404-vary-design.md` (#32): the JSON 404 for `GET`/`HEAD /` now carries the
  same `Vary`.)*
- **`HEAD`** returns the page's headers (including `Content-Length` of the GET body) and an empty
  body, built explicitly: fallback handlers get no automatic HEAD body stripping in tests.
- **Copy buttons are progressive enhancement:** rendered `hidden`, unhidden by the script. Without
  JavaScript the address is still selectable text.
- **Two copy targets:** the connector address and the Claude Code command; each has its own
  `role="status"` region. A click clears the region, then writes the target element's trimmed
  `textContent` with `navigator.clipboard.writeText`; on success the region reads `Copied.`; if
  the clipboard is missing or refuses (a rejected promise or a thrown error), the script selects
  the target's text with a DOM `Range` and the region reads `Selected. Press Ctrl+C to copy.`
  Both messages live in the one hashed script; selection by script needs no CSP allowance.
- **Hashes are hard-coded in the CSP constant** and proven by a unit test (as the brief asks), not
  computed at boot. `sha2 = "0.10"` and `base64 = "0.22"` are added to the root
  `[workspace.dependencies]` (the versions already locked through other crates, 0.10.9 and
  0.22.1) and used as dev-dependencies of `cxp-server` only; the small `Cargo.lock` change is
  committed with them.
- **The connector address is shown exactly as configured** (`CXP_RESOURCE_URI`, trailing slash
  and all): it is the token audience, so the page must not normalise it.
- Run Profile: every value came from the Development Workflow section; nothing assumed.

## Goal & Success Criteria

A person who lands on the origin in a browser learns, in under a minute, how to add Otto CXP to
their AI assistant; every program sees exactly what it saw before.

- A browser `GET /` gets the page with the configured connector address and the headers below.
- `*/*`, `application/json`, any `Authorization`, any other path, any other method: unchanged JSON
  404 (same body bytes).
- The CSP admits exactly the page's one style block and one script, proven by a test.
- `/mcp` and `/api` auth behaviour unchanged.

## Scope

In: `crates/cxp-server/src/install_page.rs` (render, negotiate, respond), the embedded
`crates/cxp-server/src/install_page.html`, the fallback wiring in `crates/cxp-server/src/lib.rs`,
tests, the route table in `lib.rs`'s module doc and `CLAUDE.md`'s Architecture line.

Out: a web console; tools, auth, scopes, migrations, metering; JavaScript beyond the copy buttons;
i18n; any new config key.

## Architecture

**Module `install_page` (cxp-server, AGPL).**

- `const TEMPLATE: &str = include_str!("install_page.html")` with three placeholders:
  `{{CONNECTOR_URL}}`, `{{GUIDE_URL}}`, `{{CONSOLE_URL}}`. The `<style>` and `<script>` blocks
  contain no placeholder, so their hashes are constant whatever the config.
- `pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; style-src 'sha256-…';
  script-src 'sha256-…'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"`.
- `pub struct InstallPage { html: axum::body::Bytes }`, `Clone` (no new `bytes` dependency).
  `InstallPage::new(resource_uri: &str, platform_url: &str) -> anyhow::Result<Self>` parses
  `platform_url` with `url::Url`, takes `origin().ascii_serialization()`, HTML-escapes
  (`& < > " '`) the connector URL and both link URLs, and substitutes them. Fails only on an
  unparsable platform URL (config already validates it).
- `pub fn wants_page(headers: &HeaderMap) -> bool`: no `Authorization` header and some `Accept`
  range is `text/html` with q > 0.
- `pub fn response(&self, head: bool) -> Response`: 200 with
  `Content-Type: text/html; charset=utf-8`, `Cache-Control: public, max-age=3600`,
  `Vary: Accept, Authorization`, `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`, `Content-Security-Policy: CONTENT_SECURITY_POLICY`,
  `Content-Length`; empty body when `head`.

**Wiring (`lib.rs`).** `router()` builds `InstallPage::new(&config.resource_uri,
&config.platform_url)?` and replaces `.fallback(not_found)` with a fallback closure that serves
the page when the path is exactly `/`, the method is `GET` or `HEAD`, and `wants_page(&headers)`;
otherwise it returns `not_found(uri)` as today. `router()` already returns `Result`, so the new
error path is additive. The page's place in the module-doc route table is `/  browsers only: the
connector install page`.

**Page.** One column, max 600px, 16px gutters, otto-platform's dark console palette as CSS custom
properties, `color-scheme: dark`, system UI font stack, monospace for the address and command,
`overflow-wrap: anywhere` so a long address never scrolls sideways at 360px, a 2px accent focus
outline, `lang="en"`, `<title>Otto CXP connector</title>`, one `h1`. Text colours used for text
are `ink`, `muted` and `accent` only (`faint` falls below 4.5:1 on the canvas, so it is not used
for text). Content exactly as #25's brief lists.

**Public interfaces.** Additive: `GET`/`HEAD /` with an HTML `Accept` and no `Authorization` now
answers 200 HTML instead of 404 JSON. Nothing else changes.

## Invariants impact

- **Auth.** `/mcp` and `/api` are routes, matched before the fallback; untouched. The page is not
  under either surface and carries no data, so it needs no token. A request with `Authorization`
  never gets it.
- **Untrusted text.** The only interpolated values are operator config (`CXP_RESOURCE_URI`,
  `CXP_PLATFORM_URL`), HTML-escaped. No request data reaches the page.
- **No SQL outside cxp-core / tenancy / metering / migrations / pool:** untouched; the page touches
  no database.
- **Licensing:** the new files are in `crates/` (AGPL-3.0-or-later via the workspace license).
- Others (migrations, composite FKs, webhooks, teams, otto-platform pin): not touched.

## Migration

None.

## Out-of-band

None: no secret, config key, scope, or registration. Recurring cost: none (no new app, machine,
volume, or service). Deploy is `ci-auto`; verification is the
`curl` pair in the AC plus the section's `Ship > verify` steps.

## Error Handling & Edge Cases

- `Accept` with invalid UTF-8: that header value is ignored.
- `Accept: text/html;q=0` → JSON. `text/html;q=0.0` → JSON. `TEXT/HTML` → page.
  `text/html;level=1;q=0.5` → page. `text/html;q=abc` → that range does not count.
- `GET /?x=1` → page (path is `/`); `//` or `/index.html` → JSON 404.
- Over-size body on `/` still hits the body limit layer first, unchanged.
- The page renders once at startup; a render failure fails `router()` (boot), never a request.

## Testing Approach

Gates: `DATABASE_URL=postgres://cxp:cxp@localhost:15435/otto_cxp cargo test --workspace --locked`,
clippy `-D warnings`, `cargo fmt --check`, the migrations extra (prints nothing), and
`podman build -t otto-cxp:check .` (crates/** and Cargo files change).

- Unit tests in `install_page.rs`: CSP hashes recomputed from `TEMPLATE`'s `<style>` and
  `<script>` contents equal the ones in `CONTENT_SECURITY_POLICY`; exactly one `<style>` and one
  `<script>` and no inline event handler attribute or `style=` attribute; render substitutes and
  escapes and leaves no `{{`; `wants_page` truth table above; the script contains no placeholder
  and contains both status messages (`Copied.`, `Selected. Press Ctrl+C to copy.`).
- Router tests in `lib.rs`'s `tests` module, through a new raw helper
  `send_raw(app, req) -> (StatusCode, HeaderMap, Bytes)` (the existing `send_to` parses JSON and
  defaults on failure, so it cannot check HTML or exact bytes): browser Accept → 200 with every header and the
  configured address (using a config whose `resource_uri` differs from the default to prove it is
  read from config); `*/*`, `application/json`, HTML + `Authorization` → JSON 404 whose body bytes
  equal the existing `not_found` body for `/`; `HEAD /` HTML → 200, empty body;
  `POST /` HTML → JSON 404; `/nope` HTML → JSON 404; `/mcp` unauthenticated → 401 +
  `WWW-Authenticate` naming the metadata path (existing test, plus one with an HTML Accept).

## Risks & Open Questions

- The Claude and ChatGPT menu paths may move; the page links to the platform's setup guide for
  the maintained version (ticket risk, owner Rob).
- Fly's proxy does not cache, so `Cache-Control: public` only affects browsers; an edit to the page
  can take up to an hour to reach a returning browser. Accepted.
