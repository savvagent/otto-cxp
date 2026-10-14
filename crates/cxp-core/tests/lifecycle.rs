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
    /// Answer every introspection with a 401: this service's credential is
    /// refused.
    introspection_rejects: bool,
    /// Event ids already counted, so a resend is a duplicate.
    seen: HashSet<Uuid>,
    /// Event ids refused outright.
    reject: HashSet<Uuid>,
    /// Answer every usage batch with a 503.
    usage_down: bool,
    /// Account for one event fewer than was sent.
    undercount: bool,
    /// Answer every usage batch with this status instead.
    ship_status: Option<StatusCode>,
    /// Refuse, whole, any usage batch containing one of these event ids,
    /// with `poison_status` (422 if unset).
    poison: HashSet<Uuid>,
    poison_status: Option<StatusCode>,
    /// The size of every usage batch received, in order.
    batch_sizes: Vec<usize>,
    /// Answer with a receipt whose counts add up but whose rejected ids do
    /// not match the batch.
    bogus: Option<Bogus>,
    /// Hold every usage batch this long before answering.
    ship_delay: Option<Duration>,
    /// Every usage event received, in order.
    received: Vec<UsageEvent>,
    /// Billable count and hard stop reported by usage-status.
    used: i64,
    hard_stop: bool,
}

type Shared = Arc<Mutex<Platform>>;

#[derive(Clone, Copy)]
enum Bogus {
    /// One event is "rejected" under an id that was not shipped.
    Foreign,
    /// The same shipped id is rejected twice.
    Repeated,
}

