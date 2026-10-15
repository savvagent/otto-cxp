//! `cxp-api` against a real Postgres and a stub otto platform: the bearer
//! gate (401 with the discovery challenge, wrong audience, platform outage),
//! scope checks, route shapes, metering (only `POST /api/v1/surveys`
//! charges), `/me/teams`, catalog team verification and the team field's
//! absent/null/id semantics, a cross-org read, the signed webhooks, and the
//! body cap. The every-route cross-org e2e lives with the binary (#12).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use chrono::Utc;
use cxp_api::{router, AppState, Config};
use cxp_core::usage::Meter;
use otto_resource::{
    ClientConfig, IntrospectionResponse, MemberInfo, MemberTeams, OrgInfo, PlatformClient, Role,
    TeamInfo, TokenKind, UsageBatch, UsageReceipt, UserInfo,
};
use otto_tenant::Db;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PUBLIC_URL: &str = "https://cxp.example.com";
const RESOURCE: &str = "https://cxp.example.com/mcp";
const OTHER_RESOURCE: &str = "https://flags.example.com/mcp";
const SECRET: &str = "otto_whsec_test";
const RW: &[&str] = &["cxp:read", "cxp:write"];
const RO: &[&str] = &["cxp:read"];

// ------------------------------------------------------------- stub platform

#[derive(Default)]
struct Platform {
    /// token -> (org, user, scopes, audience)
    tokens: HashMap<String, (Uuid, Uuid, Vec<String>, String)>,
    /// team -> (org, slug, name)
    teams: HashMap<Uuid, (Uuid, String, String)>,
    /// (org, user) -> the user's teams; absent means "not a member".
    members: HashMap<(Uuid, Uuid), Vec<Uuid>>,
    /// Answer introspection with a 500.
    introspection_down: bool,
    /// Answer team and member-team lookups with a 503.
    teams_down: bool,
    /// Report the org's allowance used up with a hard stop.
    blocked: bool,
}

type Shared = Arc<Mutex<Platform>>;

