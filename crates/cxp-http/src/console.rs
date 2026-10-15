//! The web console's sign-in: OAuth authorization code + PKCE against the
//! otto platform, and the session cookie that `/api` accepts afterwards.
//!
//! ```text
//!  browser                     otto-cxp                                 otto platform
//!    │ GET /auth/login?next=/x    │ state + PKCE verifier -> sealed cookie   │
//!    │◄─ 303 + __Host-cxp_oauth ──┤                                          │
//!    │ GET /oauth/authorize?client_id&code_challenge&resource&scope&state ──►│ sign in, pick org
//!    │◄──────────────────────────────────── 303 /auth/callback?code&state ───┤
//!    │ GET /auth/callback ───────►│ POST /oauth/token (code, verifier) ─────►│
//!    │                            │◄── access + refresh ─────────────────────┤
//!    │                            │ introspect + tombstones; store session   │
//!    │◄─ 303 next + __Host-cxp_session                                       │
//! ```
//!
//! The browser only ever holds two opaque cookies; the platform's tokens stay
//! in `console_sessions`, sealed (`cxp_core::console_sessions`). The session
//! cookie is a credential only where a surface opts in
//! ([`crate::Authenticator::with_console`]): `/api`, never `/mcp`.
//!
//! # Cookies
//!
//! - **`__Host-cxp_session`**: 32 random bytes, base64url, and nothing else. A
//!   lookup key with no claims in it; only its SHA-256 is stored.
//! - **`__Host-cxp_oauth`**: the in-flight sign-in (`state`, the PKCE
//!   verifier, where to land), sealed with `CXP_ENCRYPTION_KEY`, for ten
//!   minutes. It makes the callback the answer to *this browser's* request
//!   (login CSRF) and keeps the verifier off the URL.
//!
//! Both are `HttpOnly; Secure; SameSite=Lax; Path=/` with no `Domain`, which
//! the `__Host-` prefix makes a browser enforce: a sibling subdomain cannot
//! plant them. `Lax` rather than `Strict` because the callback is a cross-site
//! top-level navigation (back from the platform) that must carry the oauth
//! cookie; `Lax` still keeps the session cookie off cross-site subresource
//! requests and POSTs, and the same-origin check on writes
//! ([`same_origin`]) covers same-site siblings. On a plain-http loopback
//! public URL (local development) the names drop `__Host-` and the cookies
//! drop `Secure`, which `__Host-` requires.
//!
//! # Refresh is single-flight, and survives a cancelled request
//!
//! The platform **rotates the refresh token on every use, and presenting a
//! spent one revokes the whole family**, so two requests that both refreshed
//! would sign the person out. [`Console::authenticate`] refreshes holding the
//! session's row lock (`cxp_core::console_sessions::lock`): a concurrent
//! request blocks on it, re-reads the row, finds the fresh token, and
//! refreshes nothing, across replicas too. The refresh runs in a spawned task:
//! once the platform has rotated the pair, storing it is not optional, and a
//! handler future is dropped when its client disconnects.
//!
//! | Platform says | Session | Request |
//! |---|---|---|
//! | a new pair | rotated in place | proceeds |
//! | `invalid_grant` | deleted | `401`: sign in again |
//! | unreachable, 5xx | untouched | `503`, never `401` |
//! | any other refusal | untouched | `500`: the client registration or configuration is wrong |
//!
//! `/auth/login` and `/auth/callback` are navigations, so every failure there
//! is a redirect to `/?login_error=<code>` from a fixed vocabulary; nothing
//! from the query string is reflected.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::Utc;
use cxp_core::auth::{self, AuthFailure, Principal};
use cxp_core::console_sessions::{self, NewSession};
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use otto_resource::{PlatformClient, Role};
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// What the console asks the platform for: every scope its routes check. The
/// platform grants the subset the person may have, and the routes check the
/// grant as they do for a bearer.
pub const CONSOLE_SCOPES: &[&str] = &[cxp_core::scopes::CXP_READ, cxp_core::scopes::CXP_WRITE];

/// How long a sign-in may take, from `/auth/login` to `/auth/callback`.
pub const OAUTH_MAX_AGE_SECS: i64 = 600;

/// How long a session lasts without a refresh: the platform's refresh-token
/// lifetime. Each rotation issues a fresh refresh token and restarts it.
pub const SESSION_TTL_DAYS: i64 = 30;

/// Used when the platform's token response omits `expires_in`.
const DEFAULT_ACCESS_TTL_SECS: i64 = 3600;

/// Longest `next` path accepted.
const MAX_NEXT_LEN: usize = 512;

