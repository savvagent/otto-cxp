//! The web console's sign-in against a real Postgres and a stub otto
//! platform (introspection, `/oauth/token`, `/oauth/revoke`): the PKCE
//! sign-in, the session cookie as a credential on `/api/v1` (reads, and
//! writes only from this origin), a bearer deciding over a cookie, dead
//! sessions, the single-flight refresh (in a pool of five, without blocking
//! a lifecycle webhook), sign-out, signing in again, and the console switched
//! off.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use axum::{Form, Json, Router};
use chrono::Utc;
use cxp_api::{router, AppState, Config};
use cxp_core::usage::Meter;
use cxp_http::console::{pkce_challenge, Console, ConsoleKeys, ConsoleSettings};
use otto_resource::{ClientConfig, IntrospectionResponse, PlatformClient, Role, TokenKind};
use otto_tenant::crypto::Cipher;
use otto_tenant::Db;
use serde_json::{json, Value};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PUBLIC_URL: &str = "https://cxp.example.com";
const RESOURCE: &str = "https://cxp.example.com/mcp";
const CLIENT_ID: &str = "cxp-console";
const KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const SESSION: &str = "__Host-cxp_session";
const OAUTH: &str = "__Host-cxp_oauth";

// ------------------------------------------------------------- stub platform

/// A grant the platform holds: (org, user, scopes).
type Grant = (Uuid, Uuid, Vec<String>);

#[derive(Default)]
struct Platform {
    /// Live access tokens.
    tokens: HashMap<String, Grant>,
    /// Authorization codes: code -> (grant, PKCE challenge).
    codes: HashMap<String, (Grant, String)>,
    /// Refresh tokens: token -> (grant, spent).
    refresh: HashMap<String, (Grant, bool)>,
    /// Refresh tokens revoked through /oauth/revoke.
    revoked: HashSet<String>,
    /// How many refreshes the platform answered with a new pair.
    refreshes: usize,
    /// `expires_in` on every issued access token.
    expires_in: i64,
    /// Answer /oauth/token with a 503.
    token_down: bool,
    /// Issue access tokens that introspection does not know (the platform
    /// revoked them, or the member left, right after issuing).
    issue_inactive: bool,
    /// What the token endpoint was last sent, minus the secrets.
    last_form: HashMap<String, String>,
    /// How long /oauth/token takes to answer a refresh.
    refresh_delay: Duration,
    /// Refresh requests received, answered or not yet.
    refresh_requests: usize,
}

type Shared = Arc<Mutex<Platform>>;

fn new_token(prefix: &str) -> String {
    format!("{prefix}{}{}", "a".repeat(11), Uuid::new_v4().simple())
}

fn issue(p: &mut Platform, grant: Grant) -> Value {
    let (access, refresh) = (new_token("otto_at_"), new_token("otto_rt_"));
    if !p.issue_inactive {
        p.tokens.insert(access.clone(), grant.clone());
    }
    p.refresh.insert(refresh.clone(), (grant, false));
    json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": p.expires_in,
    })
}

async fn introspect(
    State(p): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Json<IntrospectionResponse> {
    let p = p.lock().unwrap();
    Json(match p.tokens.get(&f["token"]) {
        Some((org, user, scopes)) => IntrospectionResponse {
            active: true,
            sub: Some(*user),
            org_id: Some(*org),
            role: Some(Role::Admin),
            scope: Some(scopes.join(" ")),
            aud: Some(RESOURCE.into()),
            exp: Some(Utc::now().timestamp() + 3600),
            client_id: Some(CLIENT_ID.into()),
            token_type: Some("Bearer".into()),
            token_kind: Some(TokenKind::Oauth),
            jti: Some(Uuid::new_v4()),
        },
        None => IntrospectionResponse::inactive(),
    })
}

fn invalid_grant() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "invalid_grant"})),
    )
}

