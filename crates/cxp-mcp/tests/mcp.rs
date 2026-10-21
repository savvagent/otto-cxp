//! The MCP surface at the crate level: the router built against a real
//! Postgres and a stub otto platform, driven with JSON-RPC over `POST /mcp`
//! as an agent drives it. Covers authentication (401 with the discovery
//! pointer, inactive and wrong-audience tokens, a platform outage as 503),
//! scopes, the list envelope, metering (exactly one usage event per billable
//! call, none for anything else), `whoami`/`usage`, a cross-org read, and
//! resources and prompts. The every-tool cross-org e2e lives in cxp-server.
//! Needs `DATABASE_URL`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use chrono::Utc;
use cxp_core::search::SearchLimiter;
use cxp_core::usage::Meter;
use cxp_mcp::{router, Config};
use http::{Request, StatusCode};
use otto_resource::{
    ClientConfig, IntrospectionResponse, MemberInfo, OrgInfo, PlatformClient, Role, TokenKind,
    UserInfo,
};
use otto_tenant::Db;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const RESOURCE: &str = "https://cxp.example.com/mcp";
const OTHER_RESOURCE: &str = "https://flags.example.com/mcp";
const PUBLIC_URL: &str = "https://cxp.example.com";
const HOST: &str = "cxp.example.com";
const READ: &[&str] = &["cxp:read"];
const WRITE: &[&str] = &["cxp:write"];

// ------------------------------------------------------------- stub platform

#[derive(Default)]
struct Platform {
    /// token -> (org, user, scopes, audience)
    tokens: HashMap<String, (Uuid, Uuid, Vec<String>, String)>,
    /// Answer every introspection with a 500.
    down: bool,
    /// Answer every usage-status request with a 503.
    usage_down: bool,
    /// Report every org's billable calls as blocked: its otto-cxp budget is
    /// spent with the hard stop on.
    blocked: bool,
    /// How many usage-status requests have arrived.
    usage_lookups: usize,
}

type Shared = Arc<Mutex<Platform>>;

async fn introspect(
    State(p): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Json<IntrospectionResponse>, StatusCode> {
    let p = p.lock().unwrap();
    if p.down {
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

async fn member(Path((org, user)): Path<(Uuid, Uuid)>) -> Json<MemberInfo> {
    Json(MemberInfo {
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
    })
}

async fn usage_status(
    State(p): State<Shared>,
    Path(org): Path<Uuid>,
) -> Result<Json<Value>, StatusCode> {
    let mut p = p.lock().unwrap();
    p.usage_lookups += 1;
    if p.usage_down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    // A v0.10.0 platform: the legacy bucket never blocks; the budget does.
    let mut body = json!({
        "org_id": org,
        "plan": "team",
        "period_start": Utc::now().date_naive(),
        "billable_count": 3,
        "total_count": 3,
        "included_ops": 1000,
        "hard_stop": false,
        "service": "otto-cxp",
        "meters": [{
            "meter": cxp_core::usage::RESPONSES_COLLECTED,
            "display_name": "Responses collected",
            "unit": "response",
            "quantity": 3,
        }],
    });
    if p.blocked {
        body["blocked_reason"] = json!("budget_exceeded");
        body["budget"] = json!({
            "monthly_budget_cents": 1000,
            "spend_cents": 1000,
            "percent_used": 100,
            "thresholds_reached": [50, 80, 100],
            "hard_stop": true,
            "blocked": true,
            "reason": "budget_exceeded",
        });
    }
    Ok(Json(body))
}

async fn start_platform() -> (String, Shared) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/oauth/introspect", post(introspect))
        .route("/internal/orgs/{org}/members/{user}", get(member))
        .route("/internal/orgs/{org}/usage-status", get(usage_status))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), shared)
}

/// Mint a well-formed token the stub platform introspects as given.
fn token_for(p: &Shared, org: Uuid, scopes: &[&str], aud: &str) -> String {
    let t = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
    assert!(otto_resource::looks_like_token(&t), "{t}");
    p.lock().unwrap().tokens.insert(
        t.clone(),
        (
            org,
            Uuid::new_v4(),
            scopes.iter().map(|s| s.to_string()).collect(),
            aud.to_string(),
        ),
    );
    t
}

