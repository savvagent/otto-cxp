//! The MCP side of authentication. `cxp_http::require_bearer` gates `/mcp`
//! (see that crate for the status codes and headers); this module reads back
//! what it attached.
//!
//! **The principal is per request, not per session.** It is inserted into the
//! request's extensions, which the MCP transport carries through to every
//! handler as `http::request::Parts`.

use cxp_core::auth::Principal;

/// The authenticated caller of a tool call, resource read, or prompt: the
/// principal `cxp_http::require_bearer` attached to the HTTP request.
pub fn principal_from(parts: &http::request::Parts) -> Option<Principal> {
    parts.extensions.get::<Principal>().cloned()
}
