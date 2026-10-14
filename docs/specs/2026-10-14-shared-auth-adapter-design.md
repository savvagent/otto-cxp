# One axum auth adapter for /mcp and /api

**Status:** DRAFT (#18)

## Brief

`crates/cxp-api/src/auth.rs` and `crates/cxp-mcp/src/auth.rs` each carried the
same axum adapter around `cxp_core::auth::refusal` (`require_bearer`,
`refuse`), and `/.well-known/oauth-protected-resource` was mounted by cxp-mcp
although cxp-api's `401` points at it.

## Premise corrections

- The two copies were not byte-for-byte equal: cxp-api's also gives a
  handler's own `401` without a challenge the revoked-token challenge
  (`access_revoked`); cxp-mcp's does not. The shared adapter keeps both
  behaviours behind one switch.
- "Existing auth tests pass unchanged" cannot hold literally for cxp-mcp:
  `crates/cxp-mcp/tests/mcp.rs` requests the metadata document from
  `cxp_mcp::router` and builds `cxp_mcp::Config` with `resource_uri` and
  `platform_url`. Moving the route to cxp-server removes both. See Testing.

## Assumptions

- The adapter lives in a new crate, not in `cxp-core`.
- Wire behaviour stays identical: status, `WWW-Authenticate`, `Retry-After`,
  body.

## Goal & Success Criteria

- One adapter used by both surfaces.
- cxp-server mounts the protected-resource metadata route.
- Auth tests in cxp-mcp, cxp-api, and cxp-server e2e pass (the two edits to
  cxp-mcp's tests are listed under Testing).
- A later alternate credential (the console's session cookie, #14) is added in
  one place.

## Scope

In: new `cxp-http` crate; cxp-api and cxp-mcp use it; cxp-server mounts the
metadata route; cxp-mcp drops its metadata route and the two now-unused
`Config` fields.
Out: any cookie or session support; changing `cxp_core::auth`.

## Architecture

Decision: a small `cxp-http` crate (axum, http, cxp-core, otto-tenant,
otto-resource), not an `axum` feature of `cxp-core`.

- cxp-core is the crate everything depends on and "holds SQL + domain logic";
  its `auth` module is deliberately HTTP-free (`Refusal` is plain data). An
  axum feature would put a web framework behind a cargo feature flag in the
  crate every build links, and feature unification across the workspace would
  switch it on everywhere anyway.
- The crate is about 150 lines, has one reason to change (how a request is
  authenticated and refused), and has exactly the two consumers plus the
  binary. It holds no SQL (it calls `cxp_core::auth::authenticate`).
- cxp-api cannot depend on cxp-mcp or the reverse, and neither owns the
  metadata document, so a third crate is the clean home.

Shape (`crates/cxp-http/src/lib.rs`):

- `Authenticator { db, platform, public_url }`, used as middleware state.
  `Authenticator::authenticate(&HeaderMap) -> Result<Principal, AuthFailure>`
  is the single seam where a request's credential is read; a session-cookie
  source (#14) extends that method and both surfaces get it. The two other
  consumers of the result (`require_bearer`, `refuse`) do not change.
- `Authenticator::challenge_handler_401()` turns on cxp-api's behaviour of
  adding the revoked-token challenge to a handler's bare `401`.
- `require_bearer`, `refuse`: moved verbatim from the two copies.
- `protected_resource_router(resource_uri, platform_url) -> Router` and
  `PROTECTED_RESOURCE_PATH`: the open RFC 9728 document, mounted by
  `cxp_server::router` (`crates/cxp-server/src/lib.rs`) next to health.

Removed: `crates/cxp-api/src/auth.rs`; cxp-mcp's `require_bearer`, `refuse`,
`protected_resource_metadata`, and its `Shared` state. cxp-mcp keeps
`principal_from`. `cxp_mcp::Config` loses `resource_uri` and `platform_url`
(only the removed route read them; the audience is enforced by the
`PlatformClient`, built by cxp-server).

The Dockerfile copies `crates/` whole, so it needs no change.

## Invariants impact

Auth invariant unchanged: same checks, same refusals, one implementation.
Request paths still use `begin_live`. No SQL added. New crate is
AGPL-3.0-or-later through the workspace license field.

## Migration

None.

## Out-of-band

None.

## Error handling & edge cases

`refuse` still `expect`s header-safe values, guaranteed by `cxp_core::auth`.
The metadata router has no state and cannot fail.

## Testing approach

- Unit tests of `refuse` move verbatim from cxp-mcp to cxp-http, plus a test
  that `protected_resource_router` serves the document.
- cxp-server's existing router tests already cover the metadata route and the
  challenge URL on `/mcp` and `/api/v1`; a new assertion ties the challenge to
  `PROTECTED_RESOURCE_PATH`.
- cxp-mcp `tests/mcp.rs`: the two `Config { .. }` literals drop the removed
  fields, and the "metadata document is open" block is removed from the
  no-token test (the route is no longer cxp-mcp's; cxp-server's unit test and
  e2e assert it). No assertion about the wire behaviour of `/mcp` changed.

## Risks & open questions

`cxp_mcp::router` used on its own no longer serves the metadata document;
cxp-server is its only production caller.
