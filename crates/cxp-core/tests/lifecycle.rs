//! The platform integration against a real Postgres and a stub otto platform:
//! `begin_live`'s refusals, the lifecycle webhooks (`org.deleted`,
//! `member.removed`, `team.deleted`, unknown kinds) applied once and
//! acknowledged as duplicates on replay, the retention sweep as `otto_app`,
//! usage metering and shipping, and bearer-token authentication.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use chrono::Utc;
use common::{db, org, otto_app_db, user};
use cxp_core::auth::{self, AuthFailure};
use cxp_core::platform_events::{self, begin_live, Outcome, PURGED_TABLES};
use cxp_core::usage::{self, Meter, ShipError, ShipperConfig};
use cxp_core::Error;
use otto_resource::webhook::WebhookEvent;
use otto_resource::{
    ClientConfig, IntrospectionResponse, PlatformClient, RejectedEvent, Role, TokenKind,
    UsageBatch, UsageEvent, UsageReceipt,
};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const RESOURCE: &str = "https://cxp.example.com/mcp";
const OTHER_RESOURCE: &str = "https://flags.example.com/mcp";

// ------------------------------------------------------------- stub platform

#[derive(Default)]
struct Platform {
    /// token -> (org, user, scopes, audience)
    tokens: HashMap<String, (Uuid, Uuid, Vec<String>, String)>,
    /// Answer every introspection with a 500.
    introspection_down: bool,
    /// Event ids already counted, so a resend is a duplicate.
    seen: HashSet<Uuid>,
    /// Event ids refused outright.
    reject: HashSet<Uuid>,
    /// Answer every usage batch with a 503.
    usage_down: bool,
    /// Account for one event fewer than was sent.
    undercount: bool,
    /// Every usage event received, in order.
    received: Vec<UsageEvent>,
    /// Billable count and hard stop reported by usage-status.
    used: i64,
    hard_stop: bool,
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

async fn ship(
    State(p): State<Shared>,
    Json(b): Json<UsageBatch>,
) -> Result<Json<UsageReceipt>, StatusCode> {
    let mut p = p.lock().unwrap();
    if p.usage_down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut receipt = UsageReceipt::default();
    for e in &b.events {
        if p.reject.contains(&e.event_id) {
            receipt.rejected.push(RejectedEvent {
                event_id: e.event_id,
                reason: "unknown org".into(),
            });
        } else if !p.seen.insert(e.event_id) {
            receipt.duplicates += 1;
        } else {
            receipt.accepted += 1;
        }
    }
    if p.undercount && receipt.accepted > 0 {
        receipt.accepted -= 1;
    }
    p.received.extend(b.events);
    Ok(Json(receipt))
}

async fn usage_status(State(p): State<Shared>, Path(org): Path<Uuid>) -> Json<Value> {
    let p = p.lock().unwrap();
    Json(json!({
        "org_id": org,
        "plan": "free",
        "period_start": Utc::now().date_naive(),
        "billable_count": p.used,
        "total_count": p.used + 7,
        "included_ops": 10,
        "hard_stop": p.hard_stop,
    }))
}

async fn start_platform() -> (Arc<PlatformClient>, Shared) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route("/oauth/introspect", post(introspect))
        .route("/internal/usage", post(ship))
        .route("/internal/orgs/{org}/usage-status", get(usage_status))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (client(&format!("http://{addr}")), shared)
}

fn client(base: &str) -> Arc<PlatformClient> {
    Arc::new(PlatformClient::new(ClientConfig::new(base, RESOURCE, "otto_rs_test")).unwrap())
}

/// Mint a well-formed token the stub platform introspects as given.
fn token(p: &Shared, org: OrgId, user: UserId, scopes: &[&str], aud: &str) -> String {
    let t = format!("otto_at_{}{}", "a".repeat(11), Uuid::new_v4().simple());
    assert!(otto_resource::looks_like_token(&t), "{t}");
    p.lock().unwrap().tokens.insert(
        t.clone(),
        (
            org.as_uuid(),
            user.as_uuid(),
            scopes.iter().map(|s| s.to_string()).collect(),
            aud.to_string(),
        ),
    );
    t
}

// ------------------------------------------------------------------ fixtures

fn event(kind: &str, data: Value) -> WebhookEvent {
    let body = otto_resource::webhook::body(Uuid::new_v4(), kind, Utc::now(), &data);
    otto_resource::webhook::parse(&body).unwrap()
}