async fn introspect(
    State(p): State<Shared>,
    Form(f): Form<HashMap<String, String>>,
) -> Result<Json<IntrospectionResponse>, StatusCode> {
    let p = p.lock().unwrap();
    if p.introspection_down {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if p.introspection_rejects {
        return Err(StatusCode::UNAUTHORIZED);
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
    let delay = p.lock().unwrap().ship_delay;
    if let Some(delay) = delay {
        tokio::time::sleep(delay).await;
    }
    let mut p = p.lock().unwrap();
    if p.usage_down {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    p.batch_sizes.push(b.events.len());
    if b.events.iter().any(|e| p.poison.contains(&e.event_id)) {
        return Err(p.poison_status.unwrap_or(StatusCode::UNPROCESSABLE_ENTITY));
    }
    if let Some(status) = p.ship_status {
        p.received.extend(b.events);
        return Err(status);
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
    if let (Some(bogus), Some(first)) = (p.bogus, b.events.first()) {
        let event_id = match bogus {
            Bogus::Foreign => Uuid::new_v4(),
            Bogus::Repeated => first.event_id,
        };
        receipt.accepted = b.events.len() as u32 - 2;
        receipt.duplicates = 0;
        receipt.rejected = vec![
            RejectedEvent {
                event_id: first.event_id,
                reason: "bogus".into(),
            },
            RejectedEvent {
                event_id,
                reason: "bogus".into(),
            },
        ];
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
    // A rejected usage event is kept: it is the only record of usage the
    // platform never counted.
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

/// A purge larger than one batch runs as several short transactions and
/// still deletes every row, counts each where it lived, and is idempotent.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn org_deleted_purges_more_than_one_batch(pool: PgPool) {
    let db = db(pool.clone());
    let (doomed, other) = (org(), org());
    let team = Uuid::new_v4();
    seed(&db, doomed, team).await;
    seed(&db, other, team).await;
    let extra = 2 * platform_events::PURGE_BATCH + 7;
    sqlx::query(
        "INSERT INTO interactions (org_id, customer_id, interaction_type) \
         SELECT org_id, id, 'page_view' FROM customers, generate_series(1, $2) \
         WHERE org_id = $1",
    )
    .bind(doomed)
    .bind(extra)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(count(&pool, "interactions", doomed).await, extra + 1);

    let delivery = org_deleted(doomed);
    let Outcome::Applied { detail } = platform_events::apply(&db, &delivery).await.unwrap() else {
        panic!("the first delivery applies");
    };
    assert_eq!(detail["interactions"], extra + 1, "{detail}");
    for table in TENANT_TABLES {
        assert_eq!(count(&pool, table, doomed).await, 0, "{table}");
        assert_eq!(count(&pool, table, other).await, 1, "{table}");
        if table != "interactions" {
            assert_eq!(detail[table], 1, "{table}: {detail}");
        }
    }
    assert_eq!(count(&pool, "platform_events", doomed).await, 1);
    assert_eq!(
        platform_events::apply(&db, &delivery).await.unwrap(),
        Outcome::Duplicate
    );
}

/// The marker is written only after the last batch: a purge interrupted
/// part-way (here, simulated by rows left behind with no marker) is applied
/// in full by the redelivery, not acknowledged as a duplicate.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_interrupted_purge_is_finished_by_the_redelivery(pool: PgPool) {
    let db = db(pool.clone());
    let doomed = org();
    seed(&db, doomed, Uuid::new_v4()).await;
    // What a first attempt that crashed after its first batch leaves: the
    // tombstone, the leading (child) tables emptied, and no marker.
    sqlx::query("INSERT INTO deleted_orgs (org_id) VALUES ($1)")
        .bind(doomed)
        .execute(&pool)
        .await
        .unwrap();
    for table in &PURGED_TABLES[..3] {
        sqlx::query(&format!("DELETE FROM {table} WHERE org_id = $1"))
            .bind(doomed)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert!(matches!(
        platform_events::apply(&db, &org_deleted(doomed))
            .await
            .unwrap(),
        Outcome::Applied { .. }
    ));
    for table in TENANT_TABLES {
        assert_eq!(count(&pool, table, doomed).await, 0, "{table}");
    }
}

/// A request transaction already in flight (it holds the org's lifecycle
/// lock shared) finishes before the purge runs, and what it wrote is purged
/// with the rest; afterwards the org is refused.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn org_deleted_waits_for_an_in_flight_request_and_purges_its_writes(pool: PgPool) {
    let db = db(pool.clone());
    let o = org();
    let mut tx = begin_live(&db, o, None).await.unwrap();
    sqlx::query("INSERT INTO customers (org_id, email) VALUES ($1, 'late@example.com')")
        .bind(o)
        .execute(tx.conn())
        .await
        .unwrap();

    let purge = {
        let db = db.clone();
        tokio::spawn(async move { platform_events::apply(&db, &org_deleted(o)).await })
    };
    let mut purge = std::pin::pin!(purge);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut purge)
            .await
            .is_err(),
        "the purge ran while a request held the org's lifecycle lock"
    );
    tx.commit().await.unwrap();
    let outcome = purge.await.unwrap().unwrap();
    assert!(matches!(outcome, Outcome::Applied { .. }), "{outcome:?}");

    assert_eq!(count(&pool, "customers", o).await, 0);
    let err = begin_live(&db, o, None)
        .await
        .err()
        .expect("must be refused");
    assert!(matches!(err, Error::AccessRevoked), "{err:?}");
}

/// Likewise for team.deleted: a team id an in-flight request writes is
/// nulled once that request commits.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn team_deleted_waits_for_an_in_flight_request_and_nulls_its_team(pool: PgPool) {
    let db = db(pool.clone());
    let (o, team) = (org(), Uuid::new_v4());
    let mut tx = begin_live(&db, o, None).await.unwrap();
    let exp: Uuid = sqlx::query_scalar(
        "INSERT INTO experiences (org_id, name, team_id) VALUES ($1, 'Late', $2) RETURNING id",
    )
    .bind(o)
    .bind(team)
    .fetch_one(tx.conn())
    .await
    .unwrap();

    let nulling = {
        let db = db.clone();
        tokio::spawn(async move { platform_events::apply(&db, &team_deleted(o, team)).await })
    };
    let mut nulling = std::pin::pin!(nulling);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut nulling)
            .await
            .is_err(),
        "team.deleted ran while a request held the org's lifecycle lock"
    );
    tx.commit().await.unwrap();
    let outcome = nulling.await.unwrap().unwrap();
    assert!(matches!(outcome, Outcome::Applied { .. }), "{outcome:?}");

    let team_id: Option<Uuid> = sqlx::query_scalar("SELECT team_id FROM experiences WHERE id = $1")
        .bind(exp)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(team_id, None);
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