fn token(p: &Shared, org: Uuid, scopes: &[&str]) -> String {
    token_for(p, org, scopes, RESOURCE)
}

// ------------------------------------------------------------------ harness

struct Harness {
    /// The mock platform's base URL, which the upgrade URL starts with.
    base: String,
    app: Router,
    pool: PgPool,
    platform: Shared,
}

async fn harness(pool: PgPool) -> Harness {
    harness_search(pool, SearchLimiter::for_pool(5)).await
}

/// [`harness`], with the given search limiter.
async fn harness_search(pool: PgPool, search: SearchLimiter) -> Harness {
    let (base, platform) = start_platform().await;
    let client =
        Arc::new(PlatformClient::new(ClientConfig::new(&base, RESOURCE, "otto_rs_test")).unwrap());
    let meter = Arc::new(Meter::new(
        client.clone(),
        format!("{base}/settings/billing"),
    ));
    let app = router(
        Db::from_pool(pool.clone()),
        client,
        meter,
        search,
        Config {
            public_url: PUBLIC_URL.into(),
            allowed_hosts: vec![HOST.into()],
            allowed_origins: vec![],
        },
    )
    .unwrap();
    Harness {
        base,
        app,
        pool,
        platform,
    }
}

impl Harness {
    async fn send(&self, req: Request<Body>) -> (StatusCode, http::HeaderMap, Value) {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 22)
            .await
            .unwrap();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn post(auth: Option<&str>, body: Value) -> Request<Body> {
        let mut req = Request::post("/mcp")
            .header("host", HOST)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(t) = auth {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        req.body(Body::from(body.to_string())).unwrap()
    }

    /// One JSON-RPC call; `Ok(result)` or `Err(error)`.
    async fn rpc(&self, token: &str, method: &str, params: Value) -> Result<Value, Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let (status, _, v) = self.send(Self::post(Some(token), body)).await;
        assert!(status.is_success(), "{method}: HTTP {status} {v}");
        match v.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(v["result"].clone()),
        }
    }

    /// One tool call; `Ok(structuredContent)` or `Err(error)`.
    async fn tool(&self, token: &str, name: &str, args: Value) -> Result<Value, Value> {
        let result = self
            .rpc(
                token,
                "tools/call",
                json!({"name": name, "arguments": args}),
            )
            .await?;
        if result["isError"] == true {
            return Err(result);
        }
        Ok(result["structuredContent"].clone())
    }

    /// The tools recorded in this org's usage outbox, sorted.
    async fn charged(&self, org: Uuid) -> Vec<String> {
        sqlx::query_scalar("SELECT tool FROM usage_outbox WHERE org_id = $1 ORDER BY tool")
            .bind(org)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    /// The (tool, meter, quantity) of every usage row `org` has recorded,
    /// by tool.
    async fn metered(&self, org: Uuid) -> Vec<(String, Option<String>, Option<i64>)> {
        sqlx::query_as(
            "SELECT tool, meter, quantity FROM usage_outbox WHERE org_id = $1 ORDER BY tool",
        )
        .bind(org)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

fn code(e: &Value) -> &str {
    e["data"]["code"].as_str().unwrap_or_default()
}

// --------------------------------------------------------------------- auth

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn no_token_is_a_401_naming_the_metadata_document(pool: PgPool) {
    let h = harness(pool).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (status, headers, v) = h.send(Harness::post(None, body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let challenge = headers["www-authenticate"].to_str().unwrap();
    assert!(
        challenge.contains(
            r#"resource_metadata="https://cxp.example.com/.well-known/oauth-protected-resource""#
        ),
        "{challenge}"
    );
    assert!(!challenge.contains("error="), "{challenge}");
    assert_eq!(v["error"], "unauthenticated");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_cookie_alone_is_not_a_credential_on_mcp(pool: PgPool) {
    let h = harness(pool).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (s0, h0, v0) = h.send(Harness::post(None, body.clone())).await;
    let mut req = Harness::post(None, body);
    req.headers_mut()
        .insert("cookie", "session=abc; otto_session=abc".parse().unwrap());
    let (status, headers, v) = h.send(req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!((status, &v), (s0, &v0));
    assert_eq!(headers["www-authenticate"], h0["www-authenticate"]);
}

/// The converse of `/api`, where the web console's session cookie is a
/// credential: a *live* session (stored, its token one the platform honours)
/// presented as the console's cookie, under every name it is ever sent with,
/// still gets exactly the no-credential `401` on `/mcp`.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_live_console_session_cookie_is_not_a_credential_on_mcp(pool: PgPool) {
    use cxp_core::console_sessions::{self, NewSession};
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let access = token(&h.platform, org, WRITE);
    let cookie = "A".repeat(43);
    console_sessions::create(
        &Db::from_pool(h.pool.clone()),
        &otto_tenant::crypto::Cipher::from_base64_key(
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        )
        .unwrap(),
        &NewSession {
            id_hash: console_sessions::hash_cookie(&cookie),
            org_id: org.into(),
            user_id: Uuid::new_v4().into(),
            access_token: &access,
            refresh_token: "otto_rt_x",
            access_expires_at: Utc::now() + chrono::Duration::hours(1),
            expires_at: Utc::now() + chrono::Duration::days(1),
        },
    )
    .await
    .unwrap();

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (s0, h0, v0) = h.send(Harness::post(None, body.clone())).await;
    let mut req = Harness::post(None, body);
    req.headers_mut().insert(
        "cookie",
        format!("__Host-cxp_session={cookie}; cxp_session={cookie}")
            .parse()
            .unwrap(),
    );
    req.headers_mut()
        .insert("origin", PUBLIC_URL.parse().unwrap());
    let (status, headers, v) = h.send(req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!((status, &v), (s0, &v0));
    assert_eq!(headers["www-authenticate"], h0["www-authenticate"]);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn inactive_and_wrong_audience_tokens_are_401(pool: PgPool) {
    let h = harness(pool).await;
    let unknown = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
    let elsewhere = token_for(&h.platform, Uuid::new_v4(), WRITE, OTHER_RESOURCE);
    for t in [unknown, elsewhere] {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "whoami", "arguments": {}}});
        let (status, headers, _) = h.send(Harness::post(Some(&t), body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers["www-authenticate"].to_str().unwrap();
        assert!(
            challenge.contains(r#"error="invalid_token""#),
            "{challenge}"
        );
        assert!(challenge.contains("resource_metadata="), "{challenge}");
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_platform_outage_is_a_503_not_a_401(pool: PgPool) {
    let h = harness(pool).await;
    let t = token(&h.platform, Uuid::new_v4(), READ);
    h.platform.lock().unwrap().down = true;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (status, headers, v) = h.send(Harness::post(Some(&t), body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers["retry-after"], "5");
    assert!(headers.get("www-authenticate").is_none());
    assert_eq!(v["error"], "unavailable");
}

// ------------------------------------------------------------------- scopes

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_read_token_reads_but_cannot_write(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let reader = token(&h.platform, org, READ);

    let listed = h.tool(&reader, "list_customers", json!({})).await.unwrap();
    assert_eq!(listed, json!({"items": [], "total": 0}));

    let refused = h
        .tool(
            &reader,
            "create_customer",
            json!({"email": "a@example.com"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "insufficient_scope", "{refused}");
    assert_eq!(refused["data"]["retriable"], false);
    assert!(refused["message"].as_str().unwrap().contains("cxp:write"));

    // A token with neither cxp scope cannot read either.
    let stranger = token(&h.platform, org, &["flags:read"]);
    let refused = h
        .tool(&stranger, "list_customers", json!({}))
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "insufficient_scope", "{refused}");
}

// --------------------------------------------------------------- list shape

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn list_tools_return_items_and_total(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let writer = token(&h.platform, org, WRITE);
    for (email, name) in [("ada@example.com", "Ada"), ("bob@example.com", "Bob")] {
        h.tool(
            &writer,
            "create_customer",
            json!({"email": email, "name": name}),
        )
        .await
        .unwrap();
    }

    let page = h
        .tool(&writer, "list_customers", json!({"limit": 1}))
        .await
        .unwrap();
    assert_eq!(page["total"], 2, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    let next = h
        .tool(&writer, "list_customers", json!({"limit": 1, "offset": 1}))
        .await
        .unwrap();
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);

    // A search counts its matches up to 1000 and says whether it stopped
    // there (#17); other lists carry no `totalCapped`.
    let found = h
        .tool(&writer, "search_customers", json!({"query": "ada"}))
        .await
        .unwrap();
    assert_eq!(found["items"][0]["email"], "ada@example.com", "{found}");
    assert_eq!(found["total"], 1, "{found}");
    assert_eq!(found["totalCapped"], false, "{found}");
    assert!(page.get("totalCapped").is_none(), "{page}");

    // A bad JSON argument is invalid_argument, not internal.
    let bad = h
        .tool(
            &writer,
            "create_segment",
            json!({"name": "s", "criteria": "{not json"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&bad), "invalid_argument", "{bad}");
}

// ----------------------------------------------------------------- metering

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn each_billable_tool_records_exactly_one_usage_event(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let writer = token(&h.platform, org, WRITE);

    // Free writes and reads record nothing.
    let customer = h
        .tool(
            &writer,
            "create_customer",
            json!({"email": "cy@example.com", "name": "Cy"}),
        )
        .await
        .unwrap();
    h.tool(
        &writer,
        "create_ticket",
        json!({"customer_id": "cy@example.com", "subject": "Login fails"}),
    )
    .await
    .unwrap();
    h.tool(
        &writer,
        "get_customer_360",
        json!({"id": customer["id"].clone()}),
    )
    .await
    .unwrap();
    assert!(h.charged(org).await.is_empty());

    let survey = h
        .tool(
            &writer,
            "create_survey",
            json!({"title": "NPS", "questions": r#"[{"id": "q1", "type": "nps"}]"#}),
        )
        .await
        .unwrap();
    assert!(h.charged(org).await.is_empty(), "create_survey is free");

    h.tool(
        &writer,
        "send_survey",
        json!({"survey_id": survey["id"].clone(), "customer_id": "Cy"}),
    )
    .await
    .unwrap();
    assert_eq!(h.charged(org).await, ["send_survey"]);

    h.tool(
        &writer,
        "submit_survey_response",
        json!({"survey_id": survey["id"].clone(), "customer_id": customer["id"].clone(),
               "answers": r#"{"score": 9}"#}),
    )
    .await
    .unwrap();
    h.tool(
        &writer,
        "submit_feedback",
        json!({"customer_id": "cy@example.com", "feedback_type": "praise", "rating": 5}),
    )
    .await
    .unwrap();
    let row = |tool: &str, meter: &str| (tool.to_string(), Some(meter.to_string()), Some(1));
    assert_eq!(
        h.metered(org).await,
        [
            row("send_survey", cxp_core::usage::SURVEYS_SENT),
            row("submit_feedback", cxp_core::usage::RESPONSES_COLLECTED),
            row(
                "submit_survey_response",
                cxp_core::usage::RESPONSES_COLLECTED
            ),
        ]
    );

    // A billable call that fails is not billed: its charge rolls back.
    let failed = h
        .tool(
            &writer,
            "submit_feedback",
            json!({"customer_id": "nobody@example.com", "feedback_type": "praise"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&failed), "not_found", "{failed}");
    assert_eq!(h.charged(org).await.len(), 3);

    // The analytics over what was recorded.
    let nps = h
        .tool(
            &writer,
            "calculate_nps",
            json!({"id": survey["id"].clone()}),
        )
        .await
        .unwrap();
    assert_eq!(nps["nps"], 100.0, "{nps}");
    assert_eq!(nps["promoters"], 1, "{nps}");
    let csat = h.tool(&writer, "calculate_csat", json!({})).await.unwrap();
    assert_eq!(csat["csat"], 100.0, "{csat}");
    assert_eq!(h.charged(org).await.len(), 3);
}

/// A read or a free write (`create_survey` among them) never calls the
/// platform for the org's usage; only a billable tool looks it up, before its
/// transaction.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn only_billable_tools_look_the_quota_up(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let writer = token(&h.platform, org, WRITE);
    let lookups = || h.platform.lock().unwrap().usage_lookups;

    h.tool(
        &writer,
        "create_customer",
        json!({"email": "di@example.com", "name": "Di"}),
    )
    .await
    .unwrap();
    h.tool(&writer, "list_customers", json!({})).await.unwrap();
    h.tool(
        &writer,
        "create_survey",
        json!({"title": "NPS", "questions": "[]"}),
    )
    .await
    .unwrap();
    h.rpc(
        &writer,
        "resources/read",
        json!({"uri": "metrics://dashboard"}),
    )
    .await
    .unwrap();
    assert_eq!(lookups(), 0);

    h.tool(
        &writer,
        "submit_feedback",
        json!({"customer_id": "di@example.com", "feedback_type": "praise", "rating": 5}),
    )
    .await
    .unwrap();
    assert_eq!(lookups(), 1);
    assert_eq!(h.charged(org).await, ["submit_feedback"]);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn whoami_and_usage_need_no_scope_and_record_nothing(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let bare = token(&h.platform, org, &[]);

    let me = h.tool(&bare, "whoami", json!({})).await.unwrap();
    assert_eq!(me["org"]["id"], org.to_string(), "{me}");
    assert_eq!(me["org"]["slug"], "acme");
    assert_eq!(me["role"], "member");
    assert_eq!(me["token"]["canRead"], false);
    assert_eq!(me["usage"]["billableUsed"], 3);
    assert_eq!(me["usage"]["remaining"], 997);
    assert_eq!(me["usage"]["enforced"], true, "{me}");
    assert_eq!(me["usage"]["service"], "otto-cxp", "{me}");
    assert_eq!(
        me["usage"]["meters"],
        json!([
            {"meter": cxp_core::usage::SURVEYS_SENT, "displayName": "Surveys sent",
             "unit": "survey", "quantity": 0},
            {"meter": cxp_core::usage::RESPONSES_COLLECTED,
             "displayName": "Responses collected", "unit": "response", "quantity": 3},
        ]),
        "{me}"
    );

    let usage = h.tool(&bare, "usage", json!({})).await.unwrap();
    assert_eq!(usage["usage"]["plan"], "Team", "{usage}");

    assert!(h.charged(org).await.is_empty());
}

// ------------------------------------------------------------------ tenancy

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn another_org_cannot_read_a_customer(pool: PgPool) {
    let h = harness(pool).await;
    let a = token(&h.platform, Uuid::new_v4(), WRITE);
    let b = token(&h.platform, Uuid::new_v4(), WRITE);
    let customer = h
        .tool(&a, "create_customer", json!({"email": "dee@example.com"}))
        .await
        .unwrap();

    for id in [customer["id"].clone(), json!("dee@example.com")] {
        let refused = h
            .tool(&b, "get_customer", json!({"id": id}))
            .await
            .unwrap_err();
        assert_eq!(code(&refused), "not_found", "{refused}");
    }
    let uri = format!("customer://profiles/{}", customer["id"].as_str().unwrap());
    let refused = h
        .rpc(&b, "resources/read", json!({"uri": uri}))
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "not_found", "{refused}");
    // MCP's resource-not-found.
    assert_eq!(refused["code"], -32002, "{refused}");

    // Org A still sees it.
    let mine = h
        .tool(&a, "get_customer", json!({"id": "dee@example.com"}))
        .await
        .unwrap();
    assert_eq!(mine["id"], customer["id"]);
}

// ------------------------------------------------------ resources & prompts

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn resources_and_prompts_are_served_with_read_scope(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let writer = token(&h.platform, org, WRITE);
    let reader = token(&h.platform, org, READ);
    let stranger = token(&h.platform, org, &["flags:read"]);
    let customer = h
        .tool(
            &writer,
            "create_customer",
            json!({"email": "eve@example.com"}),
        )
        .await
        .unwrap();

    let listed = h.rpc(&reader, "resources/list", json!({})).await.unwrap();
    assert_eq!(listed["resources"][0]["uri"], "metrics://dashboard");
    let templates = h
        .rpc(&reader, "resources/templates/list", json!({}))
        .await
        .unwrap();
    assert_eq!(templates["resourceTemplates"].as_array().unwrap().len(), 2);

    let dash = h
        .rpc(
            &reader,
            "resources/read",
            json!({"uri": "metrics://dashboard"}),
        )
        .await
        .unwrap();
    let text: Value = serde_json::from_str(dash["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["totalTickets"], 0, "{text}");

    let uri = format!("customer://profiles/{}", customer["id"].as_str().unwrap());
    let profile = h
        .rpc(&reader, "resources/read", json!({"uri": uri}))
        .await
        .unwrap();
    let text: Value =
        serde_json::from_str(profile["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["email"], "eve@example.com");

    let refused = h
        .rpc(
            &stranger,
            "resources/read",
            json!({"uri": "metrics://dashboard"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "insufficient_scope", "{refused}");

    let prompts = h.rpc(&reader, "prompts/list", json!({})).await.unwrap();
    assert_eq!(prompts["prompts"].as_array().unwrap().len(), 4);
    let prompt = h
        .rpc(
            &reader,
            "prompts/get",
            json!({"name": "customer_summary", "arguments": {"customer_id": "eve@example.com"}}),
        )
        .await
        .unwrap();
    let text = prompt["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(text.contains("get_customer_360"), "{text}");

    let refused = h
        .rpc(
            &stranger,
            "prompts/get",
            json!({"name": "customer_summary", "arguments": {"customer_id": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "insufficient_scope", "{refused}");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn knowledge_articles_and_unknown_uris(pool: PgPool) {
    use cxp_core::knowledge::{ArticleUpdate, KnowledgeExt, NewArticle};
    let h = harness(pool.clone()).await;
    let org = Uuid::new_v4();
    let reader = token(&h.platform, org, READ);

    // No tool writes articles; seed one through cxp-core in org's own
    // transaction, as the REST surface does.
    let db = Db::from_pool(pool);
    let mut tx = db.begin(org.into()).await.unwrap();
    let article = tx
        .create_article(&NewArticle {
            title: "Resetting a password".into(),
            content: "Use the reset link on the sign-in page.".into(),
            category: Some("account".into()),
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

    let uri = format!("knowledge://articles/{}", article.id);
    let read = h
        .rpc(&reader, "resources/read", json!({"uri": uri}))
        .await
        .unwrap();
    let text: Value = serde_json::from_str(read["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["title"], "Resetting a password", "{text}");
    assert_eq!(read["contents"][0]["mimeType"], "application/json");

    // Another org's reader gets resource-not-found for the same id.
    let other = token(&h.platform, Uuid::new_v4(), READ);
    let refused = h
        .rpc(&other, "resources/read", json!({"uri": uri}))
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "not_found", "{refused}");
    assert_eq!(refused["code"], -32002, "{refused}");

    let found = h
        .tool(
            &reader,
            "search_knowledge_base",
            json!({"query": "password"}),
        )
        .await
        .unwrap();
    assert_eq!(found["items"][0]["id"], article.id.to_string(), "{found}");
    assert_eq!(found["total"], 1, "{found}");
    assert_eq!(found["totalCapped"], false, "{found}");

    for bad in ["file:///etc/passwd", "knowledge://articles/not-a-uuid"] {
        let refused = h
            .rpc(&reader, "resources/read", json!({"uri": bad}))
            .await
            .unwrap_err();
        assert_eq!(code(&refused), "invalid_argument", "{bad}: {refused}");
        assert_eq!(refused["data"]["retriable"], false);
    }
    let refused = h
        .rpc(&reader, "prompts/get", json!({"name": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "invalid_argument", "{refused}");
}

/// While the org's otto-cxp budget is spent with the hard stop on, a
/// billable tool is refused with `budget_exceeded`, records nothing, and
/// names where to fix it; free tools, `create_survey` included, still work.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_org_out_of_budget_is_refused_billable_calls(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let writer = token(&h.platform, org, WRITE);
    h.platform.lock().unwrap().blocked = true;

    h.tool(
        &writer,
        "create_customer",
        json!({"email": "q@example.com", "name": "Q"}),
    )
    .await
    .unwrap();
    let survey = h
        .tool(
            &writer,
            "create_survey",
            json!({"title": "NPS", "questions": "[]"}),
        )
        .await
        .unwrap();

    let refused = h
        .tool(
            &writer,
            "send_survey",
            json!({"survey_id": survey["id"].clone(), "customer_id": "q@example.com"}),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&refused), "budget_exceeded", "{refused}");
    assert_eq!(refused["data"]["retriable"], false);
    let message = refused["message"].as_str().unwrap();
    assert!(message.contains("send_survey"), "{refused}");
    assert!(message.contains(&h.base), "{refused}");
    assert!(h.charged(org).await.is_empty());
    let responses: i64 =
        sqlx::query_scalar("SELECT count(*) FROM survey_responses WHERE org_id = $1")
            .bind(org)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(responses, 0, "a refused send_survey wrote a response");

    let usage = h.tool(&writer, "usage", json!({})).await.unwrap();
    assert_eq!(
        usage["usage"]["blockedReason"], "budget_exceeded",
        "{usage}"
    );
    assert_eq!(usage["usage"]["warning"], true, "{usage}");
    assert_eq!(usage["usage"]["budget"]["blocked"], true, "{usage}");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn whoami_answers_without_usage_when_the_platform_cannot_report_it(pool: PgPool) {
    let h = harness(pool).await;
    let org = Uuid::new_v4();
    let t = token(&h.platform, org, READ);
    h.platform.lock().unwrap().usage_down = true;

    let me = h.tool(&t, "whoami", json!({})).await.unwrap();
    assert_eq!(me["org"]["id"], org.to_string(), "{me}");
    assert!(me["usage"].is_null(), "{me}");

    let failed = h.tool(&t, "usage", json!({})).await.unwrap_err();
    assert_eq!(code(&failed), "unavailable", "{failed}");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_foreign_host_header_is_refused(pool: PgPool) {
    let h = harness(pool).await;
    let t = token(&h.platform, Uuid::new_v4(), READ);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "whoami", "arguments": {}}});
    let req = Request::post("/mcp")
        .header("host", "evil.example.net")
        .header("authorization", format!("Bearer {t}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, _, _) = h.send(req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// rmcp reads an empty allowed-host list as "accept any Host", so the crate
/// refuses to build a router from one (or from only blank entries) whoever
/// the caller is.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_router_refuses_an_empty_allowed_host_list(pool: PgPool) {
    let (base, _) = start_platform().await;
    let client =
        Arc::new(PlatformClient::new(ClientConfig::new(&base, RESOURCE, "otto_rs_test")).unwrap());
    for hosts in [vec![], vec![" ".to_string(), String::new()]] {
        let meter = Arc::new(Meter::new(client.clone(), "https://x"));
        let built = router(
            Db::from_pool(pool.clone()),
            client.clone(),
            meter,
            SearchLimiter::for_pool(5),
            Config {
                public_url: PUBLIC_URL.into(),
                allowed_hosts: hosts.clone(),
                allowed_origins: vec![],
            },
        );
        assert_eq!(built.err(), Some(cxp_mcp::NoAllowedHosts), "{hosts:?}");
    }
}

/// #42: a search tool that cannot get a permit fails as retriable
/// `unavailable`, holding no pooled connection while it waits.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_search_tool_without_a_permit_is_unavailable(pool: PgPool) {
    let limiter = SearchLimiter::new(1);
    let h = harness_search(pool, limiter.clone()).await;
    let t = token(&h.platform, Uuid::new_v4(), READ);
    let _held = limiter.acquire().await.unwrap();
    for (tool, args) in [
        ("search_customers", json!({"query": "grace"})),
        ("search_knowledge_base", json!({"query": "reset"})),
    ] {
        // Halfway through the permit wait (500 ms), nothing is checked out.
        let sample = async {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            h.pool.size() - h.pool.num_idle() as u32
        };
        let (failed, busy) = tokio::join!(h.tool(&t, tool, args), sample);
        let failed = failed.unwrap_err();
        assert_eq!(busy, 0, "{tool} waited for a permit holding a connection");
        assert_eq!(code(&failed), "unavailable", "{tool}: {failed}");
        assert_eq!(failed["data"]["retriable"], true, "{tool}: {failed}");
    }
}
