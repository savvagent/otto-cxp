//! Authentication of `/mcp` and `/api` callers (story S4): the `Principal` a
//! platform-introspected bearer token resolves to and its `cxp:read` /
//! `cxp:write` checks, `AuthFailure`, `authenticate()`, the
//! `WWW-Authenticate` challenge, and the OAuth protected-resource metadata.
//! No axum here; the HTTP crates map these to responses.
//!
//! The authorization server is the otto platform; it mints tokens and this
//! service never sees how. This module is the *resource* server's side:
//!
//! 1. **Refuse an unauthenticated request in a way that teaches the client how
//!    to authenticate.** A `401` carrying [`www_authenticate`]'s
//!    `resource_metadata="…"` (RFC 9728) is the whole onboarding story: the
//!    agent follows it to [`metadata`], finds the platform, registers, and
//!    opens a browser.
//! 2. **Ask the platform whether the token is good for us** (RFC 7662
//!    introspection). The platform enforces the audience, and
//!    `otto_resource` checks it again: a token minted for any other resource
//!    comes back inactive. The audience is configuration
//!    (`CXP_RESOURCE_URI`), never derived from a request's `Host` header.
//! 3. **Honor the platform's lifecycle webhooks** before its introspection
//!    cache catches up: a deleted org or a just-removed member is refused.
//!
//! **A platform or database outage is [`AuthFailure::Unavailable`] (a `503`),
//! never a `401`.** `401` tells an agent its token is dead and sends it to
//! re-authenticate against the very platform that is down.

use chrono::{DateTime, Utc};
use otto_resource::{PlatformClient, Role, TokenClaims, TokenKind};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use serde::Serialize;
use uuid::Uuid;

use crate::scopes;

/// The authenticated caller: what the platform asserted about the token, in
/// the shape handlers use.
///
/// Built only from [`TokenClaims`] the platform returned for this service's
/// own audience; handlers never construct one from request data.
#[derive(Debug, Clone, PartialEq)]
pub struct Principal {
    pub token_id: Uuid,
    pub user_id: UserId,
    /// Fixed when the token was issued: a token opens exactly one org.
    pub org_id: OrgId,
    /// The caller's role in that org *now*, as of the last introspection.
    pub role: Role,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub kind: TokenKind,
    pub expires_at: DateTime<Utc>,
}

impl From<TokenClaims> for Principal {
    fn from(c: TokenClaims) -> Self {
        Self {
            token_id: c.token_id,
            user_id: c.user_id.into(),
            org_id: c.org_id.into(),
            role: c.role,
            client_id: c.client_id,
            scopes: c.scopes,
            kind: c.kind,
            expires_at: c.expires_at,
        }
    }
}

/// A token that lacks the scope a tool or route needs. Answered `403`
/// (`insufficient_scope`), not `401`: the token is fine, its grant is not.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("this token lacks the {0} scope; obtain a token that grants it")]
pub struct MissingScope(pub &'static str);

impl Principal {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// `cxp:write` implies read.
    pub fn can_read(&self) -> bool {
        self.has_scope(scopes::CXP_READ) || self.can_write()
    }

    pub fn can_write(&self) -> bool {
        self.has_scope(scopes::CXP_WRITE)
    }

    /// At the top of every tool and route that reads.
    pub fn require_read(&self) -> Result<(), MissingScope> {
        if self.can_read() {
            Ok(())
        } else {
            Err(MissingScope(scopes::CXP_READ))
        }
    }

    /// At the top of every tool and route that creates, changes, or deletes.
    pub fn require_write(&self) -> Result<(), MissingScope> {
        if self.can_write() {
            Ok(())
        } else {
            Err(MissingScope(scopes::CXP_WRITE))
        }
    }
}

/// Why a request was not authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    /// No `Authorization: Bearer …` header. `401`.
    Missing,
    /// The platform answered that the token is not active for this resource:
    /// unknown, expired, revoked, minted for another resource, or its user
    /// left the org. One answer on purpose: the distinctions are an oracle.
    /// `401`.
    Inactive,
    /// The platform still vouches for the token, but has since told us the
    /// org was deleted or the user removed from it. `401`.
    Revoked,
    /// Could not find out: the platform or the database did not answer.
    /// `503` with `Retry-After`, never `401`.
    Unavailable,
}