/// What the console's sign-in needs to know about this deployment.
pub struct ConsoleSettings {
    /// `CXP_CONSOLE_CLIENT_ID`: the first-party public client registered at
    /// the platform with the redirect URI `{public_url}/auth/callback`.
    pub client_id: String,
    /// `CXP_ENCRYPTION_KEY`: seals the stored tokens and the oauth cookie.
    pub cipher: Cipher,
    /// `CXP_PUBLIC_URL`, no trailing slash: the console's origin.
    pub public_url: String,
    /// `CXP_PLATFORM_URL`: where `/oauth/authorize` and `/oauth/token` are.
    pub platform_url: String,
    /// `CXP_RESOURCE_URI`: the `resource` every token is requested for.
    pub resource_uri: String,
}

/// The console's sign-in. Cheap to clone.
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    platform: Arc<PlatformClient>,
    oauth: PlatformOAuth,
    settings: ConsoleSettings,
    cookies: Cookies,
    /// `scheme://host[:port]` of the public URL, as a browser sends `Origin`.
    origin: String,
}

impl Console {
    pub fn new(db: Db, platform: Arc<PlatformClient>, settings: ConsoleSettings) -> Self {
        let public_url = settings.public_url.trim_end_matches('/').to_string();
        let secure = !public_url.starts_with("http://");
        let settings = ConsoleSettings {
            public_url,
            ..settings
        };
        Self {
            inner: Arc::new(Inner {
                oauth: PlatformOAuth::new(&settings.platform_url),
                origin: origin_of(&settings.public_url).unwrap_or_default(),
                cookies: Cookies { secure },
                db,
                platform,
                settings,
            }),
        }
    }

    fn redirect_uri(&self) -> String {
        format!("{}/auth/callback", self.inner.settings.public_url)
    }