/// The sweep forgets refused usage after REJECTED_USAGE_KEEP_DAYS, across
/// orgs, as otto_app under the maintenance marker; newer rows stay.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_sweep_forgets_old_rejected_usage(pool: PgPool) {
    let (a, b) = (org(), org());
    let keep = platform_events::REJECTED_USAGE_KEEP_DAYS;
    for o in [a, b] {
        for days in [keep + 1, keep - 1] {
            sqlx::query(
                "INSERT INTO usage_outbox_rejected \
                   (event_id, org_id, tool, billable, occurred_at, reason, rejected_at) \
                 VALUES (gen_random_uuid(), $1, 'submit_feedback', true, now(), 'test', \
                         now() - make_interval(days => $2))",
            )
            .bind(o)
            .bind(days)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    let app_db = otto_app_db(&pool).await;
    assert_eq!(platform_events::sweep(&app_db, 30).await.unwrap(), 2);
    app_db.pool().close().await;
    for o in [a, b] {
        assert_eq!(count(&pool, "usage_outbox_rejected", o).await, 1);
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
        // Metering a free tool is the server's bug, not the agent's request.
        let err = meter.charge(&mut tx, u, free).await.unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "{free}: {err:?}");
        assert_eq!(err.code(), "internal", "{free}: {err}");
        assert!(err.is_internal() && !err.retriable(), "{free}");
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
        .unwrap()
        .expect("the final flush succeeds");
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

/// A platform that refuses this service's own credential cannot be fixed by
/// the caller retrying: a 500 (`internal`), not the retry-shortly 503.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn authenticate_is_internal_when_the_platform_refuses_this_services_credential(pool: PgPool) {
    let db = db(pool);
    let (platform, stub) = start_platform().await;
    let t = token(&stub, org(), user(), &["cxp:read"], RESOURCE);
    stub.lock().unwrap().introspection_rejects = true;
    let failure = auth::authenticate(&db, &platform, Some(&format!("Bearer {t}")))
        .await
        .unwrap_err();
    assert_eq!(failure, AuthFailure::Internal);
    let refusal = auth::refusal("https://cxp.example.com", &failure);
    assert_eq!(refusal.status, 500);
    assert_eq!(refusal.body["error"], "internal");
    assert!(refusal.headers.is_empty(), "{:?}", refusal.headers);
}

// ------------------------------------------------------ usage: dead letters

async fn rejected_rows(pool: &PgPool) -> Vec<(Uuid, String)> {
    sqlx::query_as("SELECT event_id, reason FROM usage_outbox_rejected ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn row_failures(pool: &PgPool) -> Vec<i32> {
    sqlx::query_scalar("SELECT row_failures FROM usage_outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Each outbox row's event id, row failures, and last error, in id order.
async fn outbox_failures(pool: &PgPool) -> Vec<(Uuid, i32, Option<String>)> {
    sqlx::query_as("SELECT event_id, row_failures, last_error FROM usage_outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// A refusal that is about this side or passes on its own (the credential,
/// an unknown URL, a timeout, rate limiting, an outage) keeps every row and
/// never counts against it. Nor does a 4xx the platform answers every
/// request with: split down to single events and every one still refused,
/// with none accepted, it is about the requests (a contract change, a
/// gateway refusing everything), so the batch is kept whole and uncounted.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_refusal_of_every_event_or_of_the_credential_never_counts_against_the_rows(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();
    charge_n(&db, &meter, org(), 2).await;

    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::BAD_GATEWAY,
    ] {
        stub.lock().unwrap().ship_status = Some(status);
        make_due(&pool).await;
        assert!(usage::ship_once(&db, &platform, &cfg).await.is_err());
        assert_eq!(
            outbox(&pool).await.len(),
            2,
            "{status}: kept for after the fix"
        );
        assert_eq!(
            row_failures(&pool).await,
            [0, 0],
            "{status}: not the rows' fault"
        );
        assert!(rejected_rows(&pool).await.is_empty(), "{status}");
    }

    charge_n(&db, &meter, org(), 1).await;
    {
        let mut p = stub.lock().unwrap();
        p.ship_status = Some(StatusCode::UNPROCESSABLE_ENTITY);
        p.batch_sizes.clear();
    }
    for pass in 0..usage::MAX_ROW_FAILURES + 2 {
        make_due(&pool).await;
        let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
        assert!(matches!(err, ShipError::RefusedAll(_)), "{pass}: {err}");
        assert!(!err.is_retriable(), "needs an operator");
        let rows = outbox_failures(&pool).await;
        assert_eq!(rows.len(), 3, "{pass}: every row kept");
        assert!(
            rows.iter()
                .all(|r| r.1 == 0 && r.2.as_deref().is_some_and(|e| e.contains("422"))),
            "{pass}: {rows:?}"
        );
        assert!(rejected_rows(&pool).await.is_empty(), "{pass}");
    }
    // Split to the bottom each pass: 3, then 1 and 2, then 1 and 1.
    assert_eq!(stub.lock().unwrap().batch_sizes[..5], [3, 1, 2, 1, 1]);

    // Once the platform takes them again, they ship.
    stub.lock().unwrap().ship_status = None;
    make_due(&pool).await;
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 3);
    assert!(outbox(&pool).await.is_empty());
    assert!(rejected_rows(&pool).await.is_empty());
}

/// One event the platform refuses must not take its batch down with it: a
/// batch refused whole (422, or 413 for its size) is resent in halves until
/// the refused event stands alone. Every other event ships; the refused one
/// stays, the refusal counted against it, and moves to
/// `usage_outbox_rejected` only after MAX_ROW_FAILURES of them.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_poisoned_batch_is_split_and_only_the_poisoned_event_is_given_up_on(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();

    for status in [
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::PAYLOAD_TOO_LARGE,
    ] {
        charge_n(&db, &meter, org(), 7).await;
        let rows = outbox(&pool).await;
        let poisoned = rows[4].0;
        {
            let mut p = stub.lock().unwrap();
            p.poison = HashSet::from([poisoned]);
            p.poison_status = Some(status);
            p.received.clear();
            p.batch_sizes.clear();
        }

        let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
        assert!(err.faults_the_rows(), "{status}: {err}");
        let left = outbox_failures(&pool).await;
        assert_eq!(left.len(), 1, "{status}: {left:?}");
        assert_eq!((left[0].0, left[0].1), (poisoned, 1), "{status}");
        assert!(rejected_rows(&pool).await.len() < 2, "{status}");
        {
            let p = stub.lock().unwrap();
            let accepted: HashSet<Uuid> = p.received.iter().map(|e| e.event_id).collect();
            let expected: HashSet<Uuid> = rows
                .iter()
                .map(|r| r.0)
                .filter(|id| *id != poisoned)
                .collect();
            assert_eq!(accepted, expected, "{status}: every other event shipped");
            // Depth first, in order: 7 is refused; its first 3 ship; its last
            // 4 are refused; their first 2 (holding the poisoned 5th) are
            // refused, so each goes alone (one ships, one is refused); the
            // last 2 ship.
            assert_eq!(p.batch_sizes, [7, 3, 4, 2, 1, 1, 2], "{status}");
        }

        // Alone in later passes, it is still refused, and each refusal
        // counts until it is given up on.
        for pass in 2..usage::MAX_ROW_FAILURES {
            make_due(&pool).await;
            assert!(usage::ship_once(&db, &platform, &cfg).await.is_err());
            assert_eq!(row_failures(&pool).await, [pass], "{status}");
        }
        make_due(&pool).await;
        assert!(usage::ship_once(&db, &platform, &cfg).await.is_err());
        assert!(outbox(&pool).await.is_empty(), "{status}");
        let rejected = rejected_rows(&pool).await;
        let last = rejected.last().unwrap();
        assert_eq!(last.0, poisoned, "{status}");
        assert!(
            last.1.contains(&format!(
                "given up after {} failures about the row itself",
                usage::MAX_ROW_FAILURES
            )) && last.1.contains(status.as_str()),
            "{status}: {}",
            last.1
        );
    }
    assert_eq!(rejected_rows(&pool).await.len(), 2);
}

/// Splitting stops at the pass's request budget: the pieces not yet sent are
/// kept for the next pass, not counted against their rows, and the next
/// pass picks up where it left off.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn splitting_stops_at_the_request_budget_and_keeps_the_rest(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig {
        split_budget: 3,
        ..ShipperConfig::default()
    };
    charge_n(&db, &meter, org(), 7).await;
    let rows = outbox(&pool).await;
    stub.lock().unwrap().poison = HashSet::from([rows[4].0]);

    // 7 is refused, its first 3 ship, its last 4 are refused: the budget is
    // spent, and those 4 are kept.
    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    assert!(matches!(err, ShipError::Unfinished(_)), "{err}");
    assert!(err.is_retriable() && !err.faults_the_rows());
    assert!(err.to_string().contains("3 of 7 events answered"), "{err}");
    assert_eq!(stub.lock().unwrap().batch_sizes, [7, 3, 4]);
    let left = outbox_failures(&pool).await;
    assert_eq!(
        left.iter().map(|r| r.0).collect::<Vec<_>>(),
        rows[3..].iter().map(|r| r.0).collect::<Vec<_>>()
    );
    assert!(left.iter().all(|r| r.1 == 0), "{left:?}");
    assert!(rejected_rows(&pool).await.is_empty());

    // The next pass picks up there: 4 refused, its first 2 (holding the
    // poisoned event) refused, the first of those shipped alone, and the
    // budget is spent again with the rest kept.
    make_due(&pool).await;
    stub.lock().unwrap().batch_sizes.clear();
    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    assert!(matches!(err, ShipError::Unfinished(_)), "{err}");
    assert_eq!(stub.lock().unwrap().batch_sizes, [4, 2, 1]);
    assert_eq!(
        outbox(&pool).await.iter().map(|r| r.0).collect::<Vec<_>>(),
        rows[4..].iter().map(|r| r.0).collect::<Vec<_>>()
    );
    assert_eq!(row_failures(&pool).await, [0, 0, 0]);
}

