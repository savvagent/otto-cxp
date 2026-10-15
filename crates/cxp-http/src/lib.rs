//! `cxp-http` — the axum side of authentication, shared by `/mcp` and
//! `/api/v1` so the two surfaces cannot drift apart.
//!
//! `cxp_core::auth` decides (it holds the platform and database calls and
//! knows nothing of HTTP); this crate answers:
//!
//! - No token, or one the platform calls inactive (unknown, expired, revoked,
//!   minted for another resource), or one whose org or member the platform has
//!   since told us is gone: `401` with `WWW-Authenticate` naming
//!   `/.well-known/oauth-protected-resource`, which is how a client learns
//!   where to sign in.
//! - The platform or the database could not answer: `503` with
//!   `Retry-After: 5`, never `401`. A `401` sends every connected client to
//!   re-authenticate against the platform that is down.
//! - The platform refused this service's own credential, or something that
//!   is not the platform answered: `500`, with neither header. A
//!   misconfiguration no retry can fix, logged at `error`.
//!
//! The one place a request's credential becomes a [`Principal`] is
//! [`Authenticator::authenticate_request`]. A further credential source (the
//! web console's session cookie, [`console`]) is opt-in per surface, like
//! [`Authenticator::challenge_handler_401`]: only `/api` enables it
//! ([`Authenticator::with_console`]), and there a cookie-authenticated write
//! must come from this origin ([`console::same_origin`]) or is `403
//! cross_site_request`. `/mcp` stays bearer-only, so a cookie a browser
//! attaches on its own can never authenticate an MCP call.
//!
//! **A bearer token, when present, decides the request**, valid or not: it is
//! never "fixed" by falling back to a cookie, so a script's behaviour does not
//! depend on what a browser left lying around. Only a request with no bearer
//! looks at the cookie.
//!
//! The protected-resource metadata document (RFC 9728) is served by
//! [`protected_resource_router`], which the binary mounts: the `401`s of every
//! surface point at it, so it belongs to none of them.

pub mod console;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cxp_core::auth::{self, AuthFailure, Principal};
use http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use otto_resource::PlatformClient;
use otto_tenant::Db;

/// Where the protected-resource metadata is served, and what a `401`'s
/// `resource_metadata` points at.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// Turns a request into a [`Principal`], or says why not. Cheap to clone.
#[derive(Clone)]
pub struct Authenticator {
    db: Db,
    platform: Arc<PlatformClient>,
    public_url: String,
    challenge_handler_401: bool,
    console: Option<console::Console>,
}

/// Which credential authenticated a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `Authorization: Bearer ...`.
    Bearer,
    /// The web console's session cookie: ambient, so writes must prove their
    /// origin.
    Cookie,
}

impl Authenticator {
    pub fn new(db: Db, platform: Arc<PlatformClient>, public_url: impl Into<String>) -> Self {
        Self {
            db,
            platform,
            public_url: public_url.into(),
            challenge_handler_401: false,
            console: None,
        }
    }

    /// Accept the web console's session cookie on this surface, for a request
    /// that carries no bearer token. `/api` only; never `/mcp`.
    pub fn with_console(mut self, console: console::Console) -> Self {
        self.console = Some(console);
        self
    }

    /// Give a handler's own `401` without a `WWW-Authenticate` the challenge a
    /// revoked token gets. A handler answers `401` (`access_revoked`) when the
    /// org was deleted or the caller removed between authentication and its
    /// transaction.
    pub fn challenge_handler_401(mut self) -> Self {
        self.challenge_handler_401 = true;
        self
    }

    /// The credential in `headers`, checked: [`Self::authenticate_request`]
    /// without saying which credential it was.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<Principal, AuthFailure> {
        self.authenticate_request(headers).await.map(|(p, _)| p)
    }

    /// The credential in `headers`, checked, and which one it was: the bearer
    /// token in `Authorization` if there is one (valid or not), otherwise the
    /// console's session cookie where this surface accepts it
    /// ([`Self::with_console`]), otherwise [`AuthFailure::Missing`].
    pub async fn authenticate_request(
        &self,
        headers: &HeaderMap,
    ) -> Result<(Principal, Source), AuthFailure> {
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        if auth::bearer(authorization).is_some() {
            return auth::authenticate(&self.db, &self.platform, authorization)
                .await
                .map(|p| (p, Source::Bearer));
        }
        if let Some(console) = &self.console {
            if let Some(cookie) = console.session_cookie(headers) {
                return console
                    .authenticate(cookie)
                    .await
                    .map(|p| (p, Source::Cookie));
            }
        }
        Err(AuthFailure::Missing)
    }

    /// Whether this request would authenticate with the console's session
    /// cookie (no bearer, a cookie this surface accepts) and is a write that
    /// did not come from this origin ([`console::same_origin`]).
    fn is_cross_site_cookie_write(&self, method: &http::Method, headers: &HeaderMap) -> bool {
        let Some(console) = &self.console else {
            return false;
        };
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        auth::bearer(authorization).is_none()
            && console.session_cookie(headers).is_some()
            && !console.same_origin(method, headers)
    }
}