    /// The session cookie on this request, if it is shaped like one we mint.
    /// Anything else is never looked up.
    pub fn session_cookie<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        read_cookie(headers, self.inner.cookies.session()).filter(|v| is_token_shaped(v))
    }

    /// Whether a non-safe request carrying the session cookie came from this
    /// origin (see [`same_origin`]).
    pub fn same_origin(&self, method: &Method, headers: &HeaderMap) -> bool {
        same_origin(&self.inner.origin, method, headers)
    }

    /// The caller behind the session cookie `cookie`: a platform access token
    /// good for this request (refreshed first if it is about to expire),
    /// checked exactly as a bearer token is (the platform's introspection for
    /// this service's audience, then the tombstones). A session whose token
    /// the platform no longer honours is deleted.
    pub async fn authenticate(&self, cookie: &str) -> Result<Principal, AuthFailure> {
        let id_hash = console_sessions::hash_cookie(cookie);
        let (token, org) = self.access_token(&id_hash).await?;
        let i = &self.inner;
        match auth::authenticate_token(&i.db, &i.platform, &token).await {
            Ok(p) if p.org_id == org => Ok(p),
            Ok(p) => {
                // The platform says this token opens another org than the one
                // the session was stored under. Nothing legitimate does that.
                tracing::error!(session_org = %org, token_org = %p.org_id,
                    "a console session's token opens another org; dropping the session");
                self.forget(org, &id_hash).await;
                Err(AuthFailure::Inactive)
            }
            Err(AuthFailure::Inactive) => {
                // Revoked at the platform, or the member left: nothing will
                // revive it.
                self.forget(org, &id_hash).await;
                Err(AuthFailure::Inactive)
            }
            Err(e) => Err(e),
        }
    }

    /// A platform access token for the session `id_hash`, and the org it is
    /// bound to.
    async fn access_token(&self, id_hash: &[u8; 32]) -> Result<(String, OrgId), AuthFailure> {
        let db = &self.inner.db;
        let org = console_sessions::org_of(db, id_hash)
            .await
            .map_err(|e| db_failure("look up a console session", e))?
            .ok_or(AuthFailure::Inactive)?;
        let session = console_sessions::load(db, org, id_hash)
            .await
            .map_err(|e| db_failure("read a console session", e))?
            .ok_or(AuthFailure::Inactive)?;
        if !session.needs_refresh(Utc::now()) {
            return Ok((self.open_access(&session, id_hash).await?, org));
        }
        // Detached: see the module docs.
        let this = self.clone();
        let id_hash = *id_hash;
        let token = tokio::spawn(async move { this.refresh(org, id_hash).await })
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "the console session refresh task failed");
                AuthFailure::Unavailable
            })??;
        Ok((token, org))
    }

    /// Open a session's access token. One that cannot be opened (the key was
    /// rotated) is a dead session, not a server error.
    async fn open_access(
        &self,
        session: &console_sessions::Session,
        id_hash: &[u8; 32],
    ) -> Result<String, AuthFailure> {
        match session.access_token(&self.inner.settings.cipher) {
            Ok(token) => Ok(token),
            Err(_) => {
                tracing::warn!(
                    "a console session's token cannot be opened (was \
                    CXP_ENCRYPTION_KEY rotated?); dropping it"
                );
                self.forget(session.org_id, id_hash).await;
                Err(AuthFailure::Inactive)
            }
        }
    }

    async fn refresh(&self, org: OrgId, id_hash: [u8; 32]) -> Result<String, AuthFailure> {
        let i = &self.inner;
        let Some((mut tx, session)) = console_sessions::lock(&i.db, org, &id_hash)
            .await
            .map_err(|e| db_failure("lock a console session", e))?
        else {
            // Signed out, swept, or its member removed since it was found.
            return Err(AuthFailure::Inactive);
        };

        // The single-flight re-check: a request that waited on the lock finds
        // the winner's fresh token here and refreshes nothing.
        let fresh = !session.needs_refresh(Utc::now());
        let opened = if fresh {
            session.access_token(&i.settings.cipher)
        } else {
            session.refresh_token(&i.settings.cipher)
        };
        let Ok(opened) = opened else {
            // Deleted under the lock this transaction already holds.
            tracing::warn!(
                "a console session's tokens cannot be opened (was CXP_ENCRYPTION_KEY \
                rotated?); dropping it"
            );
            let _ = console_sessions::delete_locked(&mut tx, &id_hash).await;
            let _ = tx.commit().await;
            return Err(AuthFailure::Inactive);
        };
        if fresh {
            let _ = tx.commit().await;
            return Ok(opened);
        }
        let refresh_token = opened;

        match i
            .oauth
            .refresh(
                &i.settings.client_id,
                &refresh_token,
                &i.settings.resource_uri,
            )
            .await
        {
            Ok(tokens) => {
                console_sessions::rotate(
                    &mut tx,
                    &i.settings.cipher,
                    &id_hash,
                    &tokens.access_token,
                    &tokens.refresh_token,
                    access_expiry(&tokens),
                    Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS),
                )
                .await
                .map_err(|e| db_failure("store a refreshed console session", e))?;
                tx.commit()
                    .await
                    .map_err(|e| db_failure("store a refreshed console session", e.into()))?;
                Ok(tokens.access_token)
            }
            Err(TokenError::InvalidGrant) => {
                tracing::info!(org = %org,
                    "the platform refused a console session's refresh token; dropping the session");
                console_sessions::delete_locked(&mut tx, &id_hash)
                    .await
                    .map_err(|e| db_failure("drop a console session", e))?;
                tx.commit()
                    .await
                    .map_err(|e| db_failure("drop a console session", e.into()))?;
                Err(AuthFailure::Inactive)
            }
            Err(TokenError::Unavailable(reason)) => {
                // Untouched: as far as we know the refresh token is unspent.
                tracing::warn!(%reason, "could not refresh a console session; answering 503");
                Err(AuthFailure::Unavailable)
            }
            Err(TokenError::Rejected(reason)) => {
                tracing::error!(%reason,
                    "the platform refused the console's refresh request; is CXP_CONSOLE_CLIENT_ID \
                     registered at the platform?");
                Err(AuthFailure::Internal)
            }
        }
    }

    /// Forget a session that will never work again. Best effort.
    async fn forget(&self, org: OrgId, id_hash: &[u8; 32]) {
        if let Err(e) = console_sessions::delete(&self.inner.db, org, id_hash).await {
            tracing::warn!(error = %e.log_detail(), "could not delete a dead console session");
        }
    }
}

fn db_failure(what: &str, e: cxp_core::Error) -> AuthFailure {
    if matches!(e, cxp_core::Error::AccessRevoked) {
        return AuthFailure::Revoked;
    }
    tracing::error!(error = %e.log_detail(), source = ?std::error::Error::source(&e),
        "could not {what}; answering 503");
    AuthFailure::Unavailable
}

fn access_expiry(tokens: &TokenSet) -> chrono::DateTime<Utc> {
    let secs = tokens.expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS).max(1);
    Utc::now() + chrono::Duration::seconds(secs)
}

// ----------------------------------------------------------------- cookies

#[derive(Clone, Copy)]
struct Cookies {
    /// `__Host-` names and `Secure`: every deployment but plain-http loopback.
    secure: bool,
}