/// Splitting also stops once half the claim lease has gone, so a slow
/// platform cannot keep the claimed rows past their lease.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn splitting_stops_at_half_the_claim_lease(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig {
        claim_lease: Duration::from_millis(200),
        ..ShipperConfig::default()
    };
    charge_n(&db, &meter, org(), 4).await;
    let poisoned = outbox(&pool).await[0].0;
    {
        let mut p = stub.lock().unwrap();
        p.poison = HashSet::from([poisoned]);
        p.ship_delay = Some(Duration::from_millis(150));
    }

    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    assert!(matches!(err, ShipError::Unfinished(_)), "{err}");
    assert!(err.to_string().contains("0 of 4 events answered"), "{err}");
    assert_eq!(
        stub.lock().unwrap().batch_sizes,
        [4],
        "one request, then out of time"
    );
    assert_eq!(row_failures(&pool).await, [0, 0, 0, 0]);
    assert!(rejected_rows(&pool).await.is_empty());
}

/// When settling the answered rows fails, the pieces that failed for their
/// own reasons are still rescheduled with those reasons: a lone refusal
/// still counts against its row.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_failed_settle_still_reschedules_the_failed_pieces(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();
    charge_n(&db, &meter, org(), 3).await;
    let rows = outbox(&pool).await;
    let (gone, kept, poisoned) = (rows[0].0, rows[1].0, rows[2].0);
    {
        let mut p = stub.lock().unwrap();
        // [a, b, c] is refused; [a] comes back with a receipt refusing a;
        // [b, c] is refused; b ships; c is refused alone.
        p.poison = HashSet::from([poisoned]);
        p.reject = HashSet::from([gone]);
        p.ship_delay = Some(Duration::from_millis(100));
    }
    // While the rest is in flight, the refused row disappears from under the
    // claim, so moving it aside cannot match it and settling rolls back.
    let watcher = {
        let (stub, pool) = (stub.clone(), pool.clone());
        tokio::spawn(async move {
            while !stub
                .lock()
                .unwrap()
                .received
                .iter()
                .any(|e| e.event_id == gone)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            sqlx::query("DELETE FROM usage_outbox WHERE event_id = $1")
                .bind(gone)
                .execute(&pool)
                .await
                .unwrap();
        })
    };

    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    watcher.await.unwrap();
    assert!(matches!(err, ShipError::Claim(_)), "{err}");
    assert_eq!(stub.lock().unwrap().batch_sizes, [3, 1, 2, 1, 1]);
    let left = outbox_failures(&pool).await;
    assert_eq!(left.len(), 2, "{left:?}");
    assert_eq!(left[0].0, kept);
    assert_eq!(left[0].1, 0, "not the row's fault");
    assert!(left[0]
        .2
        .as_deref()
        .unwrap()
        .contains("changed under a claim"));
    assert_eq!(left[1].0, poisoned);
    assert_eq!(left[1].1, 1, "its own refusal counted");
    assert!(left[1].2.as_deref().unwrap().contains("422"), "{left:?}");
    assert!(rejected_rows(&pool).await.is_empty());
}