async fn token(
    State(shared): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if f.get("grant_type").map(String::as_str) == Some("refresh_token") {
        let delay = {
            let mut p = shared.lock().unwrap();
            p.refresh_requests += 1;
            p.refresh_delay
        };
        tokio::time::sleep(delay).await;
    }
    let mut p = shared.lock().unwrap();
    if p.token_down {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
    }
    p.last_form = f
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "code" | "code_verifier" | "refresh_token"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if f.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || f.get("resource").map(String::as_str) != Some(RESOURCE)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_client"})),
        );
    }
    match f["grant_type"].as_str() {
        "authorization_code" => {
            let Some((grant, challenge)) = p.codes.remove(&f["code"]) else {
                return invalid_grant();
            };
            if pkce_challenge(&f["code_verifier"]) != challenge
                || f["redirect_uri"] != format!("{PUBLIC_URL}/auth/callback")
            {
                return invalid_grant();
            }
            (StatusCode::OK, Json(issue(&mut p, grant)))
        }
        "refresh_token" => {
            let Some((grant, spent)) = p.refresh.get(&f["refresh_token"]).cloned() else {
                return invalid_grant();
            };
            if spent {
                return invalid_grant();
            }
            p.refresh.get_mut(&f["refresh_token"]).unwrap().1 = true;
            p.refreshes += 1;
            (StatusCode::OK, Json(issue(&mut p, grant)))
        }
        _ => invalid_grant(),
    }
}

async fn revoke(State(p): State<Shared>, Form(f): Form<HashMap<String, String>>) -> StatusCode {
    p.lock().unwrap().revoked.insert(f["token"].clone());
    StatusCode::OK
}

async fn start_platform() -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Platform {
        expires_in: 3600,
        ..Default::default()
    }));
    let app = Router::new()
        .route("/oauth/introspect", post(introspect))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), shared)
}

// ------------------------------------------------------------------ harness

struct Harness {
    app: Router,
    stub: Shared,
    pool: PgPool,
}

async fn harness_with(pool: PgPool, console: bool) -> Harness {
    let (base, stub) = start_platform().await;
    let platform = Arc::new(
        PlatformClient::new(ClientConfig::new(base.clone(), RESOURCE, "otto_rs_test")).unwrap(),
    );
    let db = Db::from_pool(pool.clone());
    let meter = Arc::new(Meter::new(platform.clone(), false, "https://x.example"));
    let mut state = AppState::new(
        db.clone(),
        platform.clone(),
        meter,
        Config {
            public_url: PUBLIC_URL.into(),
            webhook_secret: "otto_whsec_test".into(),
        },
    );
    let console = console.then(|| {
        Console::new(
            db,
            platform,
            ConsoleSettings {
                client_id: CLIENT_ID.into(),
                keys: ConsoleKeys::derive(KEY).unwrap(),
                public_url: PUBLIC_URL.into(),
                platform_url: base,
                resource_uri: RESOURCE.into(),
            },
        )
    });
    if let Some(c) = &console {
        state = state.with_console(c.clone());
    }
    Harness {
        app: router(state).merge(cxp_http::console::router(console)),
        stub,
        pool,
    }
}

async fn harness(pool: PgPool) -> Harness {
    harness_with(pool, true).await
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    fn location(&self) -> &str {
        self.headers["location"].to_str().unwrap()
    }

    /// The `Set-Cookie` for `name`, if any.
    fn set_cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .find(|v| v.starts_with(&format!("{name}=")))
    }

    fn cookie_value(&self, name: &str) -> String {
        let c = self.set_cookie(name).expect("cookie set");
        c[name.len() + 1..].split(';').next().unwrap().to_string()
    }
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