impl Cookies {
    fn session(self) -> &'static str {
        if self.secure {
            "__Host-cxp_session"
        } else {
            "cxp_session"
        }
    }

    fn oauth(self) -> &'static str {
        if self.secure {
            "__Host-cxp_oauth"
        } else {
            "cxp_oauth"
        }
    }

    fn set(self, name: &str, value: &str, max_age_secs: i64) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age={max_age_secs}"
        ))
        .expect("cookie values are base64url by construction")
    }

    /// Makes the browser drop `name`. Same attributes as when it was set: a
    /// clear that differs in `Path` or the `__Host-` requirements is ignored.
    fn clear(self, name: &str) -> HeaderValue {
        self.set(name, "", 0)
    }
}

/// A random 32-byte value, base64url (43 characters).
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn is_token_shaped(v: &str) -> bool {
    v.len() == 43
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The value of cookie `name`, matched on the exact name: a cookie called
/// `evil-__Host-cxp_session` is not ours.
fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(n, _)| n.trim() == name)
        .map(|(_, v)| v.trim())
}

/// `scheme://host[:port]` of `url`, as a browser serializes an `Origin`.
fn origin_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    url.has_host().then(|| url.origin().ascii_serialization())
}

/// Whether a request that authenticates with the session cookie came from
/// this origin. Safe methods always pass. Otherwise `Origin` must equal the
/// public origin exactly (a sibling subdomain's differs, and so does the
/// `null` of a sandboxed frame); with no `Origin`, `Sec-Fetch-Site` must be
/// `same-origin`; with neither, the request is refused. Browsers send `Origin`
/// on every non-GET fetch, so there is no `Referer` fallback. An empty
/// `origin` (an unparsable public URL) refuses everything.
pub fn same_origin(origin: &str, method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if origin.is_empty() {
        return false;
    }
    if let Some(value) = headers.get(header::ORIGIN) {
        return value.to_str().is_ok_and(|v| v == origin);
    }
    headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "same-origin")
}

/// `next` if it is certainly a path on this origin, else `None`: it starts
/// with a single `/` (no scheme, no authority), has no `\` (browsers read
/// `/\evil.test` as `//evil.test`), no control or whitespace characters
/// (browsers strip tabs and newlines, so `/\t/evil.test` is `//evil.test`),
/// and does not lead back into `/auth`.
pub fn safe_next(next: &str) -> Option<String> {
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && next.len() <= MAX_NEXT_LEN
        && !next
            .chars()
            .any(|c| c == '\\' || c.is_control() || c.is_whitespace())
        && next != "/auth"
        && !next.starts_with("/auth/")
        && !next.starts_with("/auth?");
    ok.then(|| next.to_string())
}