/// However long the platform stays unreachable, the rows stay in the outbox
/// (and keep counting toward their period's quota): an outage is not the
/// rows' fault.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_unreachable_platform_never_gives_usage_up(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, _stub) = start_platform().await;
    let meter = Meter::new(platform, false, "https://x");
    let cfg = ShipperConfig::default();
    charge_n(&db, &meter, org(), 2).await;

    // Bound and dropped: nothing listens there.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = client(&format!("http://{}", listener.local_addr().unwrap()));
    drop(listener);

    let failures = usage::MAX_ROW_FAILURES + 5;
    for _ in 0..failures {
        make_due(&pool).await;
        let err = usage::ship_once(&db, &dead, &cfg).await.unwrap_err();
        assert!(
            matches!(err, ShipError::Platform(otto_resource::Error::Transport(_))),
            "{err}"
        );
        assert!(err.is_retriable());
    }
    let rows = outbox(&pool).await;
    assert_eq!(rows.len(), 2, "still waiting to ship");
    assert!(rows.iter().all(|r| r.1 == failures), "{rows:?}");
    assert_eq!(row_failures(&pool).await, [0, 0]);
    assert!(rejected_rows(&pool).await.is_empty());
}

/// A row whose batch keeps coming back with a receipt that does not account
/// for it is given up on after MAX_ROW_FAILURES of those, with the last
/// error as its reason; a row that has merely been tried often is not.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn usage_the_platform_keeps_failing_to_account_for_is_given_up_on(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();
    charge_n(&db, &meter, org(), 2).await;
    stub.lock().unwrap().undercount = true;

    // One row is one failure from giving up; the other has been attempted
    // far more often than that, all through outages that never counted.
    let rows = outbox(&pool).await;
    sqlx::query("UPDATE usage_outbox SET row_failures = $2 WHERE event_id = $1")
        .bind(rows[0].0)
        .bind(usage::MAX_ROW_FAILURES - 1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE usage_outbox SET attempts = $2 WHERE event_id = $1")
        .bind(rows[1].0)
        .bind(usage::MAX_ROW_FAILURES * 3)
        .execute(&pool)
        .await
        .unwrap();
    let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
    assert!(matches!(err, ShipError::Receipt(_)), "{err}");

    let left = outbox(&pool).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].0, rows[1].0);
    assert_eq!(row_failures(&pool).await, [1]);
    let rejected = rejected_rows(&pool).await;
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].0, rows[0].0);
    assert!(
        rejected[0].1.contains(&format!(
            "given up after {} failures about the row itself",
            usage::MAX_ROW_FAILURES
        )) && rejected[0].1.contains("accounted for"),
        "{}",
        rejected[0].1
    );
}

