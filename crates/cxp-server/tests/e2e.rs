//! The whole server, end to end: the router `cxp-server` serves, against a
//! real Postgres and a mock otto platform (token introspection with an
//! audience, members, member teams, teams, usage status, usage shipping),
//! driven over HTTP the way an agent (`/mcp`), the console (`/api/v1`), and
//! the platform (`/platform/webhooks`) drive it.
//!
//! One test per success criterion of epic #1:
//!
//! 1. unauthenticated or wrong-audience requests are 401 with the discovery
//!    pointer; discovery and health are open;
//! 2. org B cannot read or write org A's rows through any tool or route, and
//!    the boot-time isolation proof passes;
//! 3. `whoami`/`usage` work, exactly the four billable tools (and
//!    `POST /api/v1/surveys`) ship one usage event each, and a replayed
//!    shipment is accepted;
//! 4. lifecycle webhooks are verified and applied once.
//!
//! Needs `DATABASE_URL` (see `compose.yaml`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use chrono::Utc;
use cxp_core::knowledge::{ArticleUpdate, KnowledgeExt, NewArticle};
use cxp_core::platform_events::PURGED_TABLES;
use cxp_core::usage::{ShipperConfig, BILLABLE};
use cxp_server::{meter, platform_client, router, Config};
use http::{HeaderMap, Method, Request, StatusCode};
use otto_resource::{
    IntrospectionResponse, MemberInfo, MemberTeams, OrgInfo, PlatformClient, Role, TeamInfo,
    TokenKind, UsageBatch, UsageEvent, UsageReceipt, UserInfo,
};
use otto_tenant::Db;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PUBLIC_URL: &str = "https://cxp.example.com";
const HOST: &str = "cxp.example.com";
const RESOURCE: &str = "https://cxp.example.com/mcp";
/// The audience of a token the platform minted for otto-flags.
const FLAGS_RESOURCE: &str = "https://otto-flags.savvagent.com/mcp";
const WEBHOOK_SECRET: &str = "otto_whsec_test";
const RW: &[&str] = &["cxp:read", "cxp:write"];

// ------------------------------------------------------------- mock platform

#[derive(Default)]
struct Platform {
    /// token -> (org, user, scopes, audience)
    tokens: HashMap<String, (Uuid, Uuid, Vec<String>, String)>,
    /// team -> (org, slug, name)
    teams: HashMap<Uuid, (Uuid, String, String)>,
    /// (org, user) -> the user's teams; absent means "not a member".
    members: HashMap<(Uuid, Uuid), Vec<Uuid>>,
    /// Every usage event received, in order, duplicates included.
    received: Vec<UsageEvent>,
    /// The event ids counted once, as the platform dedupes them.
    counted: HashSet<Uuid>,
}

type Shared = Arc<Mutex<Platform>>;