/// An org slug or id as the platform takes it in `org_hint`.
fn is_org_hint(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// RFC 7636 `S256`: base64url, unpadded, of the SHA-256 of the verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What `/auth/login` remembers for `/auth/callback`, sealed in a cookie.
#[derive(Serialize, Deserialize)]
struct OauthFlow {
    state: String,
    verifier: String,
    /// A same-origin path to land on afterwards, already checked by
    /// [`safe_next`].
    next: Option<String>,
    /// Unix seconds. The cookie's `Max-Age` is advisory; this is enforced.
    exp: i64,
}

// Not derived: the verifier is a secret until the code is redeemed.
impl std::fmt::Debug for OauthFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthFlow")
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl OauthFlow {
    fn seal(&self, cipher: &Cipher) -> Option<String> {
        let json = serde_json::to_vec(self).ok()?;
        let sealed = cipher.seal(&json).ok()?;
        let mut combined = sealed.nonce;
        combined.extend_from_slice(&sealed.ciphertext);
        Some(URL_SAFE_NO_PAD.encode(combined))
    }

    /// `None` for anything that is not an unexpired flow we sealed.
    fn open(cipher: &Cipher, value: &str, now: i64) -> Option<Self> {
        let combined = URL_SAFE_NO_PAD.decode(value).ok()?;
        if combined.len() < 12 {
            return None;
        }
        let (nonce, ciphertext) = combined.split_at(12);
        let json = cipher.open(ciphertext, nonce).ok()?;
        let flow: Self = serde_json::from_slice(&json).ok()?;
        (flow.exp > now).then_some(flow)
    }
}

// ---------------------------------------------------------- token endpoint

/// What the platform issues for an authorization code or a refresh token.
#[derive(Deserialize)]
struct TokenSet {
    access_token: String,
    refresh_token: String,
    /// Seconds until `access_token` expires.
    #[serde(default)]
    expires_in: Option<i64>,
}

// Not derived: neither token may reach a log line.
impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum TokenError {
    /// The code or refresh token is spent, expired, revoked, or never valid.
    InvalidGrant,
    /// Any other refusal (`invalid_client`, `invalid_target`, ...): the client
    /// registration or this configuration is wrong, not the person's doing.
    Rejected(String),
    /// No usable answer: transport failure, timeout, 5xx, 429, or a body that
    /// is not a token response.
    Unavailable(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
}

/// The platform's `/oauth/token` and `/oauth/revoke`, server to server. The
/// console is a public client: `client_id` is its whole identity, and PKCE is
/// what binds a code to the browser that asked for it.
struct PlatformOAuth {
    http: reqwest::Client,
    token_url: String,
    revoke_url: String,
}

impl PlatformOAuth {
    fn new(platform_url: &str) -> Self {
        let base = platform_url.trim_end_matches('/');
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                // The token endpoint never redirects; following one would send
                // a code or a refresh token wherever it pointed.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the TLS backend initialises"),
            token_url: format!("{base}/oauth/token"),
            revoke_url: format!("{base}/oauth/revoke"),
        }
    }

    async fn exchange_code(
        &self,
        client_id: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        resource: &str,
    ) -> Result<TokenSet, TokenError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
            ("client_id", client_id),
            ("resource", resource),
        ])
        .await
    }

    async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
        resource: &str,
    ) -> Result<TokenSet, TokenError> {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("resource", resource),
        ])
        .await
    }

    /// Revoke a refresh token. Only not reaching the platform is a failure,
    /// which the caller logs and moves past: signing out must work while the
    /// platform is down.
    async fn revoke(&self, client_id: &str, token: &str) -> Result<(), TokenError> {
        self.http
            .post(&self.revoke_url)
            .form(&[("token", token), ("client_id", client_id)])
            .send()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        Ok(())
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<TokenSet, TokenError> {
        let response = self
            .http
            .post(&self.token_url)
            .form(form)
            .send()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|_| TokenError::Unavailable("an unreadable token response".into()));
        }
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(TokenError::Unavailable(format!("HTTP {status}")));
        }
        let error = serde_json::from_slice::<ErrorBody>(&body)
            .map(|b| b.error)
            .unwrap_or_default();
        match error.as_str() {
            "invalid_grant" => Err(TokenError::InvalidGrant),
            // The platform's own error code, from a fixed vocabulary; logged,
            // never shown to the person.
            "" => Err(TokenError::Rejected(format!("HTTP {status}"))),
            other => Err(TokenError::Rejected(
                other
                    .chars()
                    .take(64)
                    .filter(|c| c.is_ascii_graphic())
                    .collect(),
            )),
        }
    }
}

// ------------------------------------------------------------------ routes

/// `/auth/login`, `/auth/callback`, `/auth/logout`, and `/auth/session`.
/// Mounted whether or not the console is enabled, so `/auth` is never served
/// the console's HTML: with `console` `None`, sign-in answers `503
/// console_login_disabled` and the session endpoint says so.
pub fn router(console: Option<Console>) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .route("/auth/session", get(session))
        .with_state(console)
}

fn json_error(status: StatusCode, code: &'static str, message: &str) -> Response {
    let mut res = (
        status,
        Json(serde_json::json!({ "error": message, "code": code })),
    )
        .into_response();
    no_store(&mut res);
    res
}

fn no_store(res: &mut Response) {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

fn disabled() -> Response {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "console_login_disabled",
        "console sign-in is not configured on this deployment (CXP_CONSOLE_CLIENT_ID is unset)",
    )
}

/// A `303` with `Set-Cookie`s, never cached: each carries or consumes
/// something single-use.
fn see_other(location: &str, set_cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let mut res = StatusCode::SEE_OTHER.into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static("/")),
    );
    for cookie in set_cookies {
        headers.append(header::SET_COOKIE, cookie);
    }
    no_store(&mut res);
    res
}

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
    org: Option<String>,
}