fn org_deleted(org: OrgId) -> WebhookEvent {
    event("org.deleted", json!({"org_id": org.as_uuid()}))
}

fn member_removed(org: OrgId, user: UserId) -> WebhookEvent {
    event(
        "member.removed",
        json!({"org_id": org.as_uuid(), "user_id": user.as_uuid()}),
    )
}

fn team_deleted(org: OrgId, team: Uuid) -> WebhookEvent {
    event(
        "team.deleted",
        json!({"org_id": org.as_uuid(), "team_id": team}),
    )
}

async fn exec(tx: &mut otto_tenant::Tx<'_>, sql: &str, binds: &[Uuid]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(tx.conn()).await.unwrap();
}

/// One row in every domain table, plus one unshipped usage event, for `org`,
/// written as `otto_app` pinned to it. Catalog entities belong to `team`.
async fn seed(db: &Db, org: OrgId, team: Uuid) {
    let o = org.as_uuid();
    let [exp, app, page, comp, cust, seg, session, survey] = [(); 8].map(|_| Uuid::new_v4());
    let mut tx = db.begin(org).await.unwrap();
    exec(
        &mut tx,
        "INSERT INTO experiences (id, org_id, name, team_id) VALUES ($1, $2, 'Checkout', $3)",
        &[exp, o, team],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO applications (id, org_id, experience_id, name, team_id) \
         VALUES ($1, $2, $3, 'Web', $4)",
        &[app, o, exp, team],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO pages (id, org_id, application_id, name, team_id) \
         VALUES ($1, $2, $3, 'Cart', $4)",
        &[page, o, app, team],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO components (id, org_id, page_id, name, team_id) \
         VALUES ($1, $2, $3, 'Pay', $4)",
        &[comp, o, page, team],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO customers (id, org_id, email, experience_id) \
         VALUES ($1, $2, $1::text || '@example.com', $3)",
        &[cust, o, exp],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO interactions (org_id, customer_id, interaction_type) \
         VALUES ($1, $2, 'login')",
        &[o, cust],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO segments (id, org_id, name, criteria) VALUES ($1, $2, 'VIP', '{}')",
        &[seg, o],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO segment_members (org_id, segment_id, customer_id) VALUES ($1, $2, $3)",
        &[o, seg, cust],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO tickets (org_id, customer_id, subject, page_id) \
         VALUES ($1, $2, 'Card declined', $3)",
        &[o, cust, page],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO knowledge_articles (org_id, title, content) VALUES ($1, 'Refunds', 'How')",
        &[o],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO chat_sessions (id, org_id, customer_id) VALUES ($1, $2, $3)",
        &[session, o, cust],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO chat_messages (org_id, session_id, role, content) \
         VALUES ($1, $2, 'customer'::message_role, 'hello')",
        &[o, session],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO feedback (org_id, customer_id, feedback_type, component_id) \
         VALUES ($1, $2, 'praise'::feedback_type, $3)",
        &[o, cust, comp],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO surveys (id, org_id, title, questions, application_id) \
         VALUES ($1, $2, 'NPS', '[]', $3)",
        &[survey, o, app],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO survey_responses (org_id, survey_id, customer_id, answers) \
         VALUES ($1, $2, $3, '{}')",
        &[o, survey, cust],
    )
    .await;
    exec(
        &mut tx,
        "INSERT INTO usage_outbox (org_id, tool, billable) VALUES ($1, 'submit_feedback', true)",
        &[o],
    )
    .await;
    tx.commit().await.unwrap();
}

/// Rows `org` has in `table`, counted by the superuser test pool (no RLS).
async fn count(pool: &PgPool, table: &str, org: OrgId) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE org_id = $1"))
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every table holding one of an org's domain rows or unshipped usage. Spelled
/// out rather than taken from `PURGED_TABLES`, so dropping a table from the
/// purge fails this test.
const TENANT_TABLES: [&str; 16] = [
    "experiences",
    "applications",
    "pages",
    "components",
    "customers",
    "interactions",
    "segments",
    "segment_members",
    "tickets",
    "knowledge_articles",
    "chat_sessions",
    "chat_messages",
    "feedback",
    "surveys",
    "survey_responses",
    "usage_outbox",
];