impl Harness {
    async fn send(&self, req: Request<Body>) -> Reply {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    /// A request with the given headers and an optional JSON body.
    async fn call(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        self.send(req).await
    }

    /// The whole sign-in, as a browser and the platform would do it, for a
    /// fresh member of a fresh org. Returns (org, user, session cookie).
    async fn sign_in(&self) -> (Uuid, Uuid, String) {
        let (org, user) = (Uuid::new_v4(), Uuid::new_v4());
        let cookie = self.sign_in_as(org, user, "/console/tickets").await;
        (org, user, cookie)
    }

    async fn sign_in_as(&self, org: Uuid, user: Uuid, next: &str) -> String {
        self.sign_in_with(org, user, next, None).await
    }

    /// [`Self::sign_in_as`] from a browser that may already hold a session
    /// cookie.
    async fn sign_in_with(&self, org: Uuid, user: Uuid, next: &str, prior: Option<&str>) -> String {
        let login = self
            .call("GET", &format!("/auth/login?next={next}"), &[], None)
            .await;
        assert_eq!(login.status, StatusCode::SEE_OTHER);
        let authorize = query(login.location());
        let oauth = login.cookie_value(OAUTH);
        // The person signs in at the platform, which redirects back.
        let code = Uuid::new_v4().to_string();
        self.stub.lock().unwrap().codes.insert(
            code.clone(),
            (
                (org, user, vec!["cxp:read".into(), "cxp:write".into()]),
                authorize["code_challenge"].clone(),
            ),
        );
        let back = self
            .call(
                "GET",
                &format!("/auth/callback?code={code}&state={}", authorize["state"]),
                &[(
                    "cookie",
                    &match prior {
                        Some(c) => format!("{OAUTH}={oauth}; {SESSION}={c}"),
                        None => format!("{OAUTH}={oauth}"),
                    },
                )],
                None,
            )
            .await;
        assert_eq!(back.status, StatusCode::SEE_OTHER, "{:?}", back.headers);
        assert_eq!(back.location(), next);
        back.cookie_value(SESSION)
    }

    async fn sessions(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM console_sessions")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn with_cookie(cookie: &str) -> String {
    format!("theme=dark; {SESSION}={cookie}")
}

// -------------------------------------------------------------------- tests

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_console_signs_in_through_the_platform(pool: PgPool) {
    let h = harness(pool).await;

    let login = h
        .call(
            "GET",
            "/auth/login?next=/console/customers&org=acme",
            &[],
            None,
        )
        .await;
    assert_eq!(login.status, StatusCode::SEE_OTHER);
    assert_eq!(login.headers["cache-control"], "no-store");
    let to = login.location();
    assert!(to.starts_with("http://127.0.0.1:"), "{to}");
    assert!(url::Url::parse(to).unwrap().path() == "/oauth/authorize");
    let q = query(to);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], "https://cxp.example.com/auth/callback");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["code_challenge"].len(), 43);
    assert_eq!(q["resource"], RESOURCE);
    assert_eq!(q["scope"], "cxp:read cxp:write");
    assert_eq!(q["org_hint"], "acme");
    assert_eq!(q["state"].len(), 43);
    let oauth = login.set_cookie(OAUTH).unwrap();
    for attr in [
        "Path=/",
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Max-Age=600",
    ] {
        assert!(oauth.contains(attr), "{oauth}");
    }
    assert!(!oauth.contains("Domain"), "{oauth}");
    // The verifier never travels in the clear.
    assert!(!to.contains("verifier"));

    let (org, user) = (Uuid::new_v4(), Uuid::new_v4());
    let cookie = h.sign_in_as(org, user, "/console/customers").await;
    let form = h.stub.lock().unwrap().last_form.clone();
    assert_eq!(form["grant_type"], "authorization_code");

    // One session, its tokens sealed.
    assert_eq!(h.sessions().await, 1);
    let stored: (Uuid, Uuid, Vec<u8>) =
        sqlx::query_as("SELECT org_id, user_id, access_token_enc FROM console_sessions")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!((stored.0, stored.1), (org, user));
    assert!(!stored.2.windows(7).any(|w| w == b"otto_at"));
    // Sealed with the session-token subkey, never the configured key itself
    // or the oauth cookie's subkey.
    let keys = ConsoleKeys::derive(KEY).unwrap();
    let opened = cxp_core::sealed::open(&keys.session_tokens, &stored.2).unwrap();
    assert!(opened.starts_with(b"otto_at_"));
    assert!(cxp_core::sealed::open(&Cipher::from_base64_key(KEY).unwrap(), &stored.2).is_err());
    assert!(cxp_core::sealed::open(&keys.oauth_flow, &stored.2).is_err());

    // The cookie reads /api/v1, and the session endpoint knows who it is.
    let r = h
        .call(
            "GET",
            "/api/v1/customers",
            &[("cookie", &with_cookie(&cookie))],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = h
        .call(
            "GET",
            "/auth/session",
            &[("cookie", &with_cookie(&cookie))],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(
        r.body,
        json!({
            "signedIn": true,
            "loginEnabled": true,
            "userId": user,
            "orgId": org,
            "role": "admin",
            "scopes": ["cxp:read", "cxp:write"],
        })
    );
    let r = h.call("GET", "/auth/session", &[], None).await;
    assert_eq!(r.body, json!({"signedIn": false, "loginEnabled": true}));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_session_cookie_is_host_only_http_only_and_lax(pool: PgPool) {
    let h = harness(pool).await;
    let login = h.call("GET", "/auth/login", &[], None).await;
    let q = query(login.location());
    let code = Uuid::new_v4().to_string();
    h.stub.lock().unwrap().codes.insert(
        code.clone(),
        (
            (Uuid::new_v4(), Uuid::new_v4(), vec!["cxp:read".into()]),
            q["code_challenge"].clone(),
        ),
    );
    let back = h
        .call(
            "GET",
            &format!("/auth/callback?code={code}&state={}", q["state"]),
            &[("cookie", &format!("{OAUTH}={}", login.cookie_value(OAUTH)))],
            None,
        )
        .await;
    // No `next`: the console's home.
    assert_eq!(back.location(), "/console/");
    let session = back.set_cookie(SESSION).unwrap();
    for attr in [
        "Path=/",
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        &format!("Max-Age={}", 30 * 86_400),
    ] {
        assert!(session.contains(attr), "{session}");
    }
    assert!(!session.contains("Domain"), "{session}");
    // The in-flight sign-in is cleared.
    let cleared = back.set_cookie(OAUTH).unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_failed_callback_reflects_nothing_and_stores_nothing(pool: PgPool) {
    let h = harness(pool).await;
    let login = h
        .call("GET", "/auth/login?next=//evil.test", &[], None)
        .await;
    let q = query(login.location());
    let oauth = format!("{OAUTH}={}", login.cookie_value(OAUTH));
    let code = Uuid::new_v4().to_string();
    h.stub.lock().unwrap().codes.insert(
        code.clone(),
        (
            (Uuid::new_v4(), Uuid::new_v4(), vec!["cxp:read".into()]),
            q["code_challenge"].clone(),
        ),
    );
    for (uri, cookie, expected) in [
        (
            format!("/auth/callback?code={code}&state={}", q["state"]),
            None,
            "invalid_state",
        ),
        (
            format!("/auth/callback?code={code}&state=forged"),
            Some(oauth.as_str()),
            "invalid_state",
        ),
        (
            "/auth/callback?error=access_denied".to_string(),
            Some(oauth.as_str()),
            "access_denied",
        ),
        (
            "/auth/callback?error=%3Cscript%3E".to_string(),
            Some(oauth.as_str()),
            "failed",
        ),
        (
            format!("/auth/callback?code=unknown&state={}", q["state"]),
            Some(oauth.as_str()),
            "failed",
        ),
    ] {
        let headers: Vec<(&str, &str)> = cookie.map(|c| ("cookie", c)).into_iter().collect();
        let r = h.call("GET", &uri, &headers, None).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{uri}");
        assert_eq!(
            r.location(),
            format!("/console/?login_error={expected}"),
            "{uri}"
        );
        assert!(r.set_cookie(SESSION).is_none(), "{uri}");
    }
    assert_eq!(h.sessions().await, 0);

    // An unsafe `next` is dropped, not followed.
    let back = h
        .call(
            "GET",
            &format!("/auth/callback?code={code}&state={}", q["state"]),
            &[("cookie", &oauth)],
            None,
        )
        .await;
    assert_eq!(back.location(), "/console/");
    assert_eq!(h.sessions().await, 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_cookie_write_must_come_from_this_origin(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, cookie) = h.sign_in().await;
    let cookie = with_cookie(&cookie);
    let create = |headers: Vec<(&'static str, String)>| {
        let h = &h;
        let cookie = cookie.clone();
        async move {
            let mut all: Vec<(&str, &str)> = vec![("cookie", cookie.as_str())];
            all.extend(headers.iter().map(|(k, v)| (*k, v.as_str())));
            h.call(
                "POST",
                "/api/v1/customers",
                &all,
                Some(json!({"email": format!("{}@example.com", Uuid::new_v4())})),
            )
            .await
        }
    };

    // An expiring token: a refused write must not even refresh it.
    sqlx::query("UPDATE console_sessions SET access_expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();
    for refused in [
        vec![],
        vec![("origin", "https://evil.test".to_string())],
        vec![("origin", "https://blog.cxp.example.com".to_string())],
        vec![("origin", "null".to_string())],
        vec![("sec-fetch-site", "same-site".to_string())],
        vec![("referer", "https://cxp.example.com/customers".to_string())],
    ] {
        let r = create(refused.clone()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{refused:?}");
        assert_eq!(r.body["code"], "cross_site_request", "{refused:?}");
    }
    assert_eq!(
        h.stub.lock().unwrap().refreshes,
        0,
        "a refused write refreshed"
    );
    for allowed in [
        vec![("origin", PUBLIC_URL.to_string())],
        vec![("sec-fetch-site", "same-origin".to_string())],
    ] {
        let r = create(allowed.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{allowed:?}: {}", r.body);
    }
    // Only the two allowed writes happened.
    let r = h
        .call("GET", "/api/v1/customers", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(r.body["total"], 2);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_bearer_decides_over_a_cookie(pool: PgPool) {
    let h = harness(pool).await;
    let (org, user, cookie) = h.sign_in().await;
    let bearer = new_token("otto_at_");
    h.stub.lock().unwrap().tokens.insert(
        bearer.clone(),
        (org, user, vec!["cxp:read".into(), "cxp:write".into()]),
    );
    let cookie = with_cookie(&cookie);

    // A bearer write needs no origin proof, cookie or not.
    let r = h
        .call(
            "POST",
            "/api/v1/customers",
            &[
                ("authorization", &format!("Bearer {bearer}")),
                ("cookie", &cookie),
                ("origin", "https://evil.test"),
            ],
            Some(json!({"email": "a@example.com"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    // A dead bearer is not rescued by a live cookie.
    let r = h
        .call(
            "GET",
            "/api/v1/customers",
            &[
                (
                    "authorization",
                    &format!("Bearer {}", new_token("otto_at_")),
                ),
                ("cookie", &cookie),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"], "invalid_token");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_dead_session_is_401(pool: PgPool) {
    let h = harness(pool).await;
    let get = |cookie: String| {
        let h = &h;
        async move {
            h.call(
                "GET",
                "/api/v1/customers",
                &[("cookie", &with_cookie(&cookie))],
                None,
            )
            .await
        }
    };

    // Malformed and unknown cookies.
    for bad in ["short".to_string(), "x".repeat(43)] {
        let r = get(bad).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        assert!(r.headers["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("resource_metadata="));
    }

    // Expired.
    let (_, _, cookie) = h.sign_in().await;
    sqlx::query("UPDATE console_sessions SET expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(get(cookie).await.status, StatusCode::UNAUTHORIZED);

    // The platform stopped honouring its token: refused, and forgotten. (A
    // token introspected once is cached for a minute, so the one the session
    // holds is a fresh, refreshed one the platform never vouches for.)
    sqlx::query("DELETE FROM console_sessions")
        .execute(&h.pool)
        .await
        .unwrap();
    let (_, _, cookie) = h.sign_in().await;
    sqlx::query("UPDATE console_sessions SET access_expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();
    h.stub.lock().unwrap().issue_inactive = true;
    let r = get(cookie).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"], "invalid_token");
    assert_eq!(h.sessions().await, 0);
    h.stub.lock().unwrap().issue_inactive = false;

    // The org deleted, or the member removed: the webhooks purge it.
    for kind in ["org.deleted", "member.removed"] {
        let (org, user, cookie) = h.sign_in().await;
        assert_eq!(get(cookie.clone()).await.status, StatusCode::OK);
        let body = otto_resource::webhook::body(
            Uuid::new_v4(),
            kind,
            Utc::now(),
            &json!({"org_id": org, "user_id": user}),
        );
        let sig = otto_resource::webhook::sign("otto_whsec_test", Utc::now().timestamp(), &body);
        let r = h
            .send(
                Request::post("/platform/webhooks")
                    .header("content-type", "application/json")
                    .header(otto_resource::webhook::SIGNATURE_HEADER, sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{kind}: {}", r.body);
        assert_eq!(get(cookie).await.status, StatusCode::UNAUTHORIZED, "{kind}");
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM console_sessions WHERE org_id = $1")
                .bind(org)
                .fetch_one(&h.pool)
                .await
                .unwrap();
        assert_eq!(left, 0, "{kind} left the session behind");
    }
}

/// The app's pool as production sizes it (`CXP_DB_MAX_CONNECTIONS`): five.
async fn pool_of_five(opts: PgPoolOptions, connect: PgConnectOptions) -> PgPool {
    opts.max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(connect)
        .await
        .unwrap()
}

/// The platform rotates the refresh token on every use and revokes the
/// family on a replay, so concurrent requests behind an expiring token must
/// refresh it exactly once, and all succeed. More requests than the pool has
/// connections, while the platform takes its time: none may hold a
/// connection while it waits on the refresh.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_expiring_token_is_refreshed_once_however_many_requests_need_it(
    opts: PgPoolOptions,
    connect: PgConnectOptions,
) {
    let pool = pool_of_five(opts, connect).await;
    let h = harness(pool.clone()).await;
    let (_, _, cookie) = h.sign_in().await;
    sqlx::query("UPDATE console_sessions SET access_expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();
    h.stub.lock().unwrap().refresh_delay = Duration::from_millis(500);

    let cookie = with_cookie(&cookie);
    let headers = [("cookie", cookie.as_str())];
    let calls = (0..8).map(|_| h.call("GET", "/api/v1/customers", &headers, None));
    for r in futures::future::join_all(calls).await {
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    }
    let (refreshes, requests, grant) = {
        let p = h.stub.lock().unwrap();
        (
            p.refreshes,
            p.refresh_requests,
            p.last_form["grant_type"].clone(),
        )
    };
    assert_eq!(
        (refreshes, requests, grant.as_str()),
        (1, 1, "refresh_token")
    );
    let (fresh, generation, leased): (bool, i64, bool) = sqlx::query_as(
        "SELECT access_expires_at > now() + interval '30 minutes', generation, \
                refresh_lease_until IS NOT NULL \
         FROM console_sessions",
    )
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!((fresh, generation, leased), (true, 1, false));
    drop(h);
    pool.close().await;
}

/// Another replica holds the refresh lease: this one waits for its result
/// rather than spending the same refresh token.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_leased_refresh_is_waited_for_not_repeated(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, cookie) = h.sign_in().await;
    let cookie = with_cookie(&cookie);
    // Another replica claimed the refresh a moment ago...
    sqlx::query(
        "UPDATE console_sessions SET access_expires_at = now(), \
         refresh_lease_until = now() + interval '15 seconds'",
    )
    .execute(&h.pool)
    .await
    .unwrap();
    let headers = [("cookie", cookie.as_str())];
    let request = h.call("GET", "/api/v1/customers", &headers, None);
    // ...and stores its fresh pair half a second later.
    let other_replica = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        sqlx::query(
            "UPDATE console_sessions SET access_expires_at = now() + interval '1 hour', \
             refresh_lease_until = NULL, generation = generation + 1",
        )
        .execute(&h.pool)
        .await
        .unwrap();
    };
    let (r, ()) = tokio::join!(request, other_replica);
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(h.stub.lock().unwrap().refresh_requests, 0);
}

/// A refresh holds no connection or lock while the platform answers, so an
/// `org.deleted` purge of the same org goes through at once; the refresh
/// then finds its session gone, stores nothing, and revokes the pair it was
/// issued.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_slow_refresh_does_not_block_an_org_deleted_webhook(
    opts: PgPoolOptions,
    connect: PgConnectOptions,
) {
    let pool = pool_of_five(opts, connect).await;
    let h = harness(pool.clone()).await;
    let (org, _, cookie) = h.sign_in().await;
    let cookie = with_cookie(&cookie);
    sqlx::query("UPDATE console_sessions SET access_expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();
    let slow = Duration::from_secs(4);
    h.stub.lock().unwrap().refresh_delay = slow;

    let headers = [("cookie", cookie.as_str())];
    let request = h.call("GET", "/api/v1/customers", &headers, None);
    let webhook = async {
        // Once the refresh is waiting on the platform...
        while h.stub.lock().unwrap().refresh_requests == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let body = otto_resource::webhook::body(
            Uuid::new_v4(),
            "org.deleted",
            Utc::now(),
            &json!({"org_id": org}),
        );
        let sig = otto_resource::webhook::sign("otto_whsec_test", Utc::now().timestamp(), &body);
        let started = Instant::now();
        let r = h
            .send(
                Request::post("/platform/webhooks")
                    .header("content-type", "application/json")
                    .header(otto_resource::webhook::SIGNATURE_HEADER, sig)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        (r, started.elapsed())
    };
    let (r, (hook, took)) = tokio::join!(request, webhook);
    assert_eq!(hook.status, StatusCode::OK, "{}", hook.body);
    assert!(
        took < slow / 2,
        "org.deleted waited {took:?} on a refresh's platform call"
    );
    // The purge won: the request is refused, and the session stays gone.
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    assert_eq!(h.sessions().await, 0);
    let (refreshes, revoked) = {
        let p = h.stub.lock().unwrap();
        (p.refreshes, p.revoked.len())
    };
    assert_eq!(refreshes, 1);
    assert_eq!(revoked, 1, "the orphaned pair was not revoked");
    drop(h);
    pool.close().await;
}

/// A session is its signed-in user's: a token the platform says is someone
/// else's is refused, and the session dropped.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_session_whose_token_is_another_users_is_refused(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, cookie) = h.sign_in().await;
    sqlx::query("UPDATE console_sessions SET user_id = gen_random_uuid()")
        .execute(&h.pool)
        .await
        .unwrap();
    let r = h
        .call(
            "GET",
            "/api/v1/customers",
            &[("cookie", &with_cookie(&cookie))],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.sessions().await, 0);
}

/// Signing in again from a browser that already holds a session replaces it:
/// the old session is deleted and its refresh token revoked.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn signing_in_again_ends_the_previous_session(pool: PgPool) {
    let h = harness(pool).await;
    let (org, user, first) = h.sign_in().await;
    let second = h.sign_in_with(org, user, "/console/", Some(&first)).await;
    assert_ne!(first, second);
    assert_eq!(h.sessions().await, 1);
    assert_eq!(h.stub.lock().unwrap().revoked.len(), 1);
    let get = |c: String| {
        let h = &h;
        async move {
            h.call(
                "GET",
                "/api/v1/customers",
                &[("cookie", &with_cookie(&c))],
                None,
            )
            .await
            .status
        }
    };
    assert_eq!(get(first).await, StatusCode::UNAUTHORIZED);
    assert_eq!(get(second).await, StatusCode::OK);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_refused_refresh_signs_out_and_an_outage_does_not(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, cookie) = h.sign_in().await;
    let cookie = with_cookie(&cookie);
    sqlx::query("UPDATE console_sessions SET access_expires_at = now()")
        .execute(&h.pool)
        .await
        .unwrap();

    // The platform is down: 503, never 401, and the session is kept.
    h.stub.lock().unwrap().token_down = true;
    let r = h
        .call("GET", "/api/v1/customers", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers["retry-after"], "5");
    assert_eq!(h.sessions().await, 1);

    // The refresh token was revoked at the platform: signed out.
    {
        let mut p = h.stub.lock().unwrap();
        p.token_down = false;
        for (_, spent) in p.refresh.values_mut() {
            *spent = true;
        }
    }
    let r = h
        .call("GET", "/api/v1/customers", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.sessions().await, 0);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn signing_out_revokes_and_forgets_the_session(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, cookie) = h.sign_in().await;
    let cookie = with_cookie(&cookie);

    let r = h
        .call(
            "POST",
            "/auth/logout",
            &[("cookie", &cookie), ("origin", "https://evil.test")],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(h.sessions().await, 1);

    let r = h
        .call(
            "POST",
            "/auth/logout",
            &[("cookie", &cookie), ("origin", PUBLIC_URL)],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.set_cookie(SESSION).unwrap().contains("Max-Age=0"));
    assert_eq!(h.sessions().await, 0);
    assert_eq!(h.stub.lock().unwrap().revoked.len(), 1);

    let r = h
        .call("GET", "/api/v1/customers", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Signing out again, or with no session, is fine.
    let r = h
        .call(
            "POST",
            "/auth/logout",
            &[("cookie", &cookie), ("origin", PUBLIC_URL)],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

/// Merging with no console configured must change nothing: sign-in is off,
/// and a session cookie (one left from when it was on) is no credential.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn with_the_console_off_sign_in_is_503_and_a_cookie_is_no_credential(pool: PgPool) {
    let on = harness(pool.clone()).await;
    let (_, _, cookie) = on.sign_in().await;

    let off = harness_with(pool, false).await;
    for uri in ["/auth/login", "/auth/callback?code=x&state=y"] {
        let r = off.call("GET", uri, &[], None).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(r.body["code"], "console_login_disabled");
    }
    let r = off.call("GET", "/auth/session", &[], None).await;
    assert_eq!(r.body, json!({"signedIn": false, "loginEnabled": false}));
    let r = off
        .call(
            "GET",
            "/api/v1/customers",
            &[("cookie", &with_cookie(&cookie))],
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"], "unauthenticated");
}