async fn login(State(console): State<Option<Console>>, Query(q): Query<LoginQuery>) -> Response {
    let Some(console) = console else {
        return disabled();
    };
    let s = &console.inner.settings;
    // An unsafe `next` is dropped rather than refused: the person still signs
    // in, and lands on the console's home.
    let flow = OauthFlow {
        state: random_token(),
        verifier: random_token(),
        next: q.next.as_deref().and_then(safe_next),
        exp: Utc::now().timestamp() + OAUTH_MAX_AGE_SECS,
    };
    let Some(sealed) = flow.seal(&s.cipher) else {
        tracing::error!("could not seal the console's oauth cookie");
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            cxp_core::error::INTERNAL_MESSAGE,
        );
    };
    let Ok(mut url) = url::Url::parse(&format!("{}/oauth/authorize", s.platform_url)) else {
        tracing::error!("CXP_PLATFORM_URL does not make an authorize URL");
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            cxp_core::error::INTERNAL_MESSAGE,
        );
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("response_type", "code")
            .append_pair("client_id", &s.client_id)
            .append_pair("redirect_uri", &console.redirect_uri())
            .append_pair("code_challenge", &pkce_challenge(&flow.verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &flow.state)
            .append_pair("resource", &s.resource_uri)
            .append_pair("scope", &CONSOLE_SCOPES.join(" "));
        if let Some(org) = q.org.as_deref().filter(|o| is_org_hint(o)) {
            pairs.append_pair("org_hint", org);
        }
    }
    let cookies = console.inner.cookies;
    see_other(
        url.as_str(),
        [cookies.set(cookies.oauth(), &sealed, OAUTH_MAX_AGE_SECS)],
    )
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(console): State<Option<Console>>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(console) = console else {
        return disabled();
    };
    let cookies = console.inner.cookies;
    let failed = |code: &'static str| {
        see_other(
            &format!("/?login_error={code}"),
            [cookies.clear(cookies.oauth())],
        )
    };
    let i = &console.inner;

    // The platform turned the person away. A fixed vocabulary: the query
    // string is attacker-controlled.
    if let Some(error) = q.error.as_deref() {
        return failed(match error {
            "access_denied" => "access_denied",
            "temporarily_unavailable" | "server_error" => "platform_unavailable",
            _ => "failed",
        });
    }
    // The flow this browser started, and nobody else's: without its cookie a
    // callback is someone else's sign-in being fed to this browser.
    let Some(flow) = read_cookie(&headers, cookies.oauth())
        .and_then(|v| OauthFlow::open(&i.settings.cipher, v, Utc::now().timestamp()))
    else {
        return failed("invalid_state");
    };
    let (Some(code), Some(state)) = (q.code.as_deref(), q.state.as_deref()) else {
        return failed("failed");
    };
    if !bool::from(state.as_bytes().ct_eq(flow.state.as_bytes())) {
        return failed("invalid_state");
    }

    let tokens = match i
        .oauth
        .exchange_code(
            &i.settings.client_id,
            code,
            &console.redirect_uri(),
            &flow.verifier,
            &i.settings.resource_uri,
        )
        .await
    {
        Ok(tokens) => tokens,
        Err(TokenError::InvalidGrant) => return failed("failed"),
        Err(TokenError::Rejected(reason)) => {
            tracing::error!(%reason,
                "the platform refused the console's code exchange; is CXP_CONSOLE_CLIENT_ID \
                 registered with the redirect URI {{CXP_PUBLIC_URL}}/auth/callback?");
            return failed("failed");
        }
        Err(TokenError::Unavailable(reason)) => {
            tracing::warn!(%reason, "the platform could not redeem the console's sign-in code");
            return failed("platform_unavailable");
        }
    };

    // Who did the platform just sign in, and to which org? Asked of the
    // platform for this service's audience, and checked against the
    // tombstones, exactly as a bearer token is: never read from the request.
    let principal = match auth::authenticate_token(&i.db, &i.platform, &tokens.access_token).await {
        Ok(p) => p,
        Err(AuthFailure::Unavailable) => return failed("platform_unavailable"),
        Err(_) => return failed("failed"),
    };

    let cookie = random_token();
    let stored = console_sessions::create(
        &i.db,
        &i.settings.cipher,
        &NewSession {
            id_hash: console_sessions::hash_cookie(&cookie),
            org_id: principal.org_id,
            user_id: principal.user_id,
            access_token: &tokens.access_token,
            refresh_token: &tokens.refresh_token,
            access_expires_at: access_expiry(&tokens),
            expires_at: Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS),
        },
    )
    .await;
    match stored {
        Ok(()) => {}
        // The org was deleted or the member removed while they signed in.
        Err(cxp_core::Error::AccessRevoked) => return failed("failed"),
        Err(e) => {
            tracing::error!(error = %e.log_detail(), "could not store a console session");
            return failed("unavailable");
        }
    }
    tracing::info!(org = %principal.org_id, user = %principal.user_id, "console sign-in");

    // `next` was checked going into the cookie and is checked again coming
    // out: the cookie is the one thing here a browser could have tampered
    // with, were the key ever lost.
    let landing = flow
        .next
        .as_deref()
        .and_then(safe_next)
        .unwrap_or_else(|| "/".to_string());
    see_other(
        &landing,
        [
            cookies.set(cookies.session(), &cookie, SESSION_TTL_DAYS * 86_400),
            cookies.clear(cookies.oauth()),
        ],
    )
}