// ---------------------------------------------------------------- begin_live

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn begin_live_refuses_a_deleted_org_and_a_removed_member(pool: PgPool) {
    let db = db(pool);
    let (live, gone) = (org(), org());
    let (stays, leaves) = (user(), user());

    // Both open before anything happens.
    for (o, u) in [(live, stays), (live, leaves), (gone, stays)] {
        begin_live(&db, o, Some(u))
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();
    }

    platform_events::apply(&db, &org_deleted(gone))
        .await
        .unwrap();
    platform_events::apply(&db, &member_removed(live, leaves))
        .await
        .unwrap();

    for (o, u) in [(gone, Some(stays)), (gone, None), (live, Some(leaves))] {
        let err = begin_live(&db, o, u).await.err().expect("must be refused");
        assert!(matches!(err, Error::AccessRevoked), "{err:?}");
        assert_eq!(err.code(), "access_revoked");
    }
    let mut tx = begin_live(&db, live, Some(stays)).await.unwrap();
    assert_eq!(tx.org(), live);
    let pinned: Option<String> = sqlx::query_scalar("SELECT current_setting('app.org_id', true)")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(pinned, Some(live.to_string()));
    tx.commit().await.unwrap();

    assert!(platform_events::revoked(&db, live, leaves).await.unwrap());
    assert!(!platform_events::revoked(&db, live, stays).await.unwrap());
    assert!(platform_events::revoked(&db, gone, stays).await.unwrap());
}

/// The removed-member refusal lasts only as long as the platform's cached
/// introspection could still vouch for the token.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_member_removal_expires(pool: PgPool) {
    let db = db(pool.clone());
    let (o, u) = (org(), user());
    platform_events::apply(&db, &member_removed(o, u))
        .await
        .unwrap();
    sqlx::query("UPDATE removed_members SET removed_at = now() - make_interval(secs => $1 + 1)")
        .bind(platform_events::REMOVED_MEMBER_TTL_SECS as f64)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!platform_events::revoked(&db, o, u).await.unwrap());
    begin_live(&db, o, Some(u)).await.unwrap();
}