/// Reject the request unless it carries a live credential audienced for us;
/// otherwise put the [`Principal`] in the request's extensions for handlers
/// (the MCP transport carries it through as `http::request::Parts`). A
/// cookie-authenticated write that did not come from this origin is refused
/// with `403 cross_site_request` before the handler runs.
pub async fn require_bearer(
    State(auth): State<Authenticator>,
    mut req: Request,
    next: Next,
) -> Response {
    // Before anything is looked up: a forged cross-site write must not even
    // cause a token refresh or a session deletion.
    if auth.is_cross_site_cookie_write(req.method(), req.headers()) {
        tracing::warn!(method = %req.method(), path = %req.uri().path(),
            "refused a cookie-authenticated write from another origin");
        return console::cross_site();
    }
    match auth.authenticate_request(req.headers()).await {
        Ok((principal, _)) => {
            req.extensions_mut().insert::<Principal>(principal);
            let mut res = next.run(req).await;
            if auth.challenge_handler_401
                && res.status() == StatusCode::UNAUTHORIZED
                && !res.headers().contains_key(header::WWW_AUTHENTICATE)
            {
                res.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    revoked_challenge(&auth.public_url),
                );
            }
            res
        }
        Err(failure) => refuse(&auth.public_url, failure),
    }
}

/// The response for a request that was not authenticated: the shared
/// [`auth::refusal`] as an axum response.
pub(crate) fn refuse(public_url: &str, failure: AuthFailure) -> Response {
    let refusal = auth::refusal(public_url, &failure);
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::UNAUTHORIZED);
    let mut response = (status, Json(refusal.body)).into_response();
    for (name, value) in refusal.headers {
        let value =
            HeaderValue::from_str(&value).expect("cxp_core::auth guarantees header-safe values");
        response
            .headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
    response
}

/// The challenge a handler's `401` (`access_revoked`) gets: the one a revoked
/// token gets from [`require_bearer`].
fn revoked_challenge(public_url: &str) -> HeaderValue {
    let refusal = auth::refusal(public_url, &AuthFailure::Revoked);
    let challenge = refusal
        .www_authenticate()
        .expect("a 401 refusal carries a challenge");
    HeaderValue::from_str(challenge).expect("cxp_core::auth guarantees header-safe values")
}

/// `GET /.well-known/oauth-protected-resource` (RFC 9728). Open: it is how an
/// unauthenticated client finds the authorization server (the platform).
pub fn protected_resource_router(resource_uri: &str, platform_url: &str) -> Router {
    let document = auth::metadata(resource_uri, platform_url);
    Router::new().route(
        PROTECTED_RESOURCE_PATH,
        get(move || {
            let document = document.clone();
            async move { Json(document) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_token_is_a_401_that_points_at_the_metadata() {
        let r = refuse("https://cxp.example.com/", AuthFailure::Missing);
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let h = r.headers()[http::header::WWW_AUTHENTICATE]
            .to_str()
            .unwrap();
        assert!(h.starts_with("Bearer "), "{h}");
        assert!(!h.contains("error="), "{h}");
        assert!(
            h.contains(
                r#"resource_metadata="https://cxp.example.com/.well-known/oauth-protected-resource""#
            ),
            "{h}"
        );
        assert!(h.contains(PROTECTED_RESOURCE_PATH));
    }

    #[test]
    fn inactive_and_revoked_are_401_with_invalid_token() {
        for f in [AuthFailure::Inactive, AuthFailure::Revoked] {
            let r = refuse("https://cxp.example.com", f);
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            let h = r.headers()[http::header::WWW_AUTHENTICATE]
                .to_str()
                .unwrap();
            assert!(h.contains(r#"error="invalid_token""#), "{h}");
        }
    }

    #[test]
    fn an_outage_is_a_503_with_retry_after_and_no_challenge() {
        let r = refuse("https://cxp.example.com", AuthFailure::Unavailable);
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.headers()[http::header::RETRY_AFTER], "5");
        assert!(r.headers().get(http::header::WWW_AUTHENTICATE).is_none());
    }

    #[tokio::test]
    async fn the_metadata_router_serves_the_document_it_was_built_with() {
        use tower::ServiceExt;
        let app = protected_resource_router("https://cxp.example.com/mcp", "https://p.example.com");
        let res = app
            .oneshot(
                Request::get(PROTECTED_RESOURCE_PATH)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(doc["resource"], "https://cxp.example.com/mcp");
        assert_eq!(doc["authorization_servers"][0], "https://p.example.com");
        assert_eq!(
            doc["scopes_supported"],
            serde_json::json!(["cxp:read", "cxp:write"])
        );
    }
}
