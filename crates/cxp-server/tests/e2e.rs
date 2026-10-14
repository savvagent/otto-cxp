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
use http::{HeaderMap, Request, StatusCode};
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
    );
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

    // ------------------------------------------------------------- routes
    let bt = b.token.as_str();
    let not_found = |r: &Reply, what: &str| {
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{what}: {}", r.body);
        assert_eq!(r.body["code"], "not_found", "{what}: {}", r.body);
    };

    // Every route that takes an id, with A's, by every method it serves.
    for (collection, aid, update) in [
        ("customers", &seeded.customer, json!({"name": "stolen"})),
        ("tickets", &seeded.ticket, json!({"subject": "stolen"})),
        ("surveys", &seeded.survey, json!({"title": "stolen"})),
        ("experiences", &seeded.experience, json!({"name": "stolen"})),
        (
            "applications",
            &seeded.application,
            json!({"name": "stolen"}),
        ),
        ("pages", &seeded.page, json!({"name": "stolen"})),
        ("components", &seeded.component, json!({"name": "stolen"})),
    ] {
        let uri = format!("/api/v1/{collection}/{aid}");
        not_found(
            &s.api("GET", &uri, Some(bt), None).await,
            &format!("GET {uri}"),
        );
        not_found(
            &s.api("PUT", &uri, Some(bt), Some(update)).await,
            &format!("PUT {uri}"),
        );
        not_found(
            &s.api("DELETE", &uri, Some(bt), None).await,
            &format!("DELETE {uri}"),
        );
    }
    for uri in [
        format!("/api/v1/customers/{}/360", seeded.customer),
        format!("/api/v1/surveys/{}/responses", seeded.survey),
    ] {
        not_found(
            &s.api("GET", &uri, Some(bt), None).await,
            &format!("GET {uri}"),
        );
    }
    // Creates that reference A's rows.
    for (uri, body) in [
        (
            "/api/v1/tickets",
            json!({"customer_id": seeded.customer, "subject": "stolen"}),
        ),
        (
            "/api/v1/applications",
            json!({"name": "stolen", "experience_id": seeded.experience}),
        ),
        (
            "/api/v1/pages",
            json!({"name": "stolen", "application_id": seeded.application}),
        ),
        (
            "/api/v1/components",
            json!({"name": "stolen", "page_id": seeded.page}),
        ),
    ] {
        not_found(
            &s.api("POST", uri, Some(bt), Some(body)).await,
            &format!("POST {uri}"),
        );
    }
    // A's team is not a team in B's org.
    let r = s
        .api(
            "POST",
            "/api/v1/experiences",
            Some(bt),
            Some(json!({"name": "stolen", "team_id": seeded.team})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(r.body["code"], "unknown_team");

    // Every list, plain and filtered by A's ids: answered, nothing of A's.
    let c = &seeded.customer;
    let (e, ap, p, co) = (
        &seeded.experience,
        &seeded.application,
        &seeded.page,
        &seeded.component,
    );
    for uri in [
        "/api/v1/me/teams".to_string(),
        "/api/v1/customers".to_string(),
        format!("/api/v1/customers/search?q={MARK}"),
        format!(
            "/api/v1/customers?experience_id={e}&application_id={ap}&page_id={p}&component_id={co}"
        ),
        "/api/v1/tickets".to_string(),
        format!("/api/v1/tickets?customer_id={c}"),
        format!(
            "/api/v1/tickets?experience_id={e}&application_id={ap}&page_id={p}&component_id={co}"
        ),
        "/api/v1/feedback".to_string(),
        format!("/api/v1/feedback?customer_id={c}"),
        format!(
            "/api/v1/feedback?experience_id={e}&application_id={ap}&page_id={p}&component_id={co}"
        ),
        "/api/v1/surveys".to_string(),
        format!(
            "/api/v1/surveys?experience_id={e}&application_id={ap}&page_id={p}&component_id={co}"
        ),
        "/api/v1/experiences".to_string(),
        "/api/v1/applications".to_string(),
        format!("/api/v1/applications?experience_id={e}"),
        "/api/v1/pages".to_string(),
        format!("/api/v1/pages?application_id={ap}"),
        "/api/v1/components".to_string(),
        format!("/api/v1/components?page_id={p}"),
    ] {
        let body = s.api_ok("GET", &uri, bt, None).await;
        assert_clean(&format!("GET {uri}"), &body, &seeded);
    }
    // The dashboard counts B's rows only: its two customers and the one
    // survey it created, none of A's.
    let dash = s.api_ok("GET", "/api/v1/dashboard/metrics", bt, None).await;
    assert_eq!(
        dash,
        json!({"total_customers": 2, "total_tickets": 0, "open_tickets": 0,
               "total_feedback": 0, "total_surveys": 1, "total_experiences": 0}),
        "{dash}"
    );

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

    // Every route: only POST /api/v1/surveys ships, as create_survey.
    let c = made.customer.clone();
    let mut routes: Vec<(&str, String, Option<Value>, bool)> = vec![
        ("GET", "/api/v1/me/teams".into(), None, false),
        ("GET", "/api/v1/dashboard/metrics".into(), None, false),
        ("GET", "/api/v1/customers".into(), None, false),
        ("GET", "/api/v1/customers/search?q=cy".into(), None, false),
        ("GET", format!("/api/v1/customers/{c}"), None, false),
        ("GET", format!("/api/v1/customers/{c}/360"), None, false),
        (
            "PUT",
            format!("/api/v1/customers/{c}"),
            Some(json!({"name": "Cy C"})),
            false,
        ),
        ("GET", "/api/v1/tickets".into(), None, false),
        ("GET", "/api/v1/feedback".into(), None, false),
        ("GET", "/api/v1/surveys".into(), None, false),
        (
            "POST",
            "/api/v1/surveys".into(),
            Some(json!({"title": "CSAT", "questions": []})),
            true,
        ),
    ];
    for collection in ["experiences", "applications", "pages", "components"] {
        routes.push(("GET", format!("/api/v1/{collection}"), None, false));
    }
    for (method, uri, body, billable) in routes {
        let reply = s.api_ok(method, &uri, t, body).await;
        let before = s.shipped().len();
        s.ship().await;
        let new: Vec<String> = s.shipped()[before..].to_vec();
        if billable {
            assert_eq!(new, ["create_survey"], "{method} {uri}");
            let survey = id(&reply);
            // Reading, changing, and deleting it are free.
            for (m, body) in [
                ("GET", None),
                ("PUT", Some(json!({"active": false}))),
                ("DELETE", None),
            ] {
                s.api_ok(m, &format!("/api/v1/surveys/{survey}"), t, body)
                    .await;
            }
            s.api_ok(
                "GET",
                &format!("/api/v1/surveys/{}/responses", made.survey),
                t,
                None,
            )
            .await;
        } else {
            assert!(new.is_empty(), "{method} {uri} shipped {new:?}");
        }
    }
    // Ticket and catalog writes are free too.
    let ticket = id(&s
        .api_ok(
            "POST",
            "/api/v1/tickets",
            t,
            Some(json!({"customer_id": c, "subject": "Slow"})),
        )
        .await);
    s.api_ok(
        "PUT",
        &format!("/api/v1/tickets/{ticket}"),
        t,
        Some(json!({"status": "closed"})),
    )
    .await;
    s.api_ok("GET", &format!("/api/v1/tickets/{ticket}"), t, None)
        .await;
    s.api_ok("DELETE", &format!("/api/v1/tickets/{ticket}"), t, None)
        .await;
    let mut parent: Option<(&str, String)> = None;
    let mut catalog = vec![];
    for collection in ["experiences", "applications", "pages", "components"] {
        let mut body = json!({"name": collection});
        if let Some((field, pid)) = &parent {
            body[*field] = json!(pid);
        }
        let made_id = id(&s
            .api_ok("POST", &format!("/api/v1/{collection}"), t, Some(body))
            .await);
        let uri = format!("/api/v1/{collection}/{made_id}");
        s.api_ok("GET", &uri, t, None).await;
        s.api_ok("PUT", &uri, t, Some(json!({"description": "d"})))
            .await;
        parent = Some((
            match collection {
                "experiences" => "experience_id",
                "applications" => "application_id",
                _ => "page_id",
            },
            made_id,
        ));
        catalog.push(uri);
    }
    for uri in catalog.iter().rev() {
        s.api_ok("DELETE", uri, t, None).await;
    }
    let spare = id(&s
        .api_ok(
            "POST",
            "/api/v1/customers",
            t,
            Some(json!({"email": "spare@example.com"})),
        )
        .await);
    s.api_ok("DELETE", &format!("/api/v1/customers/{spare}"), t, None)
        .await;
    assert_eq!(s.ship().await, 0, "free routes shipped something");

    // Five events in all, one per billable call, each with its own event id,
    // for this org and user.
    let received = s.mock.lock().unwrap().received.clone();
    let mut tools: Vec<&str> = received.iter().map(|e| e.tool.as_str()).collect();
    tools.sort_unstable();
    assert_eq!(
        tools,
        [
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