// ------------------------------------------------------------------- webhooks

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn org_deleted_purges_every_tenant_table_once_and_spares_other_orgs(pool: PgPool) {
    let db = db(pool.clone());
    let (doomed, other) = (org(), org());
    let team = Uuid::new_v4();
    seed(&db, doomed, team).await;
    seed(&db, other, team).await;
    // A rejected usage event is kept, as otto-flags keeps it.
    sqlx::query(
        "INSERT INTO usage_outbox_rejected (event_id, org_id, tool, billable, occurred_at, reason) \
         VALUES (gen_random_uuid(), $1, 'submit_feedback', true, now(), 'test')",
    )
    .bind(doomed)
    .execute(&pool)
    .await
    .unwrap();

    let mut purged: Vec<&str> = PURGED_TABLES.to_vec();
    purged.sort_unstable();
    let mut expected = TENANT_TABLES.to_vec();
    expected.sort_unstable();
    assert_eq!(purged, expected, "the purge must cover every tenant table");

    let delivery = org_deleted(doomed);
    let Outcome::Applied { detail } = platform_events::apply(&db, &delivery).await.unwrap() else {
        panic!("the first delivery applies");
    };
    for table in TENANT_TABLES {
        assert_eq!(
            count(&pool, table, doomed).await,
            0,
            "{table} still holds rows"
        );
        assert_eq!(
            count(&pool, table, other).await,
            1,
            "{table} lost another org's rows"
        );
    }
    // The purge deletes children before parents, so no row goes by cascade:
    // every table reports the one row the org had in it.
    for table in TENANT_TABLES {
        assert_eq!(detail[table], 1, "{table}: {detail}");
    }
    assert_eq!(count(&pool, "usage_outbox_rejected", doomed).await, 1);
    // The dedupe marker and the tombstone survive the purge.
    assert_eq!(count(&pool, "platform_events", doomed).await, 1);
    assert_eq!(count(&pool, "deleted_orgs", doomed).await, 1);

    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Duplicate
    );
    assert_eq!(count(&pool, "platform_events", doomed).await, 1);
    assert_eq!(count(&pool, "deleted_orgs", doomed).await, 1);
    for table in TENANT_TABLES {
        assert_eq!(count(&pool, table, other).await, 1, "{table} after replay");
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn member_removed_applies_once(pool: PgPool) {
    let db = db(pool.clone());
    let (o, u) = (org(), user());
    let delivery = member_removed(o, u);
    assert!(matches!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Applied { .. }
    ));
    let first: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT removed_at FROM removed_members WHERE org_id = $1")
            .bind(o)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Duplicate
    );
    // A replay does not restart the refusal window.
    let after: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT removed_at FROM removed_members WHERE org_id = $1")
            .bind(o)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(first, after);
    assert_eq!(count(&pool, "platform_events", o).await, 1);
    assert_eq!(count(&pool, "removed_members", o).await, 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn team_deleted_nulls_only_that_org_and_teams_catalog(pool: PgPool) {
    let db = db(pool.clone());
    let (o, other) = (org(), org());
    let (team, kept_team) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&db, o, team).await;
    seed(&db, o, kept_team).await;
    // Another org with the same team id (ids are opaque here) is untouched.
    seed(&db, other, team).await;

    let owned = |org: OrgId, team: Uuid| {
        let pool = pool.clone();
        async move {
            let mut n = 0i64;
            for table in ["experiences", "applications", "pages", "components"] {
                n += sqlx::query_scalar::<_, i64>(&format!(
                    "SELECT count(*) FROM {table} WHERE org_id = $1 AND team_id = $2"
                ))
                .bind(org)
                .bind(team)
                .fetch_one(&pool)
                .await
                .unwrap();
            }
            n
        }
    };

    let delivery = team_deleted(o, team);
    let Outcome::Applied { detail } = platform_events::apply(&db, &delivery).await.unwrap() else {
        panic!("the first delivery applies");
    };
    assert_eq!(
        detail,
        json!({"experiences": 1, "applications": 1, "pages": 1, "components": 1})
    );
    assert_eq!(owned(o, team).await, 0);
    assert_eq!(owned(o, kept_team).await, 4);
    assert_eq!(owned(other, team).await, 4);
    // The entities themselves stay.
    assert_eq!(count(&pool, "experiences", o).await, 2);
    assert_eq!(count(&pool, "components", o).await, 2);

    // A replay is a no-op, even if the team id were written again since.
    sqlx::query("UPDATE experiences SET team_id = $2 WHERE org_id = $1 AND team_id IS NULL")
        .bind(o)
        .bind(team)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Duplicate
    );
    assert_eq!(owned(o, team).await, 1);
    assert_eq!(count(&pool, "platform_events", o).await, 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn unknown_events_are_ignored(pool: PgPool) {
    let db = db(pool.clone());
    let o = org();
    let delivery = event("org.renamed", json!({"org_id": o.as_uuid()}));
    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Ignored
    );
    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Ignored
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_sweep_deletes_old_rows_as_otto_app(pool: PgPool) {
    let (a, b) = (org(), org());
    let keep_days = 30;
    for o in [a, b] {
        for days in [keep_days + 1, 1] {
            sqlx::query(
                "INSERT INTO platform_events (event_id, org_id, kind, received_at) \
                 VALUES (gen_random_uuid(), $1, 'member.removed', now() - make_interval(days => $2))",
            )
            .bind(o)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
        for secs in [platform_events::REMOVED_MEMBER_TTL_SECS * 2 + 60, 10] {
            sqlx::query(
                "INSERT INTO removed_members (org_id, user_id, removed_at) \
                 VALUES ($1, gen_random_uuid(), now() - make_interval(secs => $2))",
            )
            .bind(o)
            .bind(secs as f64)
            .execute(&pool)
            .await
            .unwrap();
        }
    }

    // Every connection of this Db is otto_app, so RLS applies to the sweep:
    // only the maintenance marker lets it reach rows unpinned.
    let app_db = otto_app_db(&pool).await;
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(app_db.pool())
        .await
        .unwrap();
    assert_eq!(role, "otto_app");
    let swept = platform_events::sweep(&app_db, keep_days).await.unwrap();
    assert_eq!(
        swept, 4,
        "one old dedupe marker and one old tombstone per org"
    );
    app_db.pool().close().await;

    for o in [a, b] {
        assert_eq!(count(&pool, "platform_events", o).await, 1);
        assert_eq!(count(&pool, "removed_members", o).await, 1);
    }
}

// --------------------------------------------------------------------- usage

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn charge_records_one_row_per_billable_call_and_refuses_free_tools(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, _) = start_platform().await;
    let meter = Meter::new(platform, false, "https://otto.example.com/billing");
    let (o, u) = (org(), user());

    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    for tool in usage::BILLABLE {
        meter.charge(&mut tx, u, tool).await.unwrap();
    }
    for free in ["whoami", "usage", "create_customer", "list_feedback", ""] {
        let err = meter.charge(&mut tx, u, free).await.unwrap_err();
        assert_eq!(err.code(), "invalid_argument", "{free}: {err}");
    }
    tx.commit().await.unwrap();

    let rows: Vec<(Uuid, Uuid, Option<Uuid>, String, bool)> = sqlx::query_as(
        "SELECT event_id, org_id, user_id, tool, billable FROM usage_outbox ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), usage::BILLABLE.len(), "{rows:?}");
    let ids: HashSet<Uuid> = rows.iter().map(|r| r.0).collect();
    assert_eq!(ids.len(), rows.len(), "every event_id is unique");
    for ((_, org_id, user_id, tool, billable), expected) in rows.iter().zip(usage::BILLABLE) {
        assert_eq!(*org_id, o.as_uuid());
        assert_eq!(*user_id, Some(u.as_uuid()));
        assert_eq!(tool, expected);
        assert!(billable);
    }

    // A call whose transaction rolls back leaves no record.
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    meter.charge(&mut tx, u, "submit_feedback").await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool, "usage_outbox", o).await, 4);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_enforcing_meter_refuses_an_org_out_of_budget(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let (o, u) = (org(), user());
    {
        let mut p = stub.lock().unwrap();
        p.used = 9;
        p.hard_stop = true;
    }

    let meter = Meter::new(platform.clone(), true, "https://otto.example.com/billing");
    let report = meter.report(o).await.unwrap();
    assert_eq!(report.plan, "Free");
    assert_eq!(report.remaining, 1);
    assert!(report.warning && report.enforced && report.hard_stop);

    // 9 counted at the platform + this one = 10 of 10 included: allowed.
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    meter.charge(&mut tx, u, "create_survey").await.unwrap();
    tx.commit().await.unwrap();

    // The unshipped row counts on top, so the next one is over.
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    let err = meter.charge(&mut tx, u, "send_survey").await.unwrap_err();
    assert_eq!(err.code(), "quota_exceeded", "{err}");
    tx.rollback().await.unwrap();
    assert_eq!(count(&pool, "usage_outbox", o).await, 1);

    // A meter that does not enforce records it anyway.
    let lax = Meter::new(platform, false, "https://otto.example.com/billing");
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    lax.charge(&mut tx, u, "send_survey").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&pool, "usage_outbox", o).await, 2);
}