/// Sign out: revoke the refresh token at the platform (best effort), delete
/// the session, clear the cookie. Holds the row lock, so it cannot interleave
/// with a refresh and revoke a token the refresh just spent.
async fn logout(State(console): State<Option<Console>>, headers: HeaderMap) -> Response {
    let Some(console) = console else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let cookies = console.inner.cookies;
    let mut done = StatusCode::NO_CONTENT.into_response();
    done.headers_mut()
        .append(header::SET_COOKIE, cookies.clear(cookies.session()));
    no_store(&mut done);

    let Some(cookie) = console.session_cookie(&headers) else {
        return done;
    };
    if !console.same_origin(&Method::POST, &headers) {
        return cross_site();
    }
    let i = &console.inner;
    let id_hash = console_sessions::hash_cookie(cookie);
    let org = match console_sessions::org_of(&i.db, &id_hash).await {
        Ok(Some(org)) => org,
        Ok(None) => return done,
        Err(e) => {
            tracing::error!(error = %e.log_detail(), "could not look up a console session to sign out");
            return unavailable();
        }
    };
    let (mut tx, session) = match console_sessions::lock(&i.db, org, &id_hash).await {
        Ok(Some(held)) => held,
        Ok(None) | Err(cxp_core::Error::AccessRevoked) => return done,
        Err(e) => {
            tracing::error!(error = %e.log_detail(), "could not lock a console session to sign out");
            return unavailable();
        }
    };
    if let Ok(refresh_token) = session.refresh_token(&i.settings.cipher) {
        if let Err(e) = i.oauth.revoke(&i.settings.client_id, &refresh_token).await {
            tracing::warn!(error = ?e, "could not revoke a console session's refresh token");
        }
    }
    let deleted = console_sessions::delete_locked(&mut tx, &id_hash).await;
    match (deleted, tx.commit().await) {
        (Ok(_), Ok(())) => done,
        (Err(e), _) => {
            tracing::error!(error = %e.log_detail(), "could not delete a console session");
            unavailable()
        }
        (_, Err(e)) => {
            tracing::error!(error = %e, "could not delete a console session");
            unavailable()
        }
    }
}

fn unavailable() -> Response {
    let mut res = json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "otto-cxp could not complete this right now; nothing was changed. Retry shortly",
    );
    res.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_static(auth::RETRY_AFTER_SECONDS),
    );
    res
}

/// The answer to a cookie-authenticated write from another origin.
pub(crate) fn cross_site() -> Response {
    json_error(
        StatusCode::FORBIDDEN,
        "cross_site_request",
        "this request did not come from this site, so it was refused",
    )
}

/// What `GET /auth/session` tells the console.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionView {
    signed_in: bool,
    login_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    org_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scopes: Option<Vec<String>>,
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