/// A receipt whose counts add up but whose rejected ids are not the batch's
/// (one not shipped, or one named twice) is not trusted: nothing is deleted
/// or moved, and the whole batch is retried.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_receipt_rejecting_ids_outside_the_batch_keeps_every_row(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    let cfg = ShipperConfig::default();
    charge_n(&db, &meter, org(), 3).await;

    for bogus in [Bogus::Foreign, Bogus::Repeated] {
        stub.lock().unwrap().bogus = Some(bogus);
        make_due(&pool).await;
        let err = usage::ship_once(&db, &platform, &cfg).await.unwrap_err();
        assert!(matches!(err, ShipError::Receipt(_)), "{err}");
        let rows = outbox(&pool).await;
        assert_eq!(rows.len(), 3, "every row kept");
        assert!(rows.iter().all(|r| r.2.is_some()), "rescheduled: {rows:?}");
        assert!(rejected_rows(&pool).await.is_empty(), "nothing moved");
    }

    stub.lock().unwrap().bogus = None;
    make_due(&pool).await;
    assert_eq!(usage::ship_once(&db, &platform, &cfg).await.unwrap(), 3);
    assert!(outbox(&pool).await.is_empty());
}

/// Two shippers racing over the same outbox (two replicas) never send the
/// same row twice: the claim takes rows with SKIP LOCKED and leases them
/// before the platform call.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn concurrent_shippers_deliver_each_event_once(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let meter = Meter::new(platform.clone(), false, "https://x");
    for _ in 0..4 {
        charge_n(&db, &meter, org(), 5).await;
    }
    stub.lock().unwrap().ship_delay = Some(Duration::from_millis(200));
    // Small batches, so both shippers have rows to claim at once.
    let cfg = ShipperConfig {
        batch: 7,
        ..ShipperConfig::default()
    };

    let drain = || async {
        let mut total = 0;
        loop {
            match usage::ship_once(&db, &platform, &cfg).await.unwrap() {
                0 => return total,
                n => total += n,
            }
        }
    };
    let (a, b) = tokio::join!(drain(), drain());
    assert_eq!(a + b, 20);
    assert!(a > 0 && b > 0, "both shipped ({a}, {b})");
    assert!(outbox(&pool).await.is_empty());

    let mut deliveries: HashMap<Uuid, usize> = HashMap::new();
    for e in &stub.lock().unwrap().received {
        *deliveries.entry(e.event_id).or_default() += 1;
    }
    assert_eq!(deliveries.len(), 20);
    assert!(
        deliveries.values().all(|n| *n == 1),
        "an event was delivered more than once: {deliveries:?}"
    );
}