/// If the platform cannot say how much is used, billable work is allowed and
/// still recorded.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_enforcing_meter_fails_open(pool: PgPool) {
    let db = db(pool.clone());
    let meter = Meter::new(client("http://127.0.0.1:1"), true, "https://x");
    let (o, u) = (org(), user());
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    meter.charge(&mut tx, u, "submit_feedback").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&pool, "usage_outbox", o).await, 1);
    assert_eq!(meter.report(o).await.unwrap_err().code(), "unavailable");
}

async fn charge_n(db: &Db, meter: &Meter, o: OrgId, n: usize) {
    let u = user();
    let mut tx = begin_live(db, o, Some(u)).await.unwrap();
    for _ in 0..n {
        meter
            .charge(&mut tx, u, "submit_survey_response")
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

async fn outbox(pool: &PgPool) -> Vec<(Uuid, i32, Option<String>)> {
    sqlx::query_as("SELECT event_id, attempts, last_error FROM usage_outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn make_due(pool: &PgPool) {
    sqlx::query("UPDATE usage_outbox SET next_attempt_at = now()")
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn ship_once_ships_deletes_and_handles_duplicates_and_rejections(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();
    let (a, b) = (org(), org());
    charge_n(&db, &meter, a, 2).await;
    charge_n(&db, &meter, b, 1).await;

    // Every org's rows go in one pass, and are gone once answered.
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 3);
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 0);
    assert!(outbox(&pool).await.is_empty());
    {
        let p = stub.lock().unwrap();
        assert_eq!(p.received.len(), 3);
        let orgs: HashSet<Uuid> = p.received.iter().map(|e| e.org_id).collect();
        assert_eq!(orgs, HashSet::from([a.as_uuid(), b.as_uuid()]));
        assert!(p
            .received
            .iter()
            .all(|e| e.billable && e.tool == "submit_survey_response"));
    }

    // A platform outage keeps every row, counts the attempt, and backs off.
    charge_n(&db, &meter, a, 2).await;
    stub.lock().unwrap().usage_down = true;
    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    assert!(matches!(err, ShipError::Platform(_)), "{err}");
    let rows = outbox(&pool).await;
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.1 == 1 && r.2.is_some()), "{rows:?}");
    assert_eq!(
        usage::ship_once(&db, &platform, &cfg).await.unwrap(),
        0,
        "backed off"
    );

    // One of them already reached the platform (its answer was lost): a
    // duplicate. The other it refuses: moved aside with its reason.
    let (dup, refused) = (rows[0].0, rows[1].0);
    {
        let mut p = stub.lock().unwrap();
        p.usage_down = false;
        p.seen.insert(dup);
        p.reject.insert(refused);
    }
    make_due(&pool).await;
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 2);
    assert!(outbox(&pool).await.is_empty());
    let rejected: Vec<(Uuid, Uuid, String, String)> =
        sqlx::query_as("SELECT event_id, org_id, tool, reason FROM usage_outbox_rejected")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rejected,
        vec![(
            refused,
            a.as_uuid(),
            "submit_survey_response".to_string(),
            "unknown org".to_string()
        )]
    );

    // A receipt that does not account for every event keeps every row.
    charge_n(&db, &meter, b, 2).await;
    stub.lock().unwrap().undercount = true;
    assert!(usage::ship_once(&db, &platform, &cfg).await.is_err());
    assert_eq!(outbox(&pool).await.len(), 2);
    // Fixed, the resend is accepted (the platform dedupes on event_id).
    stub.lock().unwrap().undercount = false;
    make_due(&pool).await;
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 2);
    assert!(outbox(&pool).await.is_empty());
}