impl AuthFailure {
    /// The `error_description`: what went wrong and what to do. Contains no
    /// `"` or `\`, so it is safe inside a quoted header parameter.
    pub fn reason(&self) -> &'static str {
        match self {
            AuthFailure::Missing => "an OAuth 2.1 bearer token is required",
            AuthFailure::Inactive => {
                "this token is not valid for this resource; obtain a new one, requesting the \
                 resource indicator this server advertises"
            }
            AuthFailure::Revoked => {
                "this token is no longer valid for this resource; sign in again"
            }
            AuthFailure::Unavailable => {
                "could not verify the token right now; retry shortly. Your credentials are fine."
            }
        }
    }

    /// Whether this is an outage (`503`) rather than an authentication
    /// failure (`401`).
    pub fn is_unavailable(&self) -> bool {
        matches!(self, AuthFailure::Unavailable)
    }
}

/// The bearer token in an `Authorization` header value, if any.
///
/// The scheme is compared case-insensitively (RFC 7235; real clients send
/// `bearer`). The token is trimmed but otherwise untouched: it is compared by
/// hash, so any normalization could only turn a valid token into an invalid
/// one.
pub fn bearer(authorization: Option<&str>) -> Option<&str> {
    let (scheme, token) = authorization?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Resolve an `Authorization` header value to the caller, or say why not.
///
/// The platform client caches a positive introspection for up to 60 s, so the
/// tombstones (`deleted_orgs`, `removed_members`) are checked on every call to
/// honor a lifecycle webhook within that window.
pub async fn authenticate(
    db: &Db,
    platform: &PlatformClient,
    authorization: Option<&str>,
) -> Result<Principal, AuthFailure> {
    let token = bearer(authorization).ok_or(AuthFailure::Missing)?;
    let claims = match platform.introspect(token).await {
        Ok(Some(claims)) => claims,
        Ok(None) => return Err(AuthFailure::Inactive),
        Err(e) => {
            tracing::error!(error = %e, "token introspection failed; answering 503");
            return Err(AuthFailure::Unavailable);
        }
    };
    match crate::platform_events::revoked(db, claims.org_id.into(), claims.user_id.into()).await {
        Ok(false) => Ok(Principal::from(claims)),
        Ok(true) => Err(AuthFailure::Revoked),
        Err(e) => {
            tracing::error!(error = %e, source = ?std::error::Error::source(&e),
                "could not check the revocation tombstones; answering 503");
            Err(AuthFailure::Unavailable)
        }
    }
}

/// Where an unauthenticated client is sent: `{public_url}/.well-known/oauth-protected-resource`.
/// A trailing slash on `public_url` does not double up.
pub fn metadata_url(public_url: &str) -> String {
    format!(
        "{}/.well-known/oauth-protected-resource",
        public_url.trim_end_matches('/')
    )
}

/// The `WWW-Authenticate` value for a `401`. `error` and `error_description`
/// follow RFC 6750; `resource_metadata` follows RFC 9728 and is the field MCP
/// clients actually read.
pub fn www_authenticate(public_url: &str, failure: &AuthFailure) -> String {
    format!(
        r#"Bearer realm="{}", error="invalid_token", error_description="{}", resource_metadata="{}""#,
        scopes::RESOURCE_NAME,
        failure.reason(),
        metadata_url(public_url)
    )
}

/// The RFC 9728 protected-resource metadata document served at
/// `/.well-known/oauth-protected-resource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectedResourceMetadata {
    /// `CXP_RESOURCE_URI`: the audience every token must name.
    pub resource: String,
    pub resource_name: &'static str,
    /// The platform: this service issues no tokens of its own.
    pub authorization_servers: Vec<String>,
    /// [`scopes::KNOWN`], which is also what is registered with the platform.
    pub scopes_supported: Vec<&'static str>,
    pub bearer_methods_supported: Vec<&'static str>,
}