/// The quota counts unshipped rows from the platform's current period only:
/// one left over from an earlier period is not this period's usage.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_enforcing_meter_counts_only_this_periods_unshipped_usage(pool: PgPool) {
    let db = db(pool.clone());
    let (platform, stub) = start_platform().await;
    let (o, u) = (org(), user());
    {
        let mut p = stub.lock().unwrap();
        p.used = 9;
        p.hard_stop = true;
    }
    // Left over from before the period the stub reports (it starts today).
    sqlx::query(
        "INSERT INTO usage_outbox (org_id, tool, billable, occurred_at) \
         VALUES ($1, 'submit_feedback', true, now() - interval '2 days')",
    )
    .bind(o)
    .execute(&pool)
    .await
    .unwrap();

    let meter = Meter::new(platform, true, "https://x");
    // 9 counted + the old row would be 10 already; it does not count.
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    meter.charge(&mut tx, u, "create_survey").await.unwrap();
    tx.commit().await.unwrap();
    // This period's unshipped row does.
    let mut tx = begin_live(&db, o, Some(u)).await.unwrap();
    let err = meter.charge(&mut tx, u, "create_survey").await.unwrap_err();
    assert_eq!(err.code(), "quota_exceeded", "{err}");
}

/// A removal is per org: the same user in another org keeps working.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn member_removed_in_one_org_leaves_the_user_working_in_another(pool: PgPool) {
    let db = db(pool);
    let (a, b, u) = (org(), org(), user());
    platform_events::apply(&db, &member_removed(a, u))
        .await
        .unwrap();
    let refused = begin_live(&db, a, Some(u))
        .await
        .err()
        .expect("refused in org A");
    assert_eq!(refused.code(), "access_revoked");
    let tx = begin_live(&db, b, Some(u))
        .await
        .expect("the user is still a member of org B");
    tx.commit().await.unwrap();
    assert!(!platform_events::revoked(&db, b, u).await.unwrap());
}
