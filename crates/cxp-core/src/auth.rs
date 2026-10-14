//! Authentication of `/mcp` and `/api` callers (story S4): the `Principal` a
//! platform-introspected bearer token resolves to and its `cxp:read` /
//! `cxp:write` checks, `AuthFailure`, `authenticate()`, the
//! `WWW-Authenticate` challenge, and the OAuth protected-resource metadata.
//! No axum here; the HTTP crates map these to responses.