async fn introspect(
    State(p): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Json<IntrospectionResponse>, StatusCode> {
    let p = p.lock().unwrap();
    if p.introspection_down {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(Json(match p.tokens.get(&f["token"]) {
        Some((org, user, scopes, aud)) => IntrospectionResponse {
            active: true,
            sub: Some(*user),
            org_id: Some(*org),
            role: Some(Role::Member),
            scope: Some(scopes.join(" ")),
            aud: Some(aud.clone()),
            exp: Some(Utc::now().timestamp() + 3600),
            client_id: Some("test-client".into()),
            token_type: Some("Bearer".into()),
            token_kind: Some(TokenKind::Oauth),
            jti: Some(Uuid::new_v4()),
        },
        None => IntrospectionResponse::inactive(),
    }))
}

fn team_info(p: &Platform, team: Uuid) -> Option<TeamInfo> {
    p.teams.get(&team).map(|(org, slug, name)| TeamInfo {
        id: team,
        org_id: *org,
        slug: slug.clone(),
        name: name.clone(),
    })
}

async fn member(
    State(p): State<Shared>,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<MemberInfo>, StatusCode> {
    let p = p.lock().unwrap();
    if !p.members.contains_key(&(org, user)) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(MemberInfo {
        user: UserInfo {
            id: user,
            email: Some("dev@example.com".into()),
            name: None,
        },
        org: OrgInfo {
            id: org,
            slug: "acme".into(),
            name: "Acme".into(),
            plan: "team".into(),
        },
        role: Role::Member,
    }))
}

async fn member_teams(
    State(p): State<Shared>,
    Path((org, user)): Path<(Uuid, Uuid)>,
) -> Result<Json<MemberTeams>, StatusCode> {
    let p = p.lock().unwrap();
    if p.teams_down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let ids = p.members.get(&(org, user)).ok_or(StatusCode::NOT_FOUND)?;
    let mut teams: Vec<TeamInfo> = ids.iter().filter_map(|t| team_info(&p, *t)).collect();
    teams.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(MemberTeams { teams }))
}

async fn team(
    State(p): State<Shared>,
    Path((org, team)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamInfo>, StatusCode> {
    let p = p.lock().unwrap();
    if p.teams_down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    match team_info(&p, team) {
        Some(t) if t.org_id == org => Ok(Json(t)),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn usage_status(State(p): State<Shared>, Path(org): Path<Uuid>) -> Json<Value> {
    let blocked = p.lock().unwrap().blocked;
    Json(json!({
        "org_id": org,
        "plan": "team",
        "period_start": Utc::now().date_naive(),
        "billable_count": if blocked { 1000 } else { 0 },
        "total_count": 0,
        "included_ops": 1000,
        "hard_stop": blocked,
    }))
}

async fn ship(Json(b): Json<UsageBatch>) -> Json<UsageReceipt> {
    Json(UsageReceipt {
        accepted: b.events.len() as u32,
        duplicates: 0,
        rejected: vec![],
    })
}

async fn start_platform() -> (String, Shared) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/oauth/introspect", post(introspect))
        .route("/internal/orgs/{org}/members/{user}", get(member))
        .route(
            "/internal/orgs/{org}/members/{user}/teams",
            get(member_teams),
        )
        .route("/internal/orgs/{org}/teams/{team}", get(team))
        .route("/internal/orgs/{org}/usage-status", get(usage_status))
        .route("/internal/usage", post(ship))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), shared)
}

/// Mint a well-formed token the stub introspects as given, and make its user
/// a member of the org (on no team).
fn token(p: &Shared, org: Uuid, user: Uuid, scopes: &[&str], aud: &str) -> String {
    let t = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
    assert!(otto_resource::looks_like_token(&t), "{t}");
    let mut p = p.lock().unwrap();
    p.tokens.insert(
        t.clone(),
        (
            org,
            user,
            scopes.iter().map(|s| s.to_string()).collect(),
            aud.to_string(),
        ),
    );
    p.members.entry((org, user)).or_default();
    t
}

fn add_team(p: &Shared, org: Uuid, slug: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    p.lock()
        .unwrap()
        .teams
        .insert(id, (org, slug.to_string(), name.to_string()));
    id
}

// ------------------------------------------------------------------ harness

struct Harness {
    app: Router,
    stub: Shared,
    pool: PgPool,
}

async fn harness_with(pool: PgPool, webhook_secret: &str) -> Harness {
    harness_opts(pool, webhook_secret, false).await
}

/// [`harness_with`], with the meter enforcing quotas when `enforce` is set.
async fn harness_opts(pool: PgPool, webhook_secret: &str, enforce: bool) -> Harness {
    let (base, stub) = start_platform().await;
    let platform =
        Arc::new(PlatformClient::new(ClientConfig::new(base, RESOURCE, "otto_rs_test")).unwrap());
    let meter = Arc::new(Meter::new(
        platform.clone(),
        enforce,
        "https://otto.example.com",
    ));
    let state = AppState::new(
        Db::from_pool(pool.clone()),
        platform,
        meter,
        Config {
            public_url: PUBLIC_URL.into(),
            webhook_secret: webhook_secret.into(),
        },
    );
    Harness {
        app: router(state),
        stub,
        pool,
    }
}

async fn harness(pool: PgPool) -> Harness {
    harness_with(pool, SECRET).await
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Harness {
    /// A caller in a fresh org with the given scopes: (org, user, token).
    fn caller(&self, scopes: &[&str]) -> (Uuid, Uuid, String) {
        let (org, user) = (Uuid::new_v4(), Uuid::new_v4());
        (org, user, token(&self.stub, org, user, scopes, RESOURCE))
    }

    async fn send(&self, req: Request<Body>) -> Reply {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&bytes)))
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
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

    async fn webhook(&self, signature: Option<&str>, body: &[u8]) -> Reply {
        let mut req = Request::builder()
            .method("POST")
            .uri("/platform/webhooks")
            .header("content-type", "application/json");
        if let Some(s) = signature {
            req = req.header(otto_resource::webhook::SIGNATURE_HEADER, s);
        }
        self.send(req.body(Body::from(body.to_vec())).unwrap())
            .await
    }

    /// Every usage row recorded, as (org, user, tool). Read as the test's
    /// superuser, across orgs.
    async fn outbox(&self) -> Vec<(Uuid, Option<Uuid>, String)> {
        sqlx::query_as("SELECT org_id, user_id, tool FROM usage_outbox ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

fn signed(kind: &str, data: Value) -> (Vec<u8>, String) {
    let body = otto_resource::webhook::body(Uuid::new_v4(), kind, Utc::now(), &data);
    let sig = otto_resource::webhook::sign(SECRET, Utc::now().timestamp(), &body);
    (body, sig)
}

fn metadata_pointer() -> String {
    format!(r#"resource_metadata="{PUBLIC_URL}/.well-known/oauth-protected-resource""#)
}

// ---------------------------------------------------------------------- auth

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn no_token_is_401_with_the_discovery_challenge(pool: PgPool) {
    let h = harness(pool).await;
    for (method, uri) in [
        ("GET", "/api/v1/customers"),
        ("GET", "/api/v1/me/teams"),
        ("POST", "/api/v1/surveys"),
        (
            "DELETE",
            "/api/v1/experiences/00000000-0000-0000-0000-000000000000",
        ),
    ] {
        let r = h.call(method, uri, None, Some(json!({}))).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let challenge = r.headers["www-authenticate"].to_str().unwrap();
        assert!(
            challenge.starts_with("Bearer realm=\"otto-cxp\""),
            "{challenge}"
        );
        assert!(challenge.contains(&metadata_pointer()), "{challenge}");
        assert!(
            !challenge.contains("error="),
            "no credentials, no error: {challenge}"
        );
        assert_eq!(r.body["error"], "unauthenticated");
        assert!(r.body["error_description"].is_string());
    }
    // Another scheme is no token at all.
    let req = Request::builder()
        .uri("/api/v1/customers")
        .header("authorization", "Basic dXNlcjpwdw==")
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status, StatusCode::UNAUTHORIZED);
    assert!(h.outbox().await.is_empty());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_token_for_another_resource_or_unknown_to_the_platform_is_401(pool: PgPool) {
    let h = harness(pool).await;
    let elsewhere = token(&h.stub, Uuid::new_v4(), Uuid::new_v4(), RW, OTHER_RESOURCE);
    let unknown = format!("otto_at_{}", "b".repeat(43));
    for t in [elsewhere.as_str(), unknown.as_str()] {
        let r = h.call("GET", "/api/v1/customers", Some(t), None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
        let challenge = r.headers["www-authenticate"].to_str().unwrap();
        assert!(
            challenge.contains(r#"error="invalid_token""#),
            "{challenge}"
        );
        assert!(challenge.contains(&metadata_pointer()), "{challenge}");
        assert_eq!(r.body["error"], "invalid_token");
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_platform_outage_is_503_never_401(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, t) = h.caller(RW);
    h.stub.lock().unwrap().introspection_down = true;
    let r = h.call("GET", "/api/v1/customers", Some(&t), None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers["retry-after"], "5");
    assert!(r.headers.get("www-authenticate").is_none());
    assert_eq!(r.body["error"], "unavailable");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_read_only_token_reads_but_cannot_write(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, ro) = h.caller(RO);

    for uri in [
        "/api/v1/customers",
        "/api/v1/tickets",
        "/api/v1/feedback",
        "/api/v1/surveys",
        "/api/v1/experiences",
        "/api/v1/dashboard/metrics",
        "/api/v1/me/teams",
    ] {
        assert_eq!(
            h.call("GET", uri, Some(&ro), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/customers",
            json!({"email": "a@example.com"}),
        ),
        (
            "POST",
            "/api/v1/surveys",
            json!({"title": "t", "questions": []}),
        ),
        ("POST", "/api/v1/experiences", json!({"name": "web"})),
        (
            "DELETE",
            "/api/v1/customers/00000000-0000-0000-0000-000000000000",
            json!({}),
        ),
    ] {
        let r = h.call(method, uri, Some(&ro), Some(body)).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(r.body["code"], "insufficient_scope");
        assert_eq!(
            r.headers["www-authenticate"],
            r#"Bearer realm="otto-cxp", error="insufficient_scope", scope="cxp:write""#
        );
        assert!(r.body["error"].as_str().unwrap().contains("cxp:write"));
    }
    // A token with neither scope cannot read.
    let none = token(
        &h.stub,
        Uuid::new_v4(),
        Uuid::new_v4(),
        &["flags:read"],
        RESOURCE,
    );
    let r = h.call("GET", "/api/v1/customers", Some(&none), None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["code"], "insufficient_scope");
    assert_eq!(
        r.headers["www-authenticate"],
        r#"Bearer realm="otto-cxp", error="insufficient_scope", scope="cxp:read""#
    );

    assert!(
        h.outbox().await.is_empty(),
        "a refused survey charged nothing"
    );
}

// ------------------------------------------------------------------- shapes

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn routes_answer_in_the_shapes_the_console_reads(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, t) = h.caller(RW);

    let created = h
        .call(
            "POST",
            "/api/v1/customers",
            Some(&t),
            Some(json!({"email": "ada@example.com", "name": "Ada", "tags": ["vip"]})),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let id = created.body["id"].as_str().unwrap().to_string();
    assert_eq!(created.body["email"], "ada@example.com");
    assert!(created.body.get("createdAt").is_some(), "camelCase model");
    assert!(created.body.get("orgId").is_none() && created.body.get("org_id").is_none());

    let list = h.call("GET", "/api/v1/customers", Some(&t), None).await;
    assert_eq!(list.status, StatusCode::OK);
    let keys: Vec<&str> = list
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(keys.len(), 4, "{keys:?}");
    assert_eq!(list.body["total"], 1);
    assert_eq!(list.body["limit"], 50);
    assert_eq!(list.body["offset"], 0);
    assert_eq!(list.body["data"][0]["id"], id.as_str());

    // The page served is the clamped one.
    let list = h
        .call(
            "GET",
            "/api/v1/customers?limit=1000&offset=-3",
            Some(&t),
            None,
        )
        .await;
    assert_eq!(
        (list.body["limit"].clone(), list.body["offset"].clone()),
        (json!(200), json!(0))
    );

    let got = h
        .call("GET", &format!("/api/v1/customers/{id}"), Some(&t), None)
        .await;
    assert_eq!(got.status, StatusCode::OK);
    assert_eq!(got.body["id"], id.as_str());
    assert_eq!(got.body["tags"], json!(["vip"]));

    let view = h
        .call(
            "GET",
            &format!("/api/v1/customers/{id}/360"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(view.status, StatusCode::OK);
    assert_eq!(view.body["customer"]["id"], id.as_str());
    assert!(view.body["recentTickets"].is_array());

    let found = h
        .call("GET", "/api/v1/customers/search?q=ada", Some(&t), None)
        .await;
    assert_eq!(found.status, StatusCode::OK, "{}", found.body);
    // Search answers the list envelope plus `totalCapped` (#24); the #17
    // count headers are gone.
    assert_eq!(found.body["data"].as_array().unwrap().len(), 1);
    assert_eq!(found.body["total"], 1);
    assert_eq!(found.body["totalCapped"], false);
    assert_eq!(found.body["limit"], 50);
    assert_eq!(found.body["offset"], 0);
    assert!(found.headers.get("x-total-count").is_none());
    assert!(found.headers.get("x-total-capped").is_none());

    // The old snake_case body key still works (an alias) ...
    let ticket = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customer_id": id, "subject": "Broken"})),
        )
        .await;
    assert_eq!(ticket.status, StatusCode::OK, "{}", ticket.body);
    assert_eq!(ticket.body["priority"], "medium");
    assert_eq!(ticket.body["customerId"], id.as_str());
    // ... and so does the camelCase one, the convention.
    let ticket2 = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customerId": id, "subject": "Also broken", "priority": "low"})),
        )
        .await;
    assert_eq!(ticket2.status, StatusCode::OK, "{}", ticket2.body);
    assert_eq!(ticket2.body["customerId"], id.as_str());
    for key in ["customerId", "customer_id"] {
        let listed = h
            .call(
                "GET",
                &format!("/api/v1/tickets?{key}={id}"),
                Some(&t),
                None,
            )
            .await;
        assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
        assert_eq!(listed.body["total"], 2, "{key}");
    }
    let unrelated = h
        .call(
            "GET",
            &format!("/api/v1/tickets?customerId={}", Uuid::new_v4()),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(unrelated.body["total"], 0);
    let both = h
        .call(
            "POST",
            "/api/v1/tickets",
            Some(&t),
            Some(json!({"customerId": id, "customer_id": id, "subject": "x"})),
        )
        .await;
    assert_eq!(both.status, StatusCode::BAD_REQUEST);
    assert_eq!(both.body["code"], "invalid_argument");

    let metrics = h
        .call("GET", "/api/v1/dashboard/metrics", Some(&t), None)
        .await;
    // camelCase, plus the snake_case keys this route answered with before
    // #24, kept for existing consumers.
    assert_eq!(
        metrics.body,
        json!({
            "totalCustomers": 1, "totalTickets": 2, "openTickets": 2,
            "totalFeedback": 0, "totalSurveys": 0, "totalExperiences": 0,
            "total_customers": 1, "total_tickets": 2, "open_tickets": 2,
            "total_feedback": 0, "total_surveys": 0, "total_experiences": 0,
        })
    );

    // Errors are JSON with a code, and say nothing about the database.
    let bad_id = h
        .call("GET", "/api/v1/customers/not-a-uuid", Some(&t), None)
        .await;
    assert_eq!(bad_id.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_id.body["code"], "invalid_argument");
    let missing = h
        .call("POST", "/api/v1/customers", Some(&t), Some(json!({})))
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.body["code"], "invalid_argument");
    let dup = h
        .call(
            "POST",
            "/api/v1/customers",
            Some(&t),
            Some(json!({"email": "ada@example.com"})),
        )
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(dup.body["code"], "conflict");
    let text = dup.body.to_string();
    assert!(
        !text.contains("constraint") && !text.contains("duplicate key"),
        "{text}"
    );

    let gone = h
        .call("DELETE", &format!("/api/v1/customers/{id}"), Some(&t), None)
        .await;
    assert_eq!(gone.body, json!({"deleted": true}));
    let after = h
        .call("GET", &format!("/api/v1/customers/{id}"), Some(&t), None)
        .await;
    assert_eq!(after.status, StatusCode::NOT_FOUND);
    assert_eq!(after.body["code"], "not_found");

    // A JSON body must say it is JSON.
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/customers")
        .header("authorization", format!("Bearer {t}"))
        .header("content-type", "text/plain")
        .body(Body::from(r#"{"email": "t@example.com"}"#))
        .unwrap();
    let r = h.send(req).await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(r.body["code"], "unsupported_media_type");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn unknown_routes_and_methods_answer_json_and_only_after_auth(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, t) = h.caller(RW);

    // The REST teams routes are gone; unknown paths are a JSON 404.
    for uri in [
        "/api/v1/teams",
        "/api/v1/teams/00000000-0000-0000-0000-000000000000",
        "/api/v1/nope",
    ] {
        let r = h.call("GET", uri, Some(&t), None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(r.body["code"], "not_found", "{uri}");
        assert!(r.body["error"].is_string());
    }
    // A known path with a method it does not take is a JSON 405.
    for (method, uri) in [
        ("DELETE", "/api/v1/customers"),
        ("POST", "/api/v1/feedback"),
    ] {
        let r = h.call(method, uri, Some(&t), Some(json!({}))).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
        assert_eq!(r.body["code"], "method_not_allowed", "{method} {uri}");
    }
    // Without a token the gate answers first.
    for (method, uri) in [("GET", "/api/v1/nope"), ("DELETE", "/api/v1/customers")] {
        let r = h.call(method, uri, None, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert!(r.headers.contains_key("www-authenticate"));
    }
}

// ----------------------------------------------------------------- metering

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn only_creating_a_survey_charges_and_it_charges_once(pool: PgPool) {
    let h = harness(pool).await;
    let (org, user, t) = h.caller(RW);

    let c = h
        .call(
            "POST",
            "/api/v1/customers",
            Some(&t),
            Some(json!({"email": "x@example.com"})),
        )
        .await;
    assert_eq!(c.status, StatusCode::OK);
    let e = h
        .call(
            "POST",
            "/api/v1/experiences",
            Some(&t),
            Some(json!({"name": "web"})),
        )
        .await;
    assert_eq!(e.status, StatusCode::OK);
    assert!(
        h.outbox().await.is_empty(),
        "creating a customer or experience is free"
    );

    let s = h
        .call(
            "POST",
            "/api/v1/surveys",
            Some(&t),
            Some(json!({"title": "NPS", "questions": [{"q": "How likely?"}]})),
        )
        .await;
    assert_eq!(s.status, StatusCode::OK, "{}", s.body);
    let survey = s.body["id"].as_str().unwrap().to_string();
    assert_eq!(
        h.outbox().await,
        vec![(org, Some(user), "create_survey".to_string())]
    );

    // Every other survey operation is free.
    let u = h
        .call(
            "PUT",
            &format!("/api/v1/surveys/{survey}"),
            Some(&t),
            Some(json!({"active": false})),
        )
        .await;
    assert_eq!(u.status, StatusCode::OK);
    assert_eq!(u.body["active"], false);
    for uri in [
        "/api/v1/surveys".to_string(),
        format!("/api/v1/surveys/{survey}"),
        format!("/api/v1/surveys/{survey}/responses"),
    ] {
        assert_eq!(
            h.call("GET", &uri, Some(&t), None).await.status,
            StatusCode::OK,
            "{uri}"
        );
    }
    // A refused create rolls its charge back with it.
    let bad = h
        .call(
            "POST",
            "/api/v1/surveys",
            Some(&t),
            Some(json!({"title": "x", "questions": {}})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let del = h
        .call(
            "DELETE",
            &format!("/api/v1/surveys/{survey}"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(del.status, StatusCode::OK);

    assert_eq!(h.outbox().await.len(), 1);
}

// -------------------------------------------------------------------- teams

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn me_teams_lists_the_callers_platform_teams(pool: PgPool) {
    let h = harness(pool).await;
    let (org, user, t) = h.caller(RO);
    let ops = add_team(&h.stub, org, "ops", "Ops");
    let core = add_team(&h.stub, org, "core", "Core");
    add_team(&h.stub, org, "other", "Not mine");
    h.stub
        .lock()
        .unwrap()
        .members
        .insert((org, user), vec![ops, core]);

    let r = h.call("GET", "/api/v1/me/teams", Some(&t), None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.body,
        json!([
            {"id": core, "slug": "core", "name": "Core"},
            {"id": ops, "slug": "ops", "name": "Ops"},
        ])
    );

    // A member on no team, and a caller the platform no longer counts as a
    // member, both see an empty list.
    let (_, _, lonely) = h.caller(RO);
    let r = h.call("GET", "/api/v1/me/teams", Some(&lonely), None).await;
    assert_eq!(r.body, json!([]));
    let (gone_org, gone_user, gone) = h.caller(RO);
    h.stub
        .lock()
        .unwrap()
        .members
        .remove(&(gone_org, gone_user));
    let r = h.call("GET", "/api/v1/me/teams", Some(&gone), None).await;
    assert_eq!((r.status, r.body), (StatusCode::OK, json!([])));

    // The platform failing is 503.
    let (_, _, fresh) = h.caller(RO);
    h.stub.lock().unwrap().teams_down = true;
    let r = h.call("GET", "/api/v1/me/teams", Some(&fresh), None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.body["code"], "unavailable");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn catalog_team_ids_are_verified_and_the_field_keeps_clears_or_sets(pool: PgPool) {
    let h = harness(pool).await;
    let (org, _, t) = h.caller(RW);
    let ours = add_team(&h.stub, org, "web", "Web");
    let second = add_team(&h.stub, org, "app", "App");
    let theirs = add_team(&h.stub, Uuid::new_v4(), "else", "Elsewhere");

    // Unknown, and another org's, are both unknown_team; nothing is created.
    for team in [Uuid::new_v4(), theirs] {
        let r = h
            .call(
                "POST",
                "/api/v1/experiences",
                Some(&t),
                Some(json!({"name": "web", "team_id": team})),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.body["code"], "unknown_team");
    }
    let list = h.call("GET", "/api/v1/experiences", Some(&t), None).await;
    assert_eq!(list.body["total"], 0);

    // A known team is stored, under either spelling.
    let exp = h
        .call(
            "POST",
            "/api/v1/experiences",
            Some(&t),
            Some(json!({"name": "web", "team_id": ours})),
        )
        .await;
    assert_eq!(exp.status, StatusCode::OK, "{}", exp.body);
    assert_eq!(exp.body["teamId"], json!(ours));
    let exp_id = exp.body["id"].as_str().unwrap().to_string();
    let app = h
        .call(
            "POST",
            "/api/v1/applications",
            Some(&t),
            Some(json!({"name": "ios", "experience_id": exp_id, "teamId": ours})),
        )
        .await;
    assert_eq!(app.status, StatusCode::OK, "{}", app.body);
    assert_eq!(app.body["teamId"], json!(ours));
    assert_eq!(app.body["experienceId"], exp_id.as_str());

    let uri = format!("/api/v1/experiences/{exp_id}");
    // Absent keeps the team.
    let r = h
        .call("PUT", &uri, Some(&t), Some(json!({"name": "web2"})))
        .await;
    assert_eq!(
        (r.body["name"].clone(), r.body["teamId"].clone()),
        (json!("web2"), json!(ours))
    );
    // An id sets it (camelCase), an unknown one is refused and changes nothing.
    let r = h
        .call("PUT", &uri, Some(&t), Some(json!({"teamId": second})))
        .await;
    assert_eq!(r.body["teamId"], json!(second));
    let r = h
        .call(
            "PUT",
            &uri,
            Some(&t),
            Some(json!({"team_id": Uuid::new_v4(), "name": "x"})),
        )
        .await;
    assert_eq!(r.body["code"], "unknown_team");
    let r = h.call("GET", &uri, Some(&t), None).await;
    assert_eq!(
        (r.body["name"].clone(), r.body["teamId"].clone()),
        (json!("web2"), json!(second))
    );
    // An explicit null clears it.
    let r = h
        .call("PUT", &uri, Some(&t), Some(json!({"team_id": null})))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["teamId"], Value::Null);
    // Both spellings at once is ambiguous.
    let r = h
        .call(
            "PUT",
            &uri,
            Some(&t),
            Some(json!({"team_id": ours, "teamId": null})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Pages and components verify too.
    let page = h
        .call(
            "POST",
            "/api/v1/pages",
            Some(&t),
            Some(json!({"name": "home", "team_id": theirs})),
        )
        .await;
    assert_eq!(page.body["code"], "unknown_team");
    let comp = h
        .call(
            "POST",
            "/api/v1/components",
            Some(&t),
            Some(json!({"name": "button", "team_id": ours})),
        )
        .await;
    assert_eq!(comp.body["teamId"], json!(ours));

    // The platform failing is 503, and nothing is written.
    h.stub.lock().unwrap().teams_down = true;
    let r = h
        .call(
            "POST",
            "/api/v1/pages",
            Some(&t),
            Some(json!({"name": "p", "team_id": ours})),
        )
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers["retry-after"], "5");
    // No team needs no platform.
    let r = h
        .call(
            "POST",
            "/api/v1/pages",
            Some(&t),
            Some(json!({"name": "p"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let pages = h.call("GET", "/api/v1/pages", Some(&t), None).await;
    assert_eq!(pages.body["total"], 1);
}

// ---------------------------------------------------------------- isolation

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn another_orgs_customer_is_not_found(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, a) = h.caller(RW);
    let (_, _, b) = h.caller(RW);
    let c = h
        .call(
            "POST",
            "/api/v1/customers",
            Some(&a),
            Some(json!({"email": "a@example.com"})),
        )
        .await;
    let uri = format!("/api/v1/customers/{}", c.body["id"].as_str().unwrap());

    for (method, body) in [
        ("GET", None),
        ("PUT", Some(json!({"name": "stolen"}))),
        ("DELETE", None),
    ] {
        let r = h.call(method, &uri, Some(&b), body).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(r.body["code"], "not_found");
    }
    assert_eq!(
        h.call("GET", "/api/v1/customers", Some(&b), None)
            .await
            .body["total"],
        0
    );
    let still = h.call("GET", &uri, Some(&a), None).await;
    assert_eq!(still.status, StatusCode::OK);
    assert_eq!(still.body["name"], Value::Null);
}

// ----------------------------------------------------------------- webhooks

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn webhooks_are_verified_applied_once_and_null_a_deleted_teams_catalog(pool: PgPool) {
    let h = harness(pool).await;
    let (org, _, t) = h.caller(RW);
    let team = add_team(&h.stub, org, "web", "Web");
    let exp = h
        .call(
            "POST",
            "/api/v1/experiences",
            Some(&t),
            Some(json!({"name": "web", "team_id": team})),
        )
        .await;
    let uri = format!("/api/v1/experiences/{}", exp.body["id"].as_str().unwrap());

    let (body, sig) = signed("team.deleted", json!({"org_id": org, "team_id": team}));

    // Missing, malformed, and wrong signatures are refused before parsing.
    let wrong = otto_resource::webhook::sign("otto_whsec_wrong", Utc::now().timestamp(), &body);
    let stale = otto_resource::webhook::sign(SECRET, Utc::now().timestamp() - 3600, &body);
    for s in [
        None,
        Some("t=1,v1=zz"),
        Some(wrong.as_str()),
        Some(stale.as_str()),
    ] {
        let r = h.webhook(s, &body).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{s:?}");
        assert_eq!(r.body["code"], "invalid_signature");
    }
    // A signature over different bytes does not cover these.
    let tampered = String::from_utf8(body.clone())
        .unwrap()
        .replace("team.deleted", "org.deleted");
    assert_eq!(
        h.webhook(Some(&sig), tampered.as_bytes()).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = h.call("GET", &uri, Some(&t), None).await;
    assert_eq!(r.body["teamId"], json!(team), "nothing applied yet");

    let r = h.webhook(Some(&sig), &body).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["result"], "applied");
    let r = h.call("GET", &uri, Some(&t), None).await;
    assert_eq!(
        r.body["teamId"],
        Value::Null,
        "the team's catalog is unowned"
    );

    // A redelivery (signed afresh, same event id) is a duplicate.
    let resigned = otto_resource::webhook::sign(SECRET, Utc::now().timestamp(), &body);
    let r = h.webhook(Some(&resigned), &body).await;
    assert_eq!(
        (r.status, r.body["result"].clone()),
        (StatusCode::OK, json!("duplicate"))
    );

    // Unknown kinds are acknowledged, not failed.
    let (body, sig) = signed("org.renamed", json!({"org_id": org}));
    let r = h.webhook(Some(&sig), &body).await;
    assert_eq!(
        (r.status, r.body["result"].clone()),
        (StatusCode::OK, json!("ignored"))
    );

    // Signed but unreadable is 400.
    let junk = br#"{"not": "an event"}"#;
    let sig = otto_resource::webhook::sign(SECRET, Utc::now().timestamp(), junk);
    let r = h.webhook(Some(&sig), junk).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // member.removed takes effect on the next request.
    let (o2, u2, t2) = h.caller(RW);
    assert_eq!(
        h.call("GET", "/api/v1/customers", Some(&t2), None)
            .await
            .status,
        StatusCode::OK
    );
    let (body, sig) = signed("member.removed", json!({"org_id": o2, "user_id": u2}));
    assert_eq!(h.webhook(Some(&sig), &body).await.body["result"], "applied");
    let r = h.call("GET", "/api/v1/customers", Some(&t2), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.headers.contains_key("www-authenticate"));
}

/// A delivery the database cannot record is `503`, so the platform redelivers
/// it; nothing about the failure leaks into the body.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_webhook_that_cannot_be_applied_is_503(pool: PgPool) {
    let h = harness(pool.clone()).await;
    // Deterministic failure: the dedupe log every event writes is gone.
    sqlx::query("DROP TABLE platform_events")
        .execute(&pool)
        .await
        .unwrap();
    let org = Uuid::new_v4();
    for (kind, data) in [
        (
            "team.deleted",
            json!({"org_id": org, "team_id": Uuid::new_v4()}),
        ),
        (
            "member.removed",
            json!({"org_id": org, "user_id": Uuid::new_v4()}),
        ),
    ] {
        let (body, sig) = signed(kind, data);
        let r = h.webhook(Some(&sig), &body).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{kind}");
        assert_eq!(r.headers["retry-after"], "5");
        assert_eq!(r.body["code"], "unavailable");
        assert!(
            !r.body.to_string().contains("platform_events"),
            "{}",
            r.body
        );
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn without_a_webhook_secret_every_delivery_is_refused(pool: PgPool) {
    let h = harness_with(pool, "").await;
    let (body, _) = signed("org.deleted", json!({"org_id": Uuid::new_v4()}));
    let sig = otto_resource::webhook::sign("", Utc::now().timestamp(), &body);
    let r = h.webhook(Some(&sig), &body).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["code"], "invalid_signature");
}

// --------------------------------------------------------------------- body

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn bodies_over_256_kib_are_413(pool: PgPool) {
    let h = harness(pool).await;
    let (_, _, t) = h.caller(RW);
    let big = json!({"email": "big@example.com", "metadata": {"blob": "x".repeat(300 * 1024)}});
    let r = h
        .call("POST", "/api/v1/customers", Some(&t), Some(big))
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.body["code"], "payload_too_large");

    // Just under the cap is fine.
    let ok = json!({"email": "ok@example.com", "metadata": {"blob": "x".repeat(200 * 1024)}});
    assert_eq!(
        h.call("POST", "/api/v1/customers", Some(&t), Some(ok))
            .await
            .status,
        StatusCode::OK
    );

    let (body, sig) = signed(
        "org.deleted",
        json!({"org_id": Uuid::new_v4(), "pad": "x".repeat(300 * 1024)}),
    );
    assert_eq!(
        h.webhook(Some(&sig), &body).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

/// The console's billable route is refused like the billable tool once the
/// org's allowance is used up (enforcing meter): 429 `quota_exceeded`,
/// nothing created, nothing recorded. Free routes still work.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_org_over_its_allowance_cannot_create_a_survey_over_rest(pool: PgPool) {
    let h = harness_opts(pool, SECRET, true).await;
    h.stub.lock().unwrap().blocked = true;
    let (org, _, t) = h.caller(RW);

    let r = h
        .call(
            "POST",
            "/api/v1/surveys",
            Some(&t),
            Some(json!({"title": "NPS", "questions": []})),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
    assert_eq!(r.body["code"], "quota_exceeded");
    let surveys: i64 = sqlx::query_scalar("SELECT count(*) FROM surveys WHERE org_id = $1")
        .bind(org)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(surveys, 0, "nothing created");
    assert!(h.outbox().await.is_empty(), "nothing recorded");

    let r = h
        .call(
            "POST",
            "/api/v1/customers",
            Some(&t),
            Some(json!({"email": "still@example.com"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(h.outbox().await.is_empty());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn survey_responses_answer_the_list_envelope(pool: PgPool) {
    use cxp_core::feedback::FeedbackExt;

    let h = harness(pool).await;
    let (org, _, t) = h.caller(RW);

    let mut ids = Vec::new();
    for title in ["NPS", "Empty"] {
        let s = h
            .call(
                "POST",
                "/api/v1/surveys",
                Some(&t),
                Some(json!({"title": title, "questions": [{"q": "How likely?"}]})),
            )
            .await;
        assert_eq!(s.status, StatusCode::OK, "{}", s.body);
        ids.push(s.body["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }
    let (survey, empty) = (ids[0], ids[1]);

    let mut customers = Vec::new();
    for email in ["a@example.com", "b@example.com"] {
        let c = h
            .call(
                "POST",
                "/api/v1/customers",
                Some(&t),
                Some(json!({"email": email})),
            )
            .await;
        assert_eq!(c.status, StatusCode::OK, "{}", c.body);
        customers.push(c.body["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }

    let db = Db::from_pool(h.pool.clone());
    let mut tx = db.begin(otto_tenant::ids::OrgId::from(org)).await.unwrap();
    for customer in [customers[0], customers[1], customers[0]] {
        tx.submit_survey_response(survey, customer, &json!({"q1": 9}))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    let r = h
        .call(
            "GET",
            &format!("/api/v1/surveys/{survey}/responses"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let mut keys: Vec<&str> = r
        .body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["data", "limit", "offset", "total"]);
    assert_eq!(r.body["total"], 3);
    assert_eq!(r.body["limit"], 50);
    assert_eq!(r.body["offset"], 0);
    let rows = r.body["data"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["surveyId"], survey.to_string());
    assert!(rows[0]["customerId"].is_string());
    assert!(!rows[0]["completedAt"].is_null());
    assert_eq!(rows[0]["answers"], json!({"q1": 9}));
    assert!(rows[0].get("survey_id").is_none());

    let r = h
        .call(
            "GET",
            &format!("/api/v1/surveys/{survey}/responses?limit=2&offset=0"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["data"].as_array().unwrap().len(), 2);
    assert_eq!(r.body["total"], 3);
    assert_eq!(r.body["limit"], 2);

    let r = h
        .call(
            "GET",
            &format!("/api/v1/surveys/{survey}/responses?offset=10"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["data"], json!([]));
    assert_eq!(r.body["total"], 3);
    assert_eq!(r.body["offset"], 10);

    let r = h
        .call(
            "GET",
            &format!("/api/v1/surveys/{empty}/responses"),
            Some(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body,
        json!({"data": [], "total": 0, "limit": 50, "offset": 0})
    );

    let (_, _, other) = h.caller(RO);
    let r = h
        .call(
            "GET",
            &format!("/api/v1/surveys/{survey}/responses"),
            Some(&other),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.body);
    assert_eq!(r.body["code"], "not_found");
}