async fn introspect(
    State(p): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Json<IntrospectionResponse> {
    let p = p.lock().unwrap();
    Json(match p.tokens.get(&f["token"]) {
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
    })
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
            email: Some("agent@example.com".into()),
            name: Some("Agent".into()),
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
    let ids = p.members.get(&(org, user)).ok_or(StatusCode::NOT_FOUND)?;
    let teams = ids.iter().filter_map(|t| team_info(&p, *t)).collect();
    Ok(Json(MemberTeams { teams }))
}

async fn team(
    State(p): State<Shared>,
    Path((org, team)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamInfo>, StatusCode> {
    let p = p.lock().unwrap();
    match team_info(&p, team) {
        Some(t) if t.org_id == org => Ok(Json(t)),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn usage_status(Path(org): Path<Uuid>) -> Json<Value> {
    Json(json!({
        "org_id": org,
        "plan": "team",
        "period_start": Utc::now().date_naive(),
        "billable_count": 3,
        "total_count": 3,
        "included_ops": 1000,
        "hard_stop": false,
    }))
}

/// Records every event; an event id seen before is a duplicate, as the
/// platform reports it.
async fn ship(State(p): State<Shared>, Json(b): Json<UsageBatch>) -> Json<UsageReceipt> {
    let mut p = p.lock().unwrap();
    let (mut accepted, mut duplicates) = (0, 0);
    for e in b.events {
        if p.counted.insert(e.event_id) {
            accepted += 1;
        } else {
            duplicates += 1;
        }
        p.received.push(e);
    }
    Json(UsageReceipt {
        accepted,
        duplicates,
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

// --------------------------------------------------------------- the server

struct Server {
    app: Router,
    db: Db,
    pool: PgPool,
    platform: Arc<PlatformClient>,
    mock: Shared,
}

/// The router exactly as `main` builds it, from `Config` down.
async fn server(pool: PgPool) -> Server {
    let (base, mock) = start_platform().await;
    let mut config = Config::for_test();
    config.platform_url = base;
    assert_eq!(config.public_url, PUBLIC_URL);
    assert_eq!(config.resource_uri, RESOURCE);
    assert_eq!(config.platform_webhook_secret, WEBHOOK_SECRET);
    let db = Db::from_pool(pool.clone());
    let platform = platform_client(&config).unwrap();
    let app = router(
        db.clone(),
        platform.clone(),
        meter(platform.clone(), &config),
        &config,
        cxp_server::PlatformCheck::Accepted,
    )
    .unwrap();
    Server {
        app,
        db,
        pool,
        platform,
        mock,
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// A caller: a platform member with a token for this service.
#[derive(Clone)]
struct Caller {
    org: Uuid,
    user: Uuid,
    token: String,
}

impl Server {
    /// Mint a token the mock introspects as given, for a member of `org` on
    /// no team.
    fn token_for(&self, org: Uuid, user: Uuid, scopes: &[&str], aud: &str) -> String {
        let t = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
        assert!(otto_resource::looks_like_token(&t), "{t}");
        let mut p = self.mock.lock().unwrap();
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

    fn caller_in(&self, org: Uuid) -> Caller {
        let user = Uuid::new_v4();
        Caller {
            org,
            user,
            token: self.token_for(org, user, RW, RESOURCE),
        }
    }

    fn caller(&self) -> Caller {
        self.caller_in(Uuid::new_v4())
    }

    fn add_team(&self, org: Uuid, slug: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.mock
            .lock()
            .unwrap()
            .teams
            .insert(id, (org, slug.to_string(), slug.to_uppercase()));
        id
    }

    async fn send(&self, req: Request<Body>) -> Reply {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 22)
            .await
            .unwrap();
        Reply {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    fn mcp_request(token: Option<&str>, body: &Value) -> Request<Body> {
        let mut req = Request::post("/mcp")
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        req.body(Body::from(body.to_string())).unwrap()
    }

    /// One `tools/call`: `Ok(structuredContent)` or `Err(error)`.
    async fn tool(&self, token: &str, name: &str, args: Value) -> Result<Value, Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": name, "arguments": args}});
        let r = self.send(Self::mcp_request(Some(token), &body)).await;
        assert!(
            r.status.is_success(),
            "{name}: HTTP {} {}",
            r.status,
            r.body
        );
        if let Some(e) = r.body.get("error") {
            return Err(e.clone());
        }
        let result = &r.body["result"];
        if result["isError"] == true {
            return Err(result.clone());
        }
        Ok(result["structuredContent"].clone())
    }

    /// One `resources/read`: `Ok(the JSON its text holds)` or `Err(error)`.
    async fn resource(&self, token: &str, uri: &str) -> Result<Value, Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read",
                          "params": {"uri": uri}});
        let r = self.send(Self::mcp_request(Some(token), &body)).await;
        assert!(r.status.is_success(), "{uri}: HTTP {} {}", r.status, r.body);
        if let Some(e) = r.body.get("error") {
            return Err(e.clone());
        }
        let text = r.body["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{uri}: no text in {}", r.body));
        Ok(serde_json::from_str(text).unwrap())
    }

    /// [`Self::tool`] that must succeed.
    async fn ok(&self, token: &str, name: &str, args: Value) -> Value {
        match self.tool(token, name, args.clone()).await {
            Ok(v) => v,
            Err(e) => panic!("{name}({args}) failed: {e}"),
        }
    }

    async fn api(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", HOST);
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

    /// An `/api/v1` call that must answer 200.
    async fn api_ok(&self, method: &str, uri: &str, token: &str, body: Option<Value>) -> Value {
        let r = self.api(method, uri, Some(token), body).await;
        assert_eq!(r.status, StatusCode::OK, "{method} {uri}: {}", r.body);
        r.body
    }

    async fn webhook(&self, body: &[u8], signature: &str) -> Reply {
        self.send(
            Request::post("/platform/webhooks")
                .header("content-type", "application/json")
                .header(otto_resource::webhook::SIGNATURE_HEADER, signature)
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
    }

    /// Run the usage shipper once, as the background task does.
    async fn ship(&self) -> usize {
        cxp_core::usage::ship_once(&self.db, &self.platform, &ShipperConfig::default())
            .await
            .expect("usage ships")
    }

    /// The tools of every usage event the mock has received.
    fn shipped(&self) -> Vec<String> {
        self.mock
            .lock()
            .unwrap()
            .received
            .iter()
            .map(|e| e.tool.clone())
            .collect()
    }

    /// Every row each tenant table holds for `org`, read as the test's
    /// superuser (past RLS), as one JSON value per table.
    async fn rows_of(&self, org: Uuid) -> Vec<(&'static str, Value)> {
        let mut out = vec![];
        for table in PURGED_TABLES {
            let rows: Value = sqlx::query_scalar(&format!(
                "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) \
                 FROM {table} t WHERE org_id = $1"
            ))
            .bind(org)
            .fetch_one(&self.pool)
            .await
            .unwrap();
            out.push((*table, rows));
        }
        out
    }
}

fn code(e: &Value) -> &str {
    e["data"]["code"].as_str().unwrap_or_default()
}

fn signed(event: Uuid, kind: &str, data: Value) -> (Vec<u8>, String) {
    let body = otto_resource::webhook::body(event, kind, Utc::now(), &data);
    let sig = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), &body);
    (body, sig)
}

fn metadata_pointer() -> String {
    format!(r#"resource_metadata="{PUBLIC_URL}/.well-known/oauth-protected-resource""#)
}

// ============================================================ criterion 1

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn criterion_1_unauthenticated_and_foreign_tokens_are_401_and_discovery_is_open(
    pool: PgPool,
) {
    let s = server(pool).await;

    // No token at all: 401 pointing at the metadata document, on both
    // surfaces.
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let mcp = s.send(Server::mcp_request(None, &list)).await;
    let api = s.api("GET", "/api/v1/customers", None, None).await;
    for (surface, r) in [("/mcp", &mcp), ("/api/v1/customers", &api)] {
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{surface}");
        let challenge = r.headers["www-authenticate"].to_str().unwrap();
        assert!(challenge.starts_with("Bearer "), "{surface}: {challenge}");
        assert!(
            challenge.contains(&metadata_pointer()),
            "{surface}: {challenge}"
        );
    }

    // A token the platform minted for otto-flags is active, carries the cxp
    // scopes, and is still refused: its audience is another service.
    let org = Uuid::new_v4();
    let foreign = s.token_for(org, Uuid::new_v4(), RW, FLAGS_RESOURCE);
    let whoami = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": {"name": "whoami", "arguments": {}}});
    let mcp = s.send(Server::mcp_request(Some(&foreign), &whoami)).await;
    let api = s
        .api("GET", "/api/v1/customers", Some(&foreign), None)
        .await;
    for (surface, r) in [("/mcp", &mcp), ("/api/v1/customers", &api)] {
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{surface}");
        let challenge = r.headers["www-authenticate"].to_str().unwrap();
        assert!(
            challenge.contains(r#"error="invalid_token""#),
            "{surface}: {challenge}"
        );
        assert!(
            challenge.contains(&metadata_pointer()),
            "{surface}: {challenge}"
        );
    }
    // The same grant for this service's audience is accepted, so the
    // audience alone is what refused it.
    let native = s.token_for(org, Uuid::new_v4(), RW, RESOURCE);
    let r = s.send(Server::mcp_request(Some(&native), &whoami)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.body["result"]["structuredContent"]["org"]["id"],
        org.to_string()
    );
    let r = s.api("GET", "/api/v1/customers", Some(&native), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    // A well-formed token the platform does not know is inactive: 401.
    let unknown = format!("otto_at_{}{}", "z".repeat(11), Uuid::new_v4().simple());
    let mcp = s.send(Server::mcp_request(Some(&unknown), &whoami)).await;
    let api = s
        .api("GET", "/api/v1/customers", Some(&unknown), None)
        .await;
    for (surface, r) in [("/mcp", &mcp), ("/api/v1/customers", &api)] {
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{surface}");
        let challenge = r.headers["www-authenticate"].to_str().unwrap();
        assert!(
            challenge.contains(&metadata_pointer()),
            "{surface}: {challenge}"
        );
    }

    // A read-only token is authenticated but may not write: 403 on the REST
    // API (naming the scope), insufficient_scope from the tool.
    let reader = s.token_for(org, Uuid::new_v4(), &["cxp:read"], RESOURCE);
    let r = s
        .api(
            "POST",
            "/api/v1/customers",
            Some(&reader),
            Some(json!({"email": "r@example.com"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
    assert_eq!(r.body["code"], "insufficient_scope");
    let challenge = r.headers["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains(r#"scope="cxp:write""#), "{challenge}");
    let refused = s
        .tool(
            &reader,
            "create_customer",
            json!({"email": "r@example.com"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "insufficient_scope", "{refused}");
    assert_eq!(
        s.api_ok("GET", "/api/v1/customers", &reader, None).await["total"],
        0,
        "nothing was written"
    );

    // Discovery and health need no token.
    let doc = s
        .api("GET", "/.well-known/oauth-protected-resource", None, None)
        .await;
    assert_eq!(doc.status, StatusCode::OK);
    assert_eq!(doc.body["resource"], RESOURCE);
    assert_eq!(
        doc.body["scopes_supported"],
        json!(["cxp:read", "cxp:write"])
    );
    let healthz = s.api("GET", "/healthz", None, None).await;
    assert_eq!(
        (healthz.status, healthz.body),
        (StatusCode::OK, json!({"status": "ok"}))
    );
    let readyz = s.api("GET", "/readyz", None, None).await;
    assert_eq!(
        (readyz.status, readyz.body),
        (StatusCode::OK, json!({"status": "ready"}))
    );

    // Anything else is a JSON 404, not an empty body.
    let nope = s.api("GET", "/nope", None, None).await;
    assert_eq!(nope.status, StatusCode::NOT_FOUND);
    assert_eq!(nope.body["code"], "not_found");
}

/// rmcp reads `/mcp`'s body itself, past axum's extractor limit, so only the
/// router-wide cap stops an oversized one: refused before anything reads it,
/// with or without a `Content-Length`, and from a caller with a valid token.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_oversized_body_to_mcp_is_413(pool: PgPool) {
    let s = server(pool).await;
    let a = s.caller();
    // A well-formed JSON-RPC call padded to 1 MiB.
    let pad = "x".repeat(1 << 20);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "create_customer",
                                 "arguments": {"email": "big@example.com", "notes": pad}}})
    .to_string();
    assert!(body.len() > cxp_api::MAX_BODY_BYTES);

    let declared = Request::post("/mcp")
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {}", a.token))
        .header("content-length", body.len())
        .body(Body::from(body.clone()))
        .unwrap();
    let r = s.send(declared).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
    assert_eq!(r.body["code"], "payload_too_large", "{}", r.body);

    // No Content-Length: the body is cut off as it is read.
    let undeclared = Server::mcp_request(Some(&a.token), &serde_json::from_str(&body).unwrap());
    assert!(undeclared.headers().get("content-length").is_none());
    let r = s.send(undeclared).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);

    // Nothing was written.
    let rows: HashMap<&str, Value> = s.rows_of(a.org).await.into_iter().collect();
    assert_eq!(rows["customers"], json!([]));
}

// ============================================================ criterion 2

/// Text every row org A writes carries, so a leak shows up as this string in
/// org B's view.
const MARK: &str = "zq-alpha";

/// One row of every kind, in org A.
struct Seeded {
    team: Uuid,
    customer: String,
    email: String,
    segment: String,
    ticket: String,
    session: String,
    survey: String,
    feedback: String,
    article: String,
    experience: String,
    application: String,
    page: String,
    component: String,
}

impl Seeded {
    fn ids(&self) -> Vec<&str> {
        vec![
            &self.customer,
            &self.segment,
            &self.ticket,
            &self.session,
            &self.survey,
            &self.feedback,
            &self.article,
            &self.experience,
            &self.application,
            &self.page,
            &self.component,
        ]
    }
}

fn id(v: &Value) -> String {
    v["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {v}"))
        .to_string()
}

/// Org A writes one row of every kind through the routes and tools (the
/// knowledge base and static segment members, which nothing serves a write
/// for, through cxp-core and SQL in A's org).
async fn seed(s: &Server, a: &Caller) -> Seeded {
    let t = &a.token;
    let team = s.add_team(a.org, "alpha-web");
    s.mock
        .lock()
        .unwrap()
        .members
        .insert((a.org, a.user), vec![team]);

    let experience = id(&s
        .api_ok(
            "POST",
            "/api/v1/experiences",
            t,
            Some(json!({"name": format!("{MARK} web"), "team_id": team})),
        )
        .await);
    let application = id(&s
        .api_ok(
            "POST",
            "/api/v1/applications",
            t,
            Some(json!({"name": format!("{MARK} app"), "experience_id": experience, "team_id": team})),
        )
        .await);
    let page = id(&s
        .api_ok(
            "POST",
            "/api/v1/pages",
            t,
            Some(json!({"name": format!("{MARK} page"), "application_id": application, "team_id": team})),
        )
        .await);
    let component = id(&s
        .api_ok(
            "POST",
            "/api/v1/components",
            t,
            Some(json!({"name": format!("{MARK} button"), "page_id": page, "team_id": team})),
        )
        .await);

    let email = format!("{MARK}@example.com");
    let customer = id(&s
        .ok(
            t,
            "create_customer",
            json!({"email": email, "name": format!("{MARK} Customer"), "company": format!("{MARK} Co")}),
        )
        .await);
    s.ok(
        t,
        "record_interaction",
        json!({"customer_id": customer, "interaction_type": "login",
               "metadata": format!(r#"{{"note": "{MARK}"}}"#)}),
    )
    .await;
    let segment = id(&s
        .ok(
            t,
            "create_segment",
            json!({"name": format!("{MARK} static"), "criteria": "{}", "segment_type": "static"}),
        )
        .await);
    sqlx::query(
        "INSERT INTO segment_members (org_id, segment_id, customer_id) VALUES ($1, $2, $3)",
    )
    .bind(a.org)
    .bind(Uuid::parse_str(&segment).unwrap())
    .bind(Uuid::parse_str(&customer).unwrap())
    .execute(&s.pool)
    .await
    .unwrap();
    let ticket = id(&s
        .ok(
            t,
            "create_ticket",
            json!({"customer_id": customer, "subject": format!("{MARK} ticket")}),
        )
        .await);
    let session = id(&s.ok(t, "start_chat_session", json!({"id": customer})).await);
    s.ok(
        t,
        "send_chat_message",
        json!({"session_id": session, "role": "customer", "content": format!("{MARK} hello")}),
    )
    .await;
    let survey = id(&s
        .ok(
            t,
            "create_survey",
            json!({"title": format!("{MARK} survey"),
                   "questions": r#"[{"id": "q1", "type": "nps"}]"#}),
        )
        .await);
    s.ok(
        t,
        "send_survey",
        json!({"survey_id": survey, "customer_id": customer}),
    )
    .await;
    s.ok(
        t,
        "submit_survey_response",
        json!({"survey_id": survey, "customer_id": customer, "answers": r#"{"score": 9}"#}),
    )
    .await;
    let feedback = id(&s
        .ok(
            t,
            "submit_feedback",
            json!({"customer_id": customer, "feedback_type": "praise", "rating": 5,
                   "subject": format!("{MARK} feedback"),
                   "experience_id": experience, "application_id": application,
                   "page_id": page, "component_id": component}),
        )
        .await);

    let mut tx = s.db.begin(a.org.into()).await.unwrap();
    let article = tx
        .create_article(&NewArticle {
            title: format!("{MARK} article"),
            content: format!("{MARK} answer"),
            category: None,
            tags: vec![],
        })
        .await
        .unwrap();
    tx.update_article(
        article.id,
        &ArticleUpdate {
            published: Some(true),
            ..ArticleUpdate::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Visible to A's own search: the article is really there to be leaked.
    let found = s
        .ok(t, "search_knowledge_base", json!({"query": MARK}))
        .await;
    assert_eq!(found["items"][0]["id"], article.id.to_string(), "{found}");

    // A web console session (no route creates one without the platform's
    // sign-in, so it is stored directly, as the callback stores it).
    cxp_core::console_sessions::create(
        &s.db,
        &otto_tenant::crypto::Cipher::from_base64_key(
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        )
        .unwrap(),
        &cxp_core::console_sessions::NewSession {
            id_hash: cxp_core::console_sessions::hash_cookie(t),
            org_id: a.org.into(),
            user_id: a.user.into(),
            access_token: t,
            refresh_token: "otto_rt_e2e",
            access_expires_at: Utc::now() + chrono::Duration::hours(1),
            expires_at: Utc::now() + chrono::Duration::days(1),
        },
    )
    .await
    .unwrap();

    Seeded {
        team,
        customer,
        email,
        segment,
        ticket,
        session,
        survey,
        feedback,
        article: article.id.to_string(),
        experience,
        application,
        page,
        component,
    }
}

/// What org B expects from one call made with org A's data in hand.
enum Expect {
    /// Refused as `not_found`: the id names nothing B can see.
    NotFound,
    /// Answered, with nothing of A's in it.
    Clean,
    /// Answered exactly as an org with no data answers it (aggregates, where
    /// a leak would be a number rather than an id).
    LikeAnEmptyOrg,
}

/// Every call org B makes to each tool, with org A's ids wherever the tool
/// takes one. Panics on a tool it has no case for, so a new tool cannot be
/// left out of the isolation test.
fn tool_cases(name: &str, a: &Seeded, b_customer: &str) -> Vec<(Value, Expect)> {
    use Expect::*;
    match name {
        // Scoped to the token's org; checked to name B's below.
        "whoami" | "usage" => vec![(json!({}), Clean)],

        "create_customer" => vec![(json!({"email": "b-new@example.com"}), Clean)],
        "list_customers" => vec![(json!({}), Clean), (json!({"limit": 200}), Clean)],
        "update_customer" => vec![(json!({"id": a.customer, "name": "stolen"}), NotFound)],
        "get_customer" => vec![
            (json!({"id": a.customer}), NotFound),
            (json!({"id": a.email}), NotFound),
            (json!({"id": format!("{MARK} Customer")}), NotFound),
        ],
        "search_customers" => vec![
            (json!({"query": MARK}), Clean),
            (json!({"query": a.email}), Clean),
        ],
        "record_interaction" => vec![(
            json!({"customer_id": a.customer, "interaction_type": "login"}),
            NotFound,
        )],
        "get_journey" => vec![(json!({"customer_id": a.customer}), NotFound)],
        "create_segment" => vec![(json!({"name": "b dynamic", "criteria": "{}"}), Clean)],
        "get_segment_members" => vec![(json!({"id": a.segment}), NotFound)],
        "get_customer_360" => vec![(json!({"id": a.customer}), NotFound)],

        "create_ticket" => vec![(
            json!({"customer_id": a.customer, "subject": "stolen"}),
            NotFound,
        )],
        "update_ticket" => vec![(json!({"id": a.ticket, "status": "closed"}), NotFound)],
        "get_ticket" => vec![(json!({"id": a.ticket}), NotFound)],
        "list_tickets" => vec![
            (json!({}), Clean),
            (json!({"status": "open"}), Clean),
            (json!({"customer_id": a.customer}), NotFound),
        ],
        "close_ticket" => vec![(json!({"id": a.ticket}), NotFound)],
        "search_knowledge_base" => vec![(json!({"query": MARK}), Clean)],
        "start_chat_session" => vec![(json!({"id": a.customer}), NotFound)],
        "send_chat_message" => vec![(
            json!({"session_id": a.session, "role": "agent", "content": "stolen"}),
            NotFound,
        )],
        "get_chat_messages" => vec![(json!({"session_id": a.session}), NotFound)],
        "end_chat_session" => vec![(json!({"id": a.session}), NotFound)],

        "get_survey" => vec![(json!({"id": a.survey}), NotFound)],
        "submit_survey_response" => vec![
            (
                json!({"survey_id": a.survey, "customer_id": b_customer, "answers": r#"{"score": 0}"#}),
                NotFound,
            ),
            (
                json!({"survey_id": a.survey, "customer_id": a.customer, "answers": r#"{"score": 0}"#}),
                NotFound,
            ),
        ],
        "create_survey" => vec![(json!({"title": "b survey", "questions": "[]"}), Clean)],
        "send_survey" => vec![
            (
                json!({"survey_id": a.survey, "customer_id": b_customer}),
                NotFound,
            ),
            (
                json!({"survey_id": a.survey, "customer_id": a.customer}),
                NotFound,
            ),
        ],
        "get_survey_responses" => vec![(json!({"id": a.survey}), NotFound)],
        "submit_feedback" => {
            let base = json!({"customer_id": b_customer, "feedback_type": "complaint"});
            let mut cases = vec![(
                json!({"customer_id": a.customer, "feedback_type": "complaint"}),
                NotFound,
            )];
            // A's catalog ids, one at a time, on B's own customer.
            for (field, value) in [
                ("experience_id", &a.experience),
                ("application_id", &a.application),
                ("page_id", &a.page),
                ("component_id", &a.component),
            ] {
                let mut args = base.clone();
                args[field] = json!(value);
                cases.push((args, NotFound));
            }
            cases
        }
        "get_feedback" => vec![
            (json!({}), Clean),
            (json!({"customer_id": a.customer}), NotFound),
        ],
        "calculate_nps" => vec![(json!({"id": a.survey}), NotFound)],
        "calculate_csat" => vec![
            (json!({}), LikeAnEmptyOrg),
            (json!({"feedback_type": "praise"}), LikeAnEmptyOrg),
        ],
        "analyze_sentiment" => vec![(json!({"text": "great service"}), Clean)],

        other => panic!("no isolation case for tool {other}: add one to tool_cases"),
    }
}

/// Fails if `body` carries any of org A's ids or text.
fn assert_clean(what: &str, body: &Value, a: &Seeded) {
    let text = body.to_string();
    assert!(!text.contains(MARK), "{what} leaked org A's text: {text}");
    for id in a.ids() {
        assert!(!text.contains(id), "{what} leaked org A's id {id}: {text}");
    }
}

/// Org B's own catalog chain: experience, application, page, component.
#[derive(Default)]
struct Own {
    catalog: [String; 4],
}

/// What org B expects from one route call made with org A's data in hand.
enum RouteExpect {
    /// `404 not_found`: the id names nothing B can see.
    NotFound,
    /// `400 unknown_team`: A's team is not a team of B's org.
    UnknownTeam,
    /// `200`, with nothing of A's in it.
    Clean,
    /// `200`, counting exactly B's rows.
    CountsOnlyB,
}

struct RouteCase {
    uri: String,
    body: Option<Value>,
    expect: RouteExpect,
}

/// Every call org B makes to each `/api/v1` route, with org A's ids wherever
/// the route takes one. Panics on a (method, path) it has no case for, so a
/// new route cannot be left out of the isolation test.
fn route_cases(method: &Method, path: &str, a: &Seeded, own: &Own) -> Vec<RouteCase> {
    use RouteExpect::*;
    let case = |uri: String, body: Option<Value>, expect| RouteCase { uri, body, expect };
    let at = |id: &str| path.replace("{id}", id);
    let catalog_filter = format!(
        "experience_id={}&application_id={}&page_id={}&component_id={}",
        a.experience, a.application, a.page, a.component
    );
    // (collection, A's id, B's own id, parent field, A's parent id)
    let catalog = [
        ("experiences", &a.experience, &own.catalog[0], None, None),
        (
            "applications",
            &a.application,
            &own.catalog[1],
            Some("experience_id"),
            Some(&a.experience),
        ),
        (
            "pages",
            &a.page,
            &own.catalog[2],
            Some("application_id"),
            Some(&a.application),
        ),
        (
            "components",
            &a.component,
            &own.catalog[3],
            Some("page_id"),
            Some(&a.page),
        ),
    ];
    for (collection, a_id, b_id, parent, a_parent) in catalog {
        let list = format!("/api/v1/{collection}");
        let item = format!("/api/v1/{collection}/{{id}}");
        if path == list && *method == Method::GET {
            let mut cases = vec![case(list.clone(), None, Clean)];
            if let (Some(field), Some(pid)) = (parent, a_parent) {
                cases.push(case(format!("{list}?{field}={pid}"), None, Clean));
            }
            return cases;
        }
        if path == list && *method == Method::POST {
            let mut cases = vec![
                case(list.clone(), Some(json!({"name": "b made"})), Clean),
                case(
                    list.clone(),
                    Some(json!({"name": "stolen", "team_id": a.team})),
                    UnknownTeam,
                ),
                case(
                    list.clone(),
                    Some(json!({"name": "stolen", "teamId": a.team})),
                    UnknownTeam,
                ),
            ];
            if let (Some(field), Some(pid)) = (parent, a_parent) {
                cases.push(case(
                    list.clone(),
                    Some(json!({"name": "stolen", field: pid})),
                    NotFound,
                ));
            }
            return cases;
        }
        if path == item {
            return match *method {
                Method::GET | Method::DELETE => vec![case(at(a_id), None, NotFound)],
                Method::PUT => vec![
                    case(at(a_id), Some(json!({"name": "stolen"})), NotFound),
                    case(at(b_id), Some(json!({"team_id": a.team})), UnknownTeam),
                    case(at(b_id), Some(json!({"teamId": a.team})), UnknownTeam),
                ],
                _ => panic!("no isolation case for {method} {path}: add one to route_cases"),
            };
        }
    }
    match (method.as_str(), path) {
        ("GET", "/api/v1/me/teams") => vec![case(path.into(), None, Clean)],
        ("GET", "/api/v1/dashboard/metrics") => vec![case(path.into(), None, CountsOnlyB)],
        ("GET", "/api/v1/customers") => vec![
            case(path.into(), None, Clean),
            case(format!("{path}?{catalog_filter}"), None, Clean),
        ],
        ("POST", "/api/v1/customers") => vec![case(
            path.into(),
            Some(json!({"email": "b-rest@example.com"})),
            Clean,
        )],
        ("GET", "/api/v1/customers/search") => vec![case(format!("{path}?q={MARK}"), None, Clean)],
        ("GET" | "PUT" | "DELETE", "/api/v1/customers/{id}") => vec![case(
            at(&a.customer),
            (method == Method::PUT).then(|| json!({"name": "stolen"})),
            NotFound,
        )],
        ("GET", "/api/v1/customers/{id}/360") => vec![case(at(&a.customer), None, NotFound)],
        ("GET", "/api/v1/tickets") => vec![
            case(path.into(), None, Clean),
            case(format!("{path}?customer_id={}", a.customer), None, Clean),
            case(format!("{path}?{catalog_filter}"), None, Clean),
        ],
        ("POST", "/api/v1/tickets") => vec![case(
            path.into(),
            Some(json!({"customer_id": a.customer, "subject": "stolen"})),
            NotFound,
        )],
        ("GET" | "PUT" | "DELETE", "/api/v1/tickets/{id}") => vec![case(
            at(&a.ticket),
            (method == Method::PUT).then(|| json!({"subject": "stolen"})),
            NotFound,
        )],
        ("GET", "/api/v1/feedback") => vec![
            case(path.into(), None, Clean),
            case(format!("{path}?customer_id={}", a.customer), None, Clean),
            case(format!("{path}?{catalog_filter}"), None, Clean),
        ],
        ("GET", "/api/v1/surveys") => vec![
            case(path.into(), None, Clean),
            case(format!("{path}?{catalog_filter}"), None, Clean),
        ],
        ("POST", "/api/v1/surveys") => vec![case(
            path.into(),
            Some(json!({"title": "b rest survey", "questions": []})),
            Clean,
        )],
        ("GET" | "PUT" | "DELETE", "/api/v1/surveys/{id}") => vec![case(
            at(&a.survey),
            (method == Method::PUT).then(|| json!({"title": "stolen"})),
            NotFound,
        )],
        ("GET", "/api/v1/surveys/{id}/responses") => vec![case(at(&a.survey), None, NotFound)],
        _ => panic!("no isolation case for {method} {path}: add one to route_cases"),
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn criterion_2_org_b_cannot_read_or_write_org_a_through_any_tool_or_route(pool: PgPool) {
    let s = server(pool).await;

    // The proof cxp-server runs at boot, against this database.
    let report = s.db.verify_tenant_isolation().await.expect("isolation");
    let summary = report.summary();
    assert!(
        summary.contains(r#"tenant isolation enforced as role "otto_app""#),
        "{summary}"
    );
    for table in PURGED_TABLES {
        let t = report
            .tables
            .iter()
            .find(|t| t.name == *table)
            .unwrap_or_else(|| panic!("{table} is not tenant-isolated"));
        assert!(t.rls_enabled && t.rls_forced, "{table}");
    }

    let a = s.caller();
    let seeded = seed(&s, &a).await;
    let before = s.rows_of(a.org).await;
    for (table, rows) in &before {
        assert!(
            !rows.as_array().unwrap().is_empty(),
            "org A has no {table} row, so its isolation is not exercised"
        );
    }

    // Org B: a writer with a customer of its own, so a refusal is about A's
    // id and not a missing customer. Org E: empty, for the aggregates.
    let b = s.caller();
    let b_customer = id(&s
        .ok(
            &b.token,
            "create_customer",
            json!({"email": "b@example.com"}),
        )
        .await);
    let empty = s.caller();

    let mut calls = 0;
    for name in cxp_mcp::tools::NAMES {
        for (args, expect) in tool_cases(name, &seeded, &b_customer) {
            calls += 1;
            let what = format!("{name}({args})");
            let got = s.tool(&b.token, name, args.clone()).await;
            match expect {
                Expect::NotFound => {
                    let err = got.expect_err(&format!("{what} should be refused"));
                    assert_eq!(code(&err), "not_found", "{what}: {err}");
                }
                Expect::Clean => {
                    let v = got.unwrap_or_else(|e| panic!("{what} failed: {e}"));
                    assert_clean(&what, &v, &seeded);
                }
                Expect::LikeAnEmptyOrg => {
                    let v = got.unwrap_or_else(|e| panic!("{what} failed: {e}"));
                    let baseline = s.ok(&empty.token, name, args).await;
                    assert_eq!(v, baseline, "{what}");
                }
            }
        }
    }
    assert!(calls > cxp_mcp::tools::NAMES.len());

    // Tools that are org-scoped without an id answer for B.
    let me = s.ok(&b.token, "whoami", json!({})).await;
    assert_eq!(me["org"]["id"], b.org.to_string());
    // B's own dynamic segment of "everyone" holds only B's customers.
    let everyone = id(&s
        .ok(
            &b.token,
            "create_segment",
            json!({"name": "everyone", "criteria": "{}"}),
        )
        .await);
    let members = s
        .ok(&b.token, "get_segment_members", json!({"id": everyone}))
        .await;
    assert_clean("B's dynamic segment", &members, &seeded);
    assert!(members["total"].as_i64().unwrap() >= 1, "{members}");

    // ---------------------------------------------------------- resources
    for uri in [
        format!("customer://profiles/{}", seeded.customer),
        format!("knowledge://articles/{}", seeded.article),
    ] {
        let err = s
            .resource(&b.token, &uri)
            .await
            .expect_err(&format!("{uri} should be refused"));
        assert_eq!(code(&err), "not_found", "{uri}: {err}");
    }
    // A sees its own through the same URIs, so the refusal is about the org.
    s.resource(
        &a.token,
        &format!("customer://profiles/{}", seeded.customer),
    )
    .await
    .unwrap();
    let dashboard = s.resource(&b.token, "metrics://dashboard").await.unwrap();
    assert_clean("metrics://dashboard", &dashboard, &seeded);
    // B's numbers, counted from B's rows as stored, past RLS.
    let rows: HashMap<&str, Value> = s.rows_of(b.org).await.into_iter().collect();
    let tickets = rows["tickets"].as_array().unwrap();
    let with = |status: &str| tickets.iter().filter(|t| t["status"] == status).count();
    let ratings: Vec<f64> = rows["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["rating"].as_f64())
        .collect();
    assert_eq!(dashboard["totalTickets"], tickets.len(), "{dashboard}");
    for (field, status) in [
        ("openTickets", "open"),
        ("inProgressTickets", "in_progress"),
        ("waitingTickets", "waiting"),
        ("resolvedTickets", "resolved"),
        ("closedTickets", "closed"),
    ] {
        assert_eq!(dashboard[field], with(status), "{field}: {dashboard}");
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    match dashboard["averageRating"].as_f64() {
        Some(avg) => assert!((avg - mean(&ratings)).abs() < 1e-6, "{dashboard}"),
        None => assert!(ratings.is_empty(), "{dashboard}"),
    }

    // ------------------------------------------------------------- routes
    // B's own catalog, for updates that try to attach A's team.
    let mut own = Own::default();
    for (collection, field, slot) in [
        ("experiences", None, 0usize),
        ("applications", Some("experience_id"), 1),
        ("pages", Some("application_id"), 2),
        ("components", Some("page_id"), 3),
    ] {
        let mut body = json!({"name": format!("b {collection}")});
        if let Some(field) = field {
            body[field] = json!(own.catalog[slot - 1]);
        }
        own.catalog[slot] = id(&s
            .api_ok(
                "POST",
                &format!("/api/v1/{collection}"),
                &b.token,
                Some(body),
            )
            .await);
    }

    // Every route in cxp-api's table, with A's ids wherever it takes one.
    let mut calls = 0;
    for (method, path) in cxp_api::route_table() {
        for case in route_cases(&method, path, &seeded, &own) {
            calls += 1;
            let what = format!("{method} {} {:?}", case.uri, case.body);
            let r = s
                .api(method.as_str(), &case.uri, Some(&b.token), case.body)
                .await;
            match case.expect {
                RouteExpect::NotFound => {
                    assert_eq!(r.status, StatusCode::NOT_FOUND, "{what}: {}", r.body);
                    assert_eq!(r.body["code"], "not_found", "{what}: {}", r.body);
                }
                RouteExpect::UnknownTeam => {
                    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{what}: {}", r.body);
                    assert_eq!(r.body["code"], "unknown_team", "{what}: {}", r.body);
                }
                RouteExpect::Clean => {
                    assert_eq!(r.status, StatusCode::OK, "{what}: {}", r.body);
                    assert_clean(&what, &r.body, &seeded);
                }
                RouteExpect::CountsOnlyB => {
                    assert_eq!(r.status, StatusCode::OK, "{what}: {}", r.body);
                    // Counted from B's rows as stored, past RLS.
                    let rows: HashMap<&str, Value> = s.rows_of(b.org).await.into_iter().collect();
                    let n = |t: &str| rows[t].as_array().unwrap().len();
                    let open = rows["tickets"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|t| t["status"] == "open")
                        .count();
                    assert_eq!(
                        r.body,
                        json!({"total_customers": n("customers"), "total_tickets": n("tickets"),
                               "open_tickets": open, "total_feedback": n("feedback"),
                               "total_surveys": n("surveys"), "total_experiences": n("experiences")}),
                        "{what}"
                    );
                }
            }
        }
    }
    assert!(calls > cxp_api::route_table().len());

    // A customer with A's email is B's own, and a different row (its
    // email is A's text, so this comes after the leak checks).
    let twin = s
        .ok(&b.token, "create_customer", json!({"email": seeded.email}))
        .await;
    assert_ne!(id(&twin), seeded.customer);
    // Nothing B did changed anything of A's.
    assert_eq!(s.rows_of(a.org).await, before);
    // And A still sees all of it.
    assert_eq!(
        id(&s
            .ok(&a.token, "get_customer", json!({"id": seeded.customer}))
            .await),
        seeded.customer
    );
}

// ============================================================ criterion 3

/// Every tool, in an order in which each one can succeed, with its
/// arguments. Must name exactly `cxp_mcp::tools::NAMES`.
const TOOL_ORDER: &[&str] = &[
    "whoami",
    "usage",
    "create_customer",
    "list_customers",
    "update_customer",
    "get_customer",
    "search_customers",
    "record_interaction",
    "get_journey",
    "create_segment",
    "get_segment_members",
    "get_customer_360",
    "create_ticket",
    "update_ticket",
    "get_ticket",
    "list_tickets",
    "close_ticket",
    "search_knowledge_base",
    "start_chat_session",
    "send_chat_message",
    "get_chat_messages",
    "end_chat_session",
    "create_survey",
    "get_survey",
    "send_survey",
    "submit_survey_response",
    "get_survey_responses",
    "submit_feedback",
    "get_feedback",
    "calculate_nps",
    "calculate_csat",
    "analyze_sentiment",
];

#[derive(Default)]
struct Made {
    customer: String,
    segment: String,
    ticket: String,
    session: String,
    survey: String,
}

fn happy_args(name: &str, m: &Made) -> Value {
    match name {
        "whoami" | "usage" | "list_customers" | "list_tickets" | "get_feedback"
        | "calculate_csat" => json!({}),
        "create_customer" => json!({"email": "cy@example.com", "name": "Cy"}),
        "update_customer" => json!({"id": m.customer, "company": "Cyco"}),
        "get_customer" | "get_customer_360" | "start_chat_session" => json!({"id": m.customer}),
        "search_customers" => json!({"query": "cy"}),
        "record_interaction" => {
            json!({"customer_id": m.customer, "interaction_type": "purchase"})
        }
        "get_journey" => json!({"customer_id": m.customer}),
        "create_segment" => json!({"name": "all", "criteria": "{}"}),
        "get_segment_members" => json!({"id": m.segment}),
        "create_ticket" => json!({"customer_id": m.customer, "subject": "Login fails"}),
        "update_ticket" => json!({"id": m.ticket, "priority": "high"}),
        "get_ticket" | "close_ticket" => json!({"id": m.ticket}),
        "search_knowledge_base" => json!({"query": "password"}),
        "send_chat_message" => {
            json!({"session_id": m.session, "role": "customer", "content": "hi"})
        }
        "get_chat_messages" => json!({"session_id": m.session}),
        "end_chat_session" => json!({"id": m.session}),
        "create_survey" => {
            json!({"title": "NPS", "questions": r#"[{"id": "q1", "type": "nps"}]"#})
        }
        "get_survey" | "get_survey_responses" | "calculate_nps" => json!({"id": m.survey}),
        "send_survey" => json!({"survey_id": m.survey, "customer_id": m.customer}),
        "submit_survey_response" => {
            json!({"survey_id": m.survey, "customer_id": m.customer, "answers": r#"{"score": 10}"#})
        }
        "submit_feedback" => {
            json!({"customer_id": m.customer, "feedback_type": "praise", "rating": 5})
        }
        "analyze_sentiment" => json!({"text": "love it"}),
        other => panic!("no arguments for tool {other}: add them to happy_args"),
    }
}

/// The URI and body that make one `/api/v1` route succeed for the caller of
/// criterion 3, after any setup it needs (a DELETE gets a row of its own).
/// Panics on a (method, path) it has no case for, so a new route cannot be
/// left out of the metering test.
async fn happy_route(
    s: &Server,
    t: &str,
    method: &Method,
    path: &str,
    m: &Made,
    catalog: &HashMap<&str, String>,
) -> (String, Option<Value>) {
    let at = |id: &str| path.replace("{id}", id);
    // A fresh row to delete, made through the route that creates it.
    let doomed = |collection: &'static str, body: Value| async move {
        id(&s
            .api_ok("POST", &format!("/api/v1/{collection}"), t, Some(body))
            .await)
    };
    let parent = |field: &str, collection: &str| -> Value {
        json!({"name": "child", field: catalog[collection]})
    };
    match (method.as_str(), path) {
        (
            "GET",
            "/api/v1/me/teams"
            | "/api/v1/dashboard/metrics"
            | "/api/v1/customers"
            | "/api/v1/tickets"
            | "/api/v1/feedback"
            | "/api/v1/surveys"
            | "/api/v1/experiences"
            | "/api/v1/applications"
            | "/api/v1/pages"
            | "/api/v1/components",
        ) => (path.into(), None),
        ("POST", "/api/v1/customers") => (
            path.into(),
            Some(json!({"email": format!("{}@example.com", Uuid::new_v4())})),
        ),
        ("GET", "/api/v1/customers/search") => (format!("{path}?q=cy"), None),
        ("GET", "/api/v1/customers/{id}" | "/api/v1/customers/{id}/360") => (at(&m.customer), None),
        ("PUT", "/api/v1/customers/{id}") => (at(&m.customer), Some(json!({"name": "Cy C"}))),
        ("DELETE", "/api/v1/customers/{id}") => {
            let spare = doomed("customers", json!({"email": "spare@example.com"})).await;
            (at(&spare), None)
        }
        ("POST", "/api/v1/tickets") => (
            path.into(),
            Some(json!({"customer_id": m.customer, "subject": "Slow"})),
        ),
        ("GET", "/api/v1/tickets/{id}") => (at(&m.ticket), None),
        ("PUT", "/api/v1/tickets/{id}") => (at(&m.ticket), Some(json!({"status": "open"}))),
        ("DELETE", "/api/v1/tickets/{id}") => {
            let ticket = doomed(
                "tickets",
                json!({"customer_id": m.customer, "subject": "Doomed"}),
            )
            .await;
            (at(&ticket), None)
        }
        ("POST", "/api/v1/surveys") => {
            (path.into(), Some(json!({"title": "CSAT", "questions": []})))
        }
        ("GET", "/api/v1/surveys/{id}" | "/api/v1/surveys/{id}/responses") => (at(&m.survey), None),
        ("PUT", "/api/v1/surveys/{id}") => (at(&m.survey), Some(json!({"active": true}))),
        ("DELETE", "/api/v1/surveys/{id}") => {
            // Creating it ships create_survey; the caller ships that first.
            let survey = doomed("surveys", json!({"title": "Doomed", "questions": []})).await;
            (at(&survey), None)
        }
        ("POST", "/api/v1/experiences") => (path.into(), Some(json!({"name": "web"}))),
        ("POST", "/api/v1/applications") => {
            (path.into(), Some(parent("experience_id", "experiences")))
        }
        ("POST", "/api/v1/pages") => (path.into(), Some(parent("application_id", "applications"))),
        ("POST", "/api/v1/components") => (path.into(), Some(parent("page_id", "pages"))),
        (
            "GET" | "PUT" | "DELETE",
            "/api/v1/experiences/{id}"
            | "/api/v1/applications/{id}"
            | "/api/v1/pages/{id}"
            | "/api/v1/components/{id}",
        ) => {
            let collection = path
                .strip_prefix("/api/v1/")
                .and_then(|p| p.strip_suffix("/{id}"))
                .unwrap();
            match *method {
                Method::GET => (at(&catalog[collection]), None),
                Method::PUT => (at(&catalog[collection]), Some(json!({"description": "d"}))),
                _ => {
                    let leaf = id(&s
                        .api_ok(
                            "POST",
                            &format!("/api/v1/{collection}"),
                            t,
                            Some(json!({"name": "doomed"})),
                        )
                        .await);
                    (at(&leaf), None)
                }
            }
        }
        _ => panic!("no metering case for {method} {path}: add one to happy_route"),
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn criterion_3_exactly_the_billable_calls_ship_one_usage_event_each(pool: PgPool) {
    let s = server(pool).await;
    let mut order: Vec<&str> = TOOL_ORDER.to_vec();
    let mut names: Vec<&str> = cxp_mcp::tools::NAMES.to_vec();
    order.sort_unstable();
    names.sort_unstable();
    assert_eq!(order, names, "TOOL_ORDER must list every tool once");

    let a = s.caller();
    let t = a.token.as_str();

    // whoami and usage answer from the platform's records.
    let me = s.ok(t, "whoami", json!({})).await;
    assert_eq!(me["org"]["id"], a.org.to_string(), "{me}");
    assert_eq!(me["org"]["slug"], "acme");
    assert_eq!(me["usage"]["billableUsed"], 3, "{me}");
    let usage = s.ok(t, "usage", json!({})).await;
    assert_eq!(usage["usage"]["plan"], "Team", "{usage}");
    assert_eq!(usage["usage"]["remaining"], 997, "{usage}");
    assert_eq!(s.ship().await, 0, "whoami and usage record nothing");

    // Every tool once, shipping after each: a billable one ships exactly one
    // event naming it, any other ships nothing.
    let mut made = Made::default();
    for name in TOOL_ORDER {
        let out = s.ok(t, name, happy_args(name, &made)).await;
        match *name {
            "create_customer" => made.customer = id(&out),
            "create_segment" => made.segment = id(&out),
            "create_ticket" => made.ticket = id(&out),
            "start_chat_session" => made.session = id(&out),
            "create_survey" => made.survey = id(&out),
            _ => {}
        }
        let before = s.shipped().len();
        let shipped = s.ship().await;
        let new: Vec<String> = s.shipped()[before..].to_vec();
        if BILLABLE.contains(name) {
            assert_eq!(shipped, 1, "{name}");
            assert_eq!(new, [name.to_string()], "{name}");
        } else {
            assert_eq!((shipped, new.len()), (0, 0), "{name} shipped {new:?}");
        }
    }

    // Every route in cxp-api's table, once each: only POST /api/v1/surveys
    // ships, as create_survey. Deletes go last (each on a row made for it),
    // so every other route has something to act on.
    let mut routes = cxp_api::route_table();
    routes.sort_by_key(|(m, _)| *m == Method::DELETE);
    let mut catalog: HashMap<&str, String> = HashMap::new();
    for (method, path) in routes {
        let (uri, body) = happy_route(&s, t, &method, path, &made, &catalog).await;
        // Whatever the setup made is not this call's usage.
        s.ship().await;
        let before = s.shipped().len();
        let reply = s.api_ok(method.as_str(), &uri, t, body).await;
        if method == Method::POST {
            if let Some(collection) = path.strip_prefix("/api/v1/") {
                catalog.insert(collection, id(&reply));
            }
        }
        s.ship().await;
        let new: Vec<String> = s.shipped()[before..].to_vec();
        if method == Method::POST && path == "/api/v1/surveys" {
            assert_eq!(new, ["create_survey"], "{method} {path}");
        } else {
            assert!(new.is_empty(), "{method} {path} shipped {new:?}");
        }
    }
    assert_eq!(s.ship().await, 0, "free routes shipped something");

    // One event per billable call (the second survey was made so DELETE
    // had one to delete), each with its own event id, for this org and user.
    let received = s.mock.lock().unwrap().received.clone();
    let mut tools: Vec<&str> = received.iter().map(|e| e.tool.as_str()).collect();
    tools.sort_unstable();
    assert_eq!(
        tools,
        [
            "create_survey",
            "create_survey",
            "create_survey",
            "send_survey",
            "submit_feedback",
            "submit_survey_response"
        ]
    );
    let ids: HashSet<Uuid> = received.iter().map(|e| e.event_id).collect();
    assert_eq!(ids.len(), received.len(), "event ids are unique");
    for e in &received {
        assert_eq!((e.org_id, e.user_id), (a.org, Some(a.user)));
        assert!(e.billable);
    }

    // A shipment whose answer was lost is shipped again with the same event
    // id; the platform reports it as a duplicate, and the outbox still
    // drains without counting it twice.
    let replayed = &received[0];
    sqlx::query(
        "INSERT INTO usage_outbox (event_id, org_id, user_id, tool, billable, occurred_at) \
         VALUES ($1, $2, $3, $4, true, $5)",
    )
    .bind(replayed.event_id)
    .bind(replayed.org_id)
    .bind(replayed.user_id)
    .bind(&replayed.tool)
    .bind(replayed.occurred_at)
    .execute(&s.pool)
    .await
    .unwrap();
    assert_eq!(s.ship().await, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_outbox")
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "a duplicate is accepted and leaves the outbox");
    let p = s.mock.lock().unwrap();
    assert_eq!(p.received.len(), received.len() + 1);
    assert_eq!(p.counted.len(), received.len(), "counted once");
}

// ============================================================ criterion 4

async fn status_of(s: &Server, token: &str) -> (StatusCode, StatusCode) {
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "list_customers", "arguments": {}}});
    let mcp = s.send(Server::mcp_request(Some(token), &list)).await;
    let api = s.api("GET", "/api/v1/customers", Some(token), None).await;
    if mcp.status == StatusCode::UNAUTHORIZED {
        assert!(mcp.headers.contains_key("www-authenticate"));
    }
    (mcp.status, api.status)
}

async fn events_recorded(s: &Server, event: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM platform_events WHERE event_id = $1")
        .bind(event)
        .fetch_one(&s.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn criterion_4_lifecycle_webhooks_are_verified_and_applied_once(pool: PgPool) {
    let s = server(pool).await;
    let ok = (StatusCode::OK, StatusCode::OK);
    let refused = (StatusCode::UNAUTHORIZED, StatusCode::UNAUTHORIZED);

    // Two orgs with data; A is about to be deleted.
    let a = s.caller();
    seed(&s, &a).await;
    let b = s.caller();
    s.ok(
        &b.token,
        "create_customer",
        json!({"email": "b@example.com"}),
    )
    .await;
    let b_rows = s.rows_of(b.org).await;
    assert_eq!(status_of(&s, &a.token).await, ok);

    // ---------------------------------------------------------- signatures
    let deleted = Uuid::new_v4();
    let (body, sig) = signed(deleted, "org.deleted", json!({"org_id": a.org}));
    let forged = otto_resource::webhook::sign("otto_whsec_wrong", Utc::now().timestamp(), &body);
    for bad in [forged.as_str(), "t=1,v1=00", ""] {
        let r = s.webhook(&body, bad).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{bad:?}: {}", r.body);
    }
    // A genuine signature from an hour ago is a replay: refused.
    let stale = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp() - 3600, &body);
    let r = s.webhook(&body, &stale).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "stale: {}", r.body);
    // Correctly signed but not an event: 400, nothing applied.
    let junk = br#"{"not": "an event"}"#;
    let junk_sig = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), junk);
    let r = s.webhook(junk, &junk_sig).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "malformed: {}", r.body);
    assert_eq!(events_recorded(&s, deleted).await, 0);
    assert_eq!(status_of(&s, &a.token).await, ok, "nothing applied yet");

    // ---------------------------------------------------------- org.deleted
    let r = s.webhook(&body, &sig).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["result"], "applied");
    for (table, rows) in s.rows_of(a.org).await {
        assert_eq!(rows, json!([]), "org.deleted left {table} rows");
    }
    // The platform still vouches for A's token (its cache), and both
    // surfaces refuse it.
    assert_eq!(status_of(&s, &a.token).await, refused);
    // Replayed (re-signed, same event id): a duplicate with no further effect.
    let resigned = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), &body);
    let r = s.webhook(&body, &resigned).await;
    assert_eq!(
        (r.status, r.body["result"].clone()),
        (StatusCode::OK, json!("duplicate"))
    );
    assert_eq!(events_recorded(&s, deleted).await, 1);
    assert_eq!(status_of(&s, &a.token).await, refused);
    // Org B is untouched.
    assert_eq!(s.rows_of(b.org).await, b_rows);
    assert_eq!(status_of(&s, &b.token).await, ok);

    // ------------------------------------------------------- member.removed
    let c1 = s.caller();
    let c2 = s.caller_in(c1.org);
    assert_eq!(status_of(&s, &c1.token).await, ok);
    let removed = Uuid::new_v4();
    let (body, sig) = signed(
        removed,
        "member.removed",
        json!({"org_id": c1.org, "user_id": c1.user}),
    );
    let r = s.webhook(&body, &sig).await;
    assert_eq!(r.body["result"], "applied", "{}", r.body);
    assert_eq!(status_of(&s, &c1.token).await, refused);
    assert_eq!(status_of(&s, &c2.token).await, ok, "only that member");
    let resigned = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), &body);
    let r = s.webhook(&body, &resigned).await;
    assert_eq!(r.body["result"], "duplicate", "{}", r.body);
    assert_eq!(events_recorded(&s, removed).await, 1);
    assert_eq!(status_of(&s, &c1.token).await, refused);
    assert_eq!(status_of(&s, &c2.token).await, ok);

    // --------------------------------------------------------- team.deleted
    let d = s.caller();
    let gone = s.add_team(d.org, "gone");
    let kept = s.add_team(d.org, "kept");
    let mut owned = vec![];
    let mut parent: Option<(&str, String)> = None;
    for collection in ["experiences", "applications", "pages", "components"] {
        for team in [gone, kept] {
            let mut body = json!({"name": collection, "team_id": team});
            if let Some((field, pid)) = &parent {
                body[*field] = json!(pid);
            }
            let made = id(&s
                .api_ok(
                    "POST",
                    &format!("/api/v1/{collection}"),
                    &d.token,
                    Some(body),
                )
                .await);
            owned.push((collection, made, team));
        }
        let (_, last_gone, _) = &owned[owned.len() - 2];
        parent = Some((
            match collection {
                "experiences" => "experience_id",
                "applications" => "application_id",
                _ => "page_id",
            },
            last_gone.clone(),
        ));
    }
    let team_event = Uuid::new_v4();
    let (body, sig) = signed(
        team_event,
        "team.deleted",
        json!({"org_id": d.org, "team_id": gone}),
    );
    let r = s.webhook(&body, &sig).await;
    assert_eq!(r.body["result"], "applied", "{}", r.body);
    let check = |label: &'static str| {
        let owned = owned.clone();
        let s = &s;
        let token = d.token.clone();
        async move {
            for (collection, made, team) in &owned {
                let r = s
                    .api_ok("GET", &format!("/api/v1/{collection}/{made}"), &token, None)
                    .await;
                let expected = if *team == gone {
                    Value::Null
                } else {
                    json!(team)
                };
                assert_eq!(r["teamId"], expected, "{label}: {collection} {made}");
            }
        }
    };
    check("applied").await;
    // A catalog entity is re-assigned to the surviving team after the
    // delete; the replay must not touch it.
    let (_, reassigned, _) = owned[0].clone();
    s.api_ok(
        "PUT",
        &format!("/api/v1/experiences/{reassigned}"),
        &d.token,
        Some(json!({"team_id": kept})),
    )
    .await;
    let resigned = otto_resource::webhook::sign(WEBHOOK_SECRET, Utc::now().timestamp(), &body);
    let r = s.webhook(&body, &resigned).await;
    assert_eq!(r.body["result"], "duplicate", "{}", r.body);
    assert_eq!(events_recorded(&s, team_event).await, 1);
    let r = s
        .api_ok(
            "GET",
            &format!("/api/v1/experiences/{reassigned}"),
            &d.token,
            None,
        )
        .await;
    assert_eq!(r["teamId"], json!(kept), "the replay changed nothing");
    // The catalog of the deleted team still exists, unowned.
    let listed = s.api_ok("GET", "/api/v1/experiences", &d.token, None).await;
    assert_eq!(listed["total"], 2, "{listed}");
}

// ======================================================== the lifecycle

/// Log output captured from the test's own task.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// `run`, as `main` calls it, on a real socket: it boots against the test
/// database and the mock platform, logs the lines the deploy doc checks for,
/// serves a billable call, and on shutdown flushes that call's usage event to
/// the platform before returning.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_server_boots_serves_and_flushes_usage_on_shutdown(pool: PgPool) {
    let logs = Logs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (base, mock) = start_platform().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut database_url =
        url::Url::parse(&std::env::var("DATABASE_URL").expect("DATABASE_URL")).unwrap();
    database_url.set_path(pool.connect_options().get_database().unwrap());
    let mut config = Config::for_test();
    config.database_url = database_url.to_string();
    config.bind = addr;
    config.public_url = format!("http://{addr}");
    config.resource_uri = format!("http://{addr}/mcp");
    config.platform_url = base;
    let resource = config.resource_uri.clone();

    let (org, user) = (Uuid::new_v4(), Uuid::new_v4());
    let token = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
    mock.lock().unwrap().tokens.insert(
        token.clone(),
        (org, user, vec!["cxp:write".into()], resource),
    );

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = cxp_server::run_with(
        config,
        listener,
        async move {
            let _ = stopped.await;
        },
        // No periodic pass during the test: what the platform receives can
        // only come from the flush on the way out.
        cxp_core::usage::ShipperConfig {
            poll: std::time::Duration::from_secs(3600),
            ..Default::default()
        },
    );
    let client = async move {
        let http = reqwest::Client::new();
        let base = format!("http://{addr}");
        let ready = http.get(format!("{base}/readyz")).send().await.unwrap();
        assert_eq!(ready.status().as_u16(), 200);
        let call = |name: &str, args: Value| {
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                              "params": {"name": name, "arguments": args}});
            http.post(format!("{base}/mcp"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(body.to_string())
                .send()
        };
        for (name, args) in [
            ("create_customer", json!({"email": "life@example.com"})),
            (
                "submit_feedback",
                json!({"customer_id": "life@example.com", "feedback_type": "praise"}),
            ),
        ] {
            let res = call(name, args).await.unwrap();
            assert_eq!(res.status().as_u16(), 200, "{name}");
            let v: Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
            assert!(
                v["result"]["isError"] != true && v.get("error").is_none(),
                "{name}: {v}"
            );
        }
        // The shipper's poll is an hour here; nothing has shipped it yet, so
        // what arrives below was sent by the flush on the way out.
        assert!(mock.lock().unwrap().received.is_empty());
        stop.send(()).unwrap();
        mock
    };
    let (boot, mock) = tokio::join!(server, client);
    let boot = boot.expect("a clean stop");

    assert_eq!(boot.platform, cxp_server::PlatformCheck::Accepted);
    assert!(
        boot.isolation
            .contains(r#"tenant isolation enforced as role "otto_app""#),
        "{}",
        boot.isolation
    );
    let received = mock.lock().unwrap().received.clone();
    assert_eq!(
        received.len(),
        1,
        "the final flush shipped the billable call"
    );
    assert_eq!(
        (
            received[0].tool.as_str(),
            received[0].org_id,
            received[0].user_id
        ),
        ("submit_feedback", org, Some(user))
    );

    // The lines docs/deploy/fly.md tells an operator to look for, in order.
    let text = logs.text();
    let mut from = 0;
    for line in [
        "applying migrations",
        r#"tenant isolation enforced as role "otto_app""#,
        "the otto platform accepted this service's credential",
        "cxp-server listening",
        "flushing usage",
        "stopped",
    ] {
        let at = text[from..]
            .find(line)
            .unwrap_or_else(|| panic!("{line:?} not logged after byte {from}:\n{text}"));
        from += at + line.len();
    }
}

/// A database where one tenant table no longer enforces row-level security
/// is refused before anything is served: `run` returns the refusal and never
/// accepts a connection. (`NO FORCE` alone would not break it here: the
/// tenant role `otto_app` does not own the table, so RLS still applies to it,
/// and the boot proof knows that.)
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_server_refuses_to_serve_when_tenant_isolation_is_broken(pool: PgPool) {
    // Every callsite this boot logs through gets an interest computed with a
    // subscriber present, so the log assertions of the other boot test, run
    // concurrently on another thread, see their lines.
    let subscriber = tracing_subscriber::fmt()
        .with_writer(Logs::default)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    sqlx::query("ALTER TABLE customers DISABLE ROW LEVEL SECURITY")
        .execute(&pool)
        .await
        .unwrap();
    let (base, _) = start_platform().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut database_url =
        url::Url::parse(&std::env::var("DATABASE_URL").expect("DATABASE_URL")).unwrap();
    database_url.set_path(pool.connect_options().get_database().unwrap());
    let mut config = Config::for_test();
    config.database_url = database_url.to_string();
    config.bind = addr;
    config.public_url = format!("http://{addr}");
    config.resource_uri = format!("http://{addr}/mcp");
    config.platform_url = base;

    // The shutdown signal never fires: only a refusal can end this.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        cxp_server::run(config, listener, std::future::pending()),
    )
    .await
    .expect("a refused boot returns rather than serving");
    let err = format!("{:#}", result.expect_err("boot refused"));
    assert!(err.contains("refusing to serve"), "{err}");
    // Nothing listens: the listener was dropped with the refusal.
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}