/// The shipper runs unpinned across every org through `maintenance`; as
/// `otto_app`, RLS applies to it, so this proves its policies admit it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn ship_once_drains_every_org_as_otto_app(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let (a, b) = (org(), org());
    charge_n(&db, &meter, a, 2).await;
    charge_n(&db, &meter, b, 1).await;
    let refused = outbox(&pool).await[0].0;
    stub.lock().unwrap().reject.insert(refused);

    let app_db = otto_app_db(&pool).await;
    let role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(app_db.pool())
        .await
        .unwrap();
    assert_eq!(role, "otto_app");
    let cfg = ShipperConfig::default();

    // A failure reschedules as otto_app too.
    stub.lock().unwrap().usage_down = true;
    assert!(usage::ship_once(&app_db, &platform, &cfg).await.is_err());
    let rows = outbox(&pool).await;
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.1 == 1 && r.2.is_some()), "{rows:?}");

    stub.lock().unwrap().usage_down = false;
    make_due(&pool).await;
    assert_eq!(usage::ship_once(&app_db, &platform, &cfg).await.unwrap(), 3);
    app_db.pool().close().await;

    assert!(outbox(&pool).await.is_empty());
    assert_eq!(count(&pool, "usage_outbox_rejected", a).await, 1);
    let orgs: HashSet<Uuid> = stub
        .lock()
        .unwrap()
        .received
        .iter()
        .map(|e| e.org_id)
        .collect();
    assert_eq!(orgs, HashSet::from([a.as_uuid(), b.as_uuid()]));
}

/// `run` ships on its first pass, then sleeps for `poll`; flipping the
/// shutdown channel makes it flush once more and return. The poll is an hour,
/// so only that final flush can ship rows charged after the first pass.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn run_ships_then_flushes_on_shutdown(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let o = org();
    charge_n(&db, &meter, o, 1).await;

    let cfg = ShipperConfig {
        poll: Duration::from_secs(3600),
        ..ShipperConfig::default()
    };
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let shipper = tokio::spawn(usage::run(db.clone(), platform.clone(), cfg, shutdown));

    // The first pass: shipped, answered, and deleted.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !outbox(&pool).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the first pass ships");
    assert_eq!(stub.lock().unwrap().received.len(), 1);

    // Charged while `run` sleeps: still waiting in the outbox.
    charge_n(&db, &meter, o, 2).await;
    assert_eq!(outbox(&pool).await.len(), 2);

    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(10), shipper)
        .await
        .expect("run returns on shutdown")
        .unwrap();
    assert!(
        outbox(&pool).await.is_empty(),
        "the final flush shipped them"
    );
    assert_eq!(stub.lock().unwrap().received.len(), 3);
}

