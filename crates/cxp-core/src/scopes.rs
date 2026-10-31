//! The scopes Savvagent CXP's resource server understands.
//!
//! Registered with the platform (`otto-platform-server resource register
//! --scopes …`, see docs/deploy/fly.md), advertised in
//! `/.well-known/oauth-protected-resource`, and checked by every route and
//! by every tool except `whoami` and `usage`, which need no scope. One list,
//! so the three cannot drift.

/// What this resource server is called on a consent screen, in its metadata,
/// in `WWW-Authenticate`'s realm, and in `whoami`: the name it is registered
/// under at the platform (#59). The billing service slug stays `otto-cxp`.
pub const RESOURCE_NAME: &str = "Savvagent CXP";

/// Read customers, tickets, knowledge, chat, feedback, surveys, the catalog,
/// and the dashboard.
pub const CXP_READ: &str = "cxp:read";
/// Create, change, and delete them. Implies read.
pub const CXP_WRITE: &str = "cxp:write";

pub const KNOWN: &[&str] = &[CXP_READ, CXP_WRITE];

/// Granted when a client asks for nothing in particular. Read-only, as in
/// otto-factory and otto-flags: a client that wants to change anything has to
/// ask, and the person sees it on the consent screen.
pub const DEFAULT: &[&str] = &[CXP_READ];

#[cfg(test)]
mod tests {
    use super::*;

    /// These strings are carried by issued tokens and registered at the
    /// platform. Changing one breaks every existing grant.
    #[test]
    fn the_scope_list_is_stable() {
        assert_eq!(RESOURCE_NAME, "Savvagent CXP");
        assert_eq!(KNOWN, ["cxp:read", "cxp:write"]);
        assert_eq!(DEFAULT, ["cxp:read"]);
        assert!(DEFAULT.iter().all(|s| KNOWN.contains(s)));
    }
}