/// Whether this browser is signed in, and as whom. Always `200` unless the
/// answer cannot be known (`503`): the console decides what to show from the
/// body, not the status.
async fn session(State(console): State<Option<Console>>, headers: HeaderMap) -> Response {
    let signed_out = |login_enabled| SessionView {
        signed_in: false,
        login_enabled,
        user_id: None,
        org_id: None,
        role: None,
        scopes: None,
    };
    let view = match console {
        None => signed_out(false),
        Some(console) => match console.session_cookie(&headers) {
            None => signed_out(true),
            Some(cookie) => match console.authenticate(cookie).await {
                Ok(p) => SessionView {
                    signed_in: true,
                    login_enabled: true,
                    user_id: Some(p.user_id.as_uuid()),
                    org_id: Some(p.org_id.as_uuid()),
                    role: Some(role_name(p.role)),
                    scopes: Some(p.scopes),
                },
                Err(AuthFailure::Unavailable) => return unavailable(),
                Err(AuthFailure::Internal) => {
                    return json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal",
                        cxp_core::error::INTERNAL_MESSAGE,
                    )
                }
                Err(_) => signed_out(true),
            },
        },
    };
    let mut res = Json(view).into_response();
    no_store(&mut res);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn cipher(byte: u8) -> Cipher {
        Cipher::from_base64_key(&base64::engine::general_purpose::STANDARD.encode([byte; 32]))
            .unwrap()
    }

    /// RFC 7636 appendix B's worked example.
    #[test]
    fn the_pkce_challenge_matches_the_rfc_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn every_console_scope_is_one_this_service_knows() {
        for scope in CONSOLE_SCOPES {
            assert!(cxp_core::scopes::KNOWN.contains(scope), "{scope}");
        }
    }

    #[test]
    fn a_cookie_is_found_by_its_exact_name_and_only_if_well_formed() {
        let good = random_token();
        assert_eq!(good.len(), 43);
        assert!(is_token_shaped(&good));
        let h = headers(&[(
            "cookie",
            &format!("theme=dark; __Host-cxp_session={good}; x=1"),
        )]);
        assert_eq!(read_cookie(&h, "__Host-cxp_session"), Some(good.as_str()));
        let h = headers(&[("cookie", "evil-__Host-cxp_session=abc")]);
        assert_eq!(read_cookie(&h, "__Host-cxp_session"), None);
        for bad in ["short", "a'b", &"x".repeat(44), &"x".repeat(42)] {
            assert!(!is_token_shaped(bad), "{bad}");
        }
    }

    #[test]
    fn cookies_are_host_only_http_only_lax_and_secure_off_loopback() {
        let secure = Cookies { secure: true };
        let v = secure.set(secure.session(), "abc", 60);
        assert_eq!(
            v,
            "__Host-cxp_session=abc; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=60"
        );
        assert_eq!(
            secure.clear(secure.oauth()),
            "__Host-cxp_oauth=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"
        );
        let dev = Cookies { secure: false };
        assert_eq!(
            dev.set(dev.session(), "abc", 60),
            "cxp_session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=60"
        );
        for v in [secure.set("n", "v", 1), dev.set("n", "v", 1)] {
            assert!(!v.to_str().unwrap().contains("Domain"), "{v:?}");
        }
    }

    #[test]
    fn safe_next_accepts_paths_and_refuses_everything_else() {
        for ok in ["/", "/tickets", "/customers/1?tab=x#y"] {
            assert_eq!(safe_next(ok).as_deref(), Some(ok));
        }
        for bad in [
            "",
            "tickets",
            "//evil.test",
            "/\\evil.test",
            "https://evil.test",
            "javascript:alert(1)",
            "/\t/evil.test",
            "/x\r\nSet-Cookie: a=1",
            "/ spaced",
            "/auth",
            "/auth/login",
            "/auth?x",
            &format!("/{}", "a".repeat(600)),
        ] {
            assert_eq!(safe_next(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_flow_survives_its_cookie_and_expires() {
        let flow = OauthFlow {
            state: "s".into(),
            verifier: "v".into(),
            next: Some("/tickets".into()),
            exp: 1_000,
        };
        let sealed = flow.seal(&cipher(5)).unwrap();
        let back = OauthFlow::open(&cipher(5), &sealed, 999).unwrap();
        assert_eq!((back.state.as_str(), back.verifier.as_str()), ("s", "v"));
        assert_eq!(back.next.as_deref(), Some("/tickets"));
        assert!(
            OauthFlow::open(&cipher(5), &sealed, 1_000).is_none(),
            "expired"
        );
        assert!(
            OauthFlow::open(&cipher(6), &sealed, 0).is_none(),
            "wrong key"
        );
        assert!(OauthFlow::open(&cipher(5), "garbage", 0).is_none());
        assert!(!format!("{back:?}").contains("verifier"));
    }

    #[test]
    fn the_origin_is_scheme_host_and_port() {
        assert_eq!(
            origin_of("https://cxp.example.com/").as_deref(),
            Some("https://cxp.example.com")
        );
        assert_eq!(
            origin_of("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(origin_of("nonsense"), None);
    }

    #[test]
    fn a_cookie_write_must_prove_its_origin() {
        const ORIGIN: &str = "https://cxp.example.com";
        let post = |h: &[(&str, &str)]| same_origin(ORIGIN, &Method::POST, &headers(h));
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(same_origin(
                ORIGIN,
                &m,
                &headers(&[("origin", "https://evil.test")])
            ));
        }
        assert!(post(&[("origin", ORIGIN)]));
        for bad in [
            "https://evil.test",
            "https://blog.cxp.example.com",
            "null",
            "http://cxp.example.com",
            "https://cxp.example.com:8443",
        ] {
            assert!(!post(&[("origin", bad)]), "{bad}");
        }
        assert!(post(&[("sec-fetch-site", "same-origin")]));
        for bad in ["same-site", "cross-site", "none"] {
            assert!(!post(&[("sec-fetch-site", bad)]), "{bad}");
        }
        assert!(!post(&[]), "no proof at all");
        assert!(
            !post(&[("referer", "https://cxp.example.com/x")]),
            "Referer is not proof"
        );
        // A wrong Origin is not rescued by fetch metadata.
        assert!(!post(&[
            ("origin", "https://evil.test"),
            ("sec-fetch-site", "same-origin")
        ]));
        for m in [Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(!same_origin(ORIGIN, &m, &headers(&[])), "{m}");
        }
        // An unparsable public URL fails closed.
        assert!(!same_origin("", &Method::POST, &headers(&[("origin", "")])));
    }
}