pub fn metadata(resource_uri: &str, platform_url: &str) -> ProtectedResourceMetadata {
    ProtectedResourceMetadata {
        resource: resource_uri.to_string(),
        resource_name: scopes::RESOURCE_NAME,
        authorization_servers: vec![platform_url.trim_end_matches('/').to_string()],
        scopes_supported: scopes::KNOWN.to_vec(),
        bearer_methods_supported: vec!["header"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(scopes: &[&str]) -> Principal {
        Principal {
            token_id: Uuid::new_v4(),
            user_id: UserId::new(),
            org_id: OrgId::new(),
            role: Role::Member,
            client_id: None,
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            kind: TokenKind::Oauth,
            expires_at: Utc::now(),
        }
    }

    #[test]
    fn write_implies_read_but_not_the_reverse() {
        let read = principal(&["cxp:read"]);
        assert!(read.require_read().is_ok());
        assert_eq!(read.require_write(), Err(MissingScope("cxp:write")));

        let write = principal(&["cxp:write"]);
        assert!(write.require_read().is_ok());
        assert!(write.require_write().is_ok());

        let none = principal(&["flags:write"]);
        assert_eq!(none.require_read(), Err(MissingScope("cxp:read")));
        assert_eq!(none.require_write(), Err(MissingScope("cxp:write")));
    }

    #[test]
    fn the_scheme_is_case_insensitive() {
        for prefix in ["Bearer", "bearer", "BEARER", "BeArEr"] {
            assert_eq!(
                bearer(Some(&format!("{prefix} otto_at_abc"))),
                Some("otto_at_abc"),
                "{prefix} should be accepted"
            );
        }
    }

    #[test]
    fn other_schemes_and_malformed_headers_carry_no_token() {
        for bad in [
            "Basic dXNlcjpwdw==",
            "otto_at_abc",
            "Bearer",
            "Bearer   ",
            "",
        ] {
            assert_eq!(bearer(Some(bad)), None, "{bad:?} should yield no token");
        }
        assert_eq!(bearer(None), None);
    }

    /// The header a client cannot follow is the header that breaks onboarding.
    #[test]
    fn the_challenge_points_at_the_metadata_document() {
        for failure in [
            AuthFailure::Missing,
            AuthFailure::Inactive,
            AuthFailure::Revoked,
            AuthFailure::Unavailable,
        ] {
            let header = www_authenticate("https://cxp.example.com/", &failure);
            assert_eq!(
                header,
                format!(
                    r#"Bearer realm="otto-cxp", error="invalid_token", error_description="{}", resource_metadata="https://cxp.example.com/.well-known/oauth-protected-resource""#,
                    failure.reason()
                )
            );
            assert!(!failure.reason().contains(['"', '\\']), "{failure:?}");
            assert!(http_header_safe(&header), "{header}");
        }
        assert!(AuthFailure::Unavailable.is_unavailable());
        assert!(!AuthFailure::Revoked.is_unavailable());
    }

    /// Visible ASCII and spaces only, so `HeaderValue::from_str` accepts it.
    fn http_header_safe(s: &str) -> bool {
        s.bytes().all(|b| b == b' ' || b.is_ascii_graphic())
    }

    #[test]
    fn the_metadata_document_names_the_platform_and_the_scopes() {
        let doc = serde_json::to_value(metadata(
            "https://cxp.example.com/mcp",
            "https://otto.example.com/",
        ))
        .unwrap();
        assert_eq!(
            doc,
            serde_json::json!({
                "resource": "https://cxp.example.com/mcp",
                "resource_name": "otto-cxp",
                "authorization_servers": ["https://otto.example.com"],
                "scopes_supported": ["cxp:read", "cxp:write"],
                "bearer_methods_supported": ["header"],
            })
        );
    }
}