// ---------------------------------------------------------------------- auth

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn authenticate_resolves_a_live_token_and_says_why_it_will_not(pool: PgPool) {
    let db = db(pool);
    let (platform, stub) = start_platform().await;
    let (o, u) = (org(), user());

    // Missing: no header, another scheme, an empty bearer.
    for header in [None, Some("Basic dXNlcjpwdw=="), Some("Bearer  ")] {
        assert_eq!(
            auth::authenticate(&db, &platform, header).await,
            Err(AuthFailure::Missing),
            "{header:?}"
        );
    }

    // Success, with the scheme in any case.
    let good = token(&stub, o, u, &["cxp:read", "cxp:write"], RESOURCE);
    let p = auth::authenticate(&db, &platform, Some(&format!("bearer {good}")))
        .await
        .unwrap();
    assert_eq!((p.org_id, p.user_id), (o, u));
    assert_eq!(p.role, Role::Member);
    assert_eq!(p.kind, TokenKind::Oauth);
    assert_eq!(p.client_id.as_deref(), Some("test-client"));
    assert!(p.require_read().is_ok() && p.require_write().is_ok());

    let read_only = token(&stub, o, u, &["cxp:read"], RESOURCE);
    let p = auth::authenticate(&db, &platform, Some(&format!("Bearer {read_only}")))
        .await
        .unwrap();
    assert!(p.require_read().is_ok());
    assert!(p.require_write().is_err());

    // Inactive: a token the platform does not know, and one that is not
    // even token-shaped.
    let unknown = format!("otto_at_{}", "b".repeat(43));
    for t in [unknown.as_str(), "not-a-token"] {
        assert_eq!(
            auth::authenticate(&db, &platform, Some(&format!("Bearer {t}"))).await,
            Err(AuthFailure::Inactive),
            "{t}"
        );
    }

    // Wrong audience: active, but minted for another resource server.
    let elsewhere = token(&stub, o, u, &["cxp:read", "cxp:write"], OTHER_RESOURCE);
    assert_eq!(
        auth::authenticate(&db, &platform, Some(&format!("Bearer {elsewhere}"))).await,
        Err(AuthFailure::Inactive)
    );

    // Revoked: a removed member, then a deleted org, while the platform (and
    // its client's cache) still vouch for the token.
    let (o2, leaver) = (org(), user());
    let leaver_token = token(&stub, o2, leaver, &["cxp:read"], RESOURCE);
    let header = format!("Bearer {leaver_token}");
    assert!(auth::authenticate(&db, &platform, Some(&header))
        .await
        .is_ok());
    platform_events::apply(&db, &member_removed(o2, leaver))
        .await
        .unwrap();
    assert_eq!(
        auth::authenticate(&db, &platform, Some(&header)).await,
        Err(AuthFailure::Revoked)
    );
    platform_events::apply(&db, &org_deleted(o)).await.unwrap();
    assert_eq!(
        auth::authenticate(&db, &platform, Some(&format!("Bearer {good}"))).await,
        Err(AuthFailure::Revoked)
    );

    // Unavailable: the platform errors (a fresh token, so no cached answer)...
    let (o3, u3) = (org(), user());
    let fresh = token(&stub, o3, u3, &["cxp:read"], RESOURCE);
    stub.lock().unwrap().introspection_down = true;
    assert_eq!(
        auth::authenticate(&db, &platform, Some(&format!("Bearer {fresh}"))).await,
        Err(AuthFailure::Unavailable)
    );
    stub.lock().unwrap().introspection_down = false;
    // ...or cannot be reached at all.
    let unreachable = client("http://127.0.0.1:1");
    assert_eq!(
        auth::authenticate(&db, &unreachable, Some(&format!("Bearer {fresh}"))).await,
        Err(AuthFailure::Unavailable)
    );
    // ...or the tombstones cannot be read.
    let closed = otto_tenant::Db::from_pool(db.pool().clone());
    closed.pool().close().await;
    assert_eq!(
        auth::authenticate(&closed, &platform, Some(&format!("Bearer {fresh}"))).await,
        Err(AuthFailure::Unavailable)
    );
}
