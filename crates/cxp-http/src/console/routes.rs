//! `/auth/login`, `/auth/callback`, `/auth/logout`, and `/auth/session`.
//!
//! `/auth/login` and `/auth/callback` are navigations, so every failure there
//! is a redirect to `/console/?login_error=<code>` from a fixed vocabulary;
//! nothing from the query string is reflected.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use cxp_core::auth::{self, AuthFailure};
use cxp_core::console_sessions::{self, NewSession};
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use otto_resource::Role;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use super::cookies::{random_token, read_cookie};
use super::oauth::{pkce_challenge, OauthFlow, TokenError};
use super::{access_expiry, Console, CONSOLE_HOME, CONSOLE_SCOPES, SESSION_TTL_DAYS};

/// How long a sign-in may take, from `/auth/login` to `/auth/callback`.
pub const OAUTH_MAX_AGE_SECS: i64 = 600;

/// Longest `next` path accepted.
const MAX_NEXT_LEN: usize = 512;

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

/// `next` if it is certainly a console path on this origin, else `None`: it
/// starts with `/console/` (so a single `/`, no scheme, no authority, and
/// never `/auth`, `/api`, or anything else the server answers itself), has no
/// `\` (browsers read `/\evil.test` as `//evil.test`), no control or
/// whitespace characters (browsers strip tabs and newlines), and no `.` or
/// `..` segment, plain or percent-encoded (`/console/../auth/login` is
/// `/auth/login` to a browser).
pub fn safe_next(next: &str) -> Option<String> {
    let path = next.split(['?', '#']).next().unwrap_or_default();
    let ok = next.starts_with(CONSOLE_HOME)
        && next.len() <= MAX_NEXT_LEN
        && !next
            .chars()
            .any(|c| c == '\\' || c.is_control() || c.is_whitespace())
        && !path.to_ascii_lowercase().contains("%2e")
        && !path.split('/').any(|s| s == "." || s == "..");
    ok.then(|| next.to_string())
}

/// An org slug or id as the platform takes it in `org_hint`.
fn is_org_hint(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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

fn internal() -> Response {
    json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        cxp_core::error::INTERNAL_MESSAGE,
    )
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
        HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static(CONSOLE_HOME)),
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
    let i = &console.inner;
    let s = &i.settings;
    // An unsafe `next` is dropped rather than refused: the person still signs
    // in, and lands on the console's home.
    let flow = OauthFlow {
        state: random_token(),
        verifier: random_token(),
        next: q.next.as_deref().and_then(safe_next),
        exp: Utc::now().timestamp() + OAUTH_MAX_AGE_SECS,
    };
    let Some(sealed) = flow.seal(&i.keys.oauth_flow) else {
        tracing::error!("could not seal the console's oauth cookie");
        return internal();
    };
    let Ok(mut url) = url::Url::parse(&format!("{}/oauth/authorize", s.platform_url)) else {
        tracing::error!("CXP_PLATFORM_URL does not make an authorize URL");
        return internal();
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
    let cookies = i.cookies;
    let Some(cookie) = cookies.set(cookies.oauth(), &sealed, OAUTH_MAX_AGE_SECS) else {
        tracing::error!("the sealed oauth cookie is not a header value");
        return internal();
    };
    see_other(url.as_str(), [cookie])
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
    let i = &console.inner;
    let cookies = i.cookies;
    let failed = |code: &'static str| {
        see_other(
            &format!("{CONSOLE_HOME}?login_error={code}"),
            [cookies.clear(cookies.oauth())],
        )
    };

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
        .and_then(|v| OauthFlow::open(&i.keys.oauth_flow, v, Utc::now().timestamp()))
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
        &i.keys.session_tokens,
        &NewSession {
            id_hash: console_sessions::hash_cookie(&cookie),
            org_id: principal.org_id,
            user_id: principal.user_id,
            access_token: &tokens.access_token,
            refresh_token: &tokens.refresh_token,
            access_expires_at: access_expiry(tokens.expires_in),
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

    // Signing in again replaces this browser's previous session: end it, so
    // its refresh token does not outlive the cookie that pointed at it.
    if let Some(prior) = console.session_cookie(&headers) {
        console.end_session(prior).await.ok();
    }

    let Some(session_cookie) = cookies.set(cookies.session(), &cookie, SESSION_TTL_DAYS * 86_400)
    else {
        tracing::error!("the session cookie is not a header value");
        return failed("unavailable");
    };
    // `next` was checked going into the cookie and is checked again coming
    // out: the cookie is the one thing here a browser could have tampered
    // with, were the key ever lost.
    let landing = flow
        .next
        .as_deref()
        .and_then(safe_next)
        .unwrap_or_else(|| CONSOLE_HOME.to_string());
    see_other(&landing, [session_cookie, cookies.clear(cookies.oauth())])
}

/// Sign out: delete the session and commit, then revoke its refresh token at
/// the platform with nothing held (best effort), and clear the cookie. A
/// refresh racing with this finds the row gone and stores nothing.
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
    match console.end_session(cookie).await {
        Ok(()) => done,
        Err(()) => unavailable(),
    }
}

fn unavailable() -> Response {
    let mut res = json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Savvagent CXP could not complete this right now; nothing was changed. Retry shortly",
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
                Err(AuthFailure::Internal) => return internal(),
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

    #[test]
    fn safe_next_accepts_console_paths_and_refuses_everything_else() {
        for ok in [
            "/console/",
            "/console/tickets",
            "/console/customers/1?tab=x#y",
            "/console/a.b/c..d",
        ] {
            assert_eq!(safe_next(ok).as_deref(), Some(ok));
        }
        for bad in [
            "",
            "/",
            "/console",
            "/consoles/x",
            "/tickets",
            "tickets",
            "//evil.test",
            "/\\evil.test",
            "/console/\\evil.test",
            "https://evil.test",
            "javascript:alert(1)",
            "/console/\t/evil.test",
            "/console/x\r\nSet-Cookie: a=1",
            "/console/ spaced",
            "/auth/login",
            "/api/v1/customers",
            "/console/../auth/login",
            "/console/./x",
            "/console/%2e%2e/auth/login",
            "/console/%2E./auth/login",
            "/console/x/..",
            &format!("/console/{}", "a".repeat(600)),
        ] {
            assert_eq!(safe_next(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_org_hint_is_a_slug_or_id() {
        assert!(is_org_hint("acme-co_1"));
        for bad in ["", "a b", "a&b=c", &"a".repeat(65)] {
            assert!(!is_org_hint(bad), "{bad}");
        }
    }
}
