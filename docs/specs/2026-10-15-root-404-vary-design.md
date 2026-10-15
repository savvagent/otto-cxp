# `Vary: Accept, Authorization` on the JSON 404 at `/`

**Status:** DRAFT
**Ticket:** #32

## Brief

From #32 (verbatim):

> Since #29, `GET /` returns either the HTML install page or the JSON 404, depending on `Accept`
> and `Authorization`. The HTML response carries `Vary: Accept, Authorization`, but the JSON 404
> doesn't. A shared cache that stored the JSON 404 could hand it to a browser, or the reverse.
> otto-flags sends `Vary` on both branches (otto-flags#75).
>
> Add `Vary: Accept, Authorization` to the JSON 404 for `/` only. Other paths' 404s don't vary,
> so they stay as they are. The status and body don't change.

Acceptance criteria:

- `curl -si https://otto-cxp.savvagent.com/` shows `Vary: Accept, Authorization` and the same
  JSON body as before.
- A 404 for any other path is unchanged.
- There is a router-level test for both cases.

## Premise corrections

- This reverses one decision of the install-page spec
  (`docs/specs/2026-10-14-connector-install-page-design.md`, "The JSON 404 stays identical,
  headers included. It gets no `Vary`"). That spec reasoned that the 404 carries no freshness
  information, so no cache reuses it. RFC 9111 lets a cache store a 404 heuristically, so that
  reasoning doesn't hold for every cache. The shipped spec stays as written, since it records
  #29. This spec supersedes that one bullet.

## Assumptions

- **`GET` and `HEAD` only.** Only those two methods choose between the page and the JSON 404
  (`fallback` in `crates/cxp-server/src/lib.rs`). For `POST`/`PUT`/`DELETE /` the answer never
  depends on `Accept` or `Authorization`, and those responses aren't cached, so they stay
  unchanged. "The JSON 404 for `/`" in the brief is read as the response that competes with the
  page.
- **The value matches the page's exactly**: `Accept, Authorization`, one shared constant, so the
  two branches can't drift apart.
- **Path match is exact `/`** (the same `req.uri().path() == "/"` test that serves the page;
  a query string doesn't change the path). `//` and every other path keep a plain 404.
- No Run Profile value was assumed. The section supplies every key this run uses.

## Goal & Success Criteria

Make every response that `GET`/`HEAD /` can produce declare the request headers that chose it, so
no shared cache serves one branch's response to the other branch's request.

- `GET /` and `HEAD /` that get the JSON 404 carry `Vary: Accept, Authorization`.
- The status (404), `Content-Type` (`application/json`), and body bytes don't change.
- `POST /`, `GET /nope`, `GET //`, and every other 404 carry no `Vary`.
- The install page response doesn't change.

## Scope

In: the fallback's handling of `GET`/`HEAD /` when the page is not wanted; router-level tests.

Out: `Vary` on any other route or 404; `Cache-Control` on the 404; changes to `wants_page`.

## Architecture

`fallback` (`crates/cxp-server/src/lib.rs`) serves the page for page-eligible
`GET`/`HEAD /`, and otherwise delegates to `console_or_not_found`. For path `/` that always
ends in `not_found("/")`, because `/` is never under `/console`. The change: when the path is
`/` and the method is `GET` or `HEAD`, the fallback returns `not_found("/")` with
`Vary: Accept, Authorization` inserted. Everything else still goes to `console_or_not_found`.
`install_page` exposes the shared `VARY` value (`pub const VARY: &str = "Accept, Authorization"`),
which both `InstallPage::response` and the fallback use.

No public interface changes beyond one response header on one path, which is additive.

## Invariants impact

None touched: no SQL, migrations, auth, metering, tenancy, or config. The `/mcp` 401 and
`WWW-Authenticate` behaviour doesn't change.

## Out-of-band

None. No config key, secret, scope, migration, or role change.

## Error Handling & Edge Cases

- `HEAD /` with a non-browser `Accept`: same headers as `GET` plus `Vary`. In production hyper
  strips the body.
- `/?q=1`: the path is `/`, so it gets `Vary` as well.
- An `Authorization` header with a browser `Accept`: JSON 404 with `Vary`.

## Testing Approach

Router-level unit tests in `crates/cxp-server/src/lib.rs` `mod tests`:

- `programs_at_the_root_still_get_the_json_404`: assert `vary == "Accept, Authorization"`
  instead of absent, keeping the byte-for-byte body check, and add `HEAD /` and `GET /?from=agent`
  cases. `not_found` itself doesn't change, so `root_404_bytes()` still pins the old body. The
  tests assert the literal value rather than `install_page::VARY`, so a wrong value in the
  constant fails them.
- `other_methods_and_paths_stay_json_404_for_a_browser`: assert no `vary` on every case
  (`POST /`, `GET /nope`, `GET //`, and the rest).

Gates per CLAUDE.md: test, clippy, fmt, the migrations-append-only check, and `podman build`
(crates/** changed).

## Risks & Open Questions

- A `Vary` on a 404 slightly lowers cache hit rates for `/`. That's negligible, and it's the
  point of the change.
