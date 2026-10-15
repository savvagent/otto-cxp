//! The baseline schema against a real Postgres: tenant isolation on every
//! tenant table, composite foreign keys, and the migrator's rollback
//! tolerance. Each test gets a fresh, migrated database from `#[sqlx::test]`;
//! set `DATABASE_URL` to a superuser on a scratch cluster (`podman compose up
//! -d` gives you one on port 15435).

mod common;

use common::{as_app_maintenance, as_app_unpinned, db, org, otto_app_db, sqlstate};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use sqlx::migrate::MigrateError;
use sqlx::PgPool;
use uuid::Uuid;

/// Every domain table. Each must be org-scoped and isolated.
const DOMAIN_TABLES: [&str; 15] = [
    "customers",
    "tickets",
    "interactions",
    "feedback",
    "surveys",
    "survey_responses",
    "chat_sessions",
    "chat_messages",
    "knowledge_articles",
    "segments",
    "segment_members",
    "experiences",
    "applications",
    "pages",
    "components",
];

/// The search index tables (migration 0004): org-scoped and isolated like
/// the domain tables, but derived from them by triggers, many rows per
/// customer or article.
const SEARCH_INDEX_TABLES: [&str; 2] = ["customer_search_grams", "knowledge_search_terms"];

/// Bookkeeping tables carrying a `<table>_tenant_isolation` policy.
const BOOKKEEPING_TABLES: [&str; 4] = [
    "platform_events",
    "usage_outbox",
    "usage_outbox_rejected",
    "console_sessions",
];

/// Ids of one row in every tenant table, all in one org.
struct Seeded {
    experience: Uuid,
    application: Uuid,
    page: Uuid,
    component: Uuid,
    customer: Uuid,
    segment: Uuid,
    session: Uuid,
    survey: Uuid,
    article: Uuid,
}

/// Insert one row into every tenant table for `org`, as `otto_app` pinned to
/// it, and commit.
async fn seed(db: &Db, org: OrgId) -> Seeded {
    let s = Seeded {
        experience: Uuid::new_v4(),
        application: Uuid::new_v4(),
        page: Uuid::new_v4(),
        component: Uuid::new_v4(),
        customer: Uuid::new_v4(),
        segment: Uuid::new_v4(),
        session: Uuid::new_v4(),
        survey: Uuid::new_v4(),
        article: Uuid::new_v4(),
    };
    let email = format!("{}@example.com", Uuid::new_v4());
    let catalog = "$3, $4, $5, $6";
    let statements = [
        "INSERT INTO experiences (id, org_id, name) VALUES ($3, $1, 'Checkout')".to_string(),
        "INSERT INTO applications (id, org_id, experience_id, name) VALUES ($4, $1, $3, 'Web')"
            .to_string(),
        "INSERT INTO pages (id, org_id, application_id, name) VALUES ($5, $1, $4, 'Cart')"
            .to_string(),
        "INSERT INTO components (id, org_id, page_id, name) VALUES ($6, $1, $5, 'Pay button')"
            .to_string(),
        format!(
            "INSERT INTO customers (id, org_id, email, name, experience_id, application_id, \
             page_id, component_id) VALUES ($2, $1, $10, 'Ada', {catalog})"
        ),
        "INSERT INTO interactions (org_id, customer_id, interaction_type) \
         VALUES ($1, $2, 'login')"
            .to_string(),
        "INSERT INTO segments (id, org_id, name, criteria) VALUES ($7, $1, 'VIP', '{}')"
            .to_string(),
        "INSERT INTO segment_members (org_id, segment_id, customer_id) VALUES ($1, $7, $2)"
            .to_string(),
        format!(
            "INSERT INTO tickets (org_id, customer_id, subject, priority, experience_id, \
             application_id, page_id, component_id) \
             VALUES ($1, $2, 'Card declined', 'high'::ticket_priority, {catalog})"
        ),
        // Published, so the search index holds it (it holds published
        // articles only).
        "INSERT INTO knowledge_articles (id, org_id, title, content, published) \
         VALUES ($11, $1, 'Refunds', 'How refunds work', true)"
            .to_string(),
        "INSERT INTO chat_sessions (id, org_id, customer_id) VALUES ($8, $1, $2)".to_string(),
        "INSERT INTO chat_messages (org_id, session_id, role, content) \
         VALUES ($1, $8, 'customer'::message_role, 'hello')"
            .to_string(),
        format!(
            "INSERT INTO feedback (org_id, customer_id, feedback_type, rating, experience_id, \
             application_id, page_id, component_id) \
             VALUES ($1, $2, 'praise'::feedback_type, 5, {catalog})"
        ),
        format!(
            "INSERT INTO surveys (id, org_id, title, questions, experience_id, application_id, \
             page_id, component_id) VALUES ($9, $1, 'NPS', '[]', {catalog})"
        ),
        "INSERT INTO survey_responses (org_id, survey_id, customer_id, answers) \
         VALUES ($1, $9, $2, '{}')"
            .to_string(),
        "INSERT INTO platform_events (event_id, org_id, kind) \
         VALUES (gen_random_uuid(), $1, 'member.removed')"
            .to_string(),
        "INSERT INTO usage_outbox (org_id, tool, billable) VALUES ($1, 'submit_feedback', true)"
            .to_string(),
        "INSERT INTO usage_outbox_rejected (event_id, org_id, tool, billable, occurred_at, reason) \
         VALUES (gen_random_uuid(), $1, 'submit_feedback', true, now(), 'test')"
            .to_string(),
        "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
         refresh_token_enc, access_expires_at, expires_at) \
         VALUES (sha256(gen_random_uuid()::text::bytea), $1, gen_random_uuid(), '\\x00', \
         '\\x00', now(), now() + interval '1 day')"
            .to_string(),
    ];
    let mut tx = db.begin(org).await.unwrap();
    for sql in &statements {
        // Postgres refuses a bind carrying more parameters than the statement
        // references, so every statement is prefixed to reference all eleven.
        sqlx::query(&with_param_types(sql))
            .bind(org)
            .bind(s.customer)
            .bind(s.experience)
            .bind(s.application)
            .bind(s.page)
            .bind(s.component)
            .bind(s.segment)
            .bind(s.session)
            .bind(s.survey)
            .bind(&email)
            .bind(s.article)
            .execute(tx.conn())
            .await
            .unwrap_or_else(|e| panic!("seeding failed on {sql}: {e}"));
    }
    tx.commit().await.unwrap();
    s
}

/// Prefix a statement with a no-op that references all eleven seed parameters,
/// so each one's type is known even where `sql` does not use it.
fn with_param_types(sql: &str) -> String {
    format!(
        "WITH _p AS (SELECT $1::uuid, $2::uuid, $3::uuid, $4::uuid, $5::uuid, $6::uuid, \
         $7::uuid, $8::uuid, $9::uuid, $10::text, $11::uuid) {sql}"
    )
}

async fn count(db: &Db, org: OrgId, table: &str) -> i64 {
    let mut tx = db.begin(org).await.unwrap();
    let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(tx.conn())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    n
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn tenant_isolation_is_enforced_on_every_tenant_table(pool: PgPool) {
    let db = db(pool);
    let report = db.verify_tenant_isolation().await.expect("isolation");
    let summary = report.summary();
    assert!(summary.contains("otto_app"), "{summary}");

    let mut expected: Vec<&str> = DOMAIN_TABLES
        .iter()
        .chain(SEARCH_INDEX_TABLES.iter())
        .chain(BOOKKEEPING_TABLES.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    for name in &expected {
        let table = report
            .tables
            .iter()
            .find(|t| t.name == *name)
            .unwrap_or_else(|| panic!("{name} is missing from the isolation report"));
        assert!(table.rls_enabled, "{name}: RLS not enabled");
        assert!(table.rls_forced, "{name}: RLS not forced");
    }
    // Exactly these: a new tenant table must be added to this test, and the
    // pre-org tombstone lookups must stay outside RLS.
    let mut reported: Vec<&str> = report.tables.iter().map(|t| t.name.as_str()).collect();
    reported.sort_unstable();
    assert_eq!(reported, expected);
}

/// Behavioral proof per table: another org sees none of an org's rows and
/// cannot update or delete them, and no org can write a row for another.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn another_org_can_neither_see_nor_change_an_orgs_rows(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    seed(&db, a).await;

    for table in DOMAIN_TABLES.iter().chain(BOOKKEEPING_TABLES.iter()) {
        assert_eq!(
            count(&db, a, table).await,
            1,
            "org A sees its own {table} row"
        );
        assert_eq!(count(&db, b, table).await, 0, "org B sees org A's {table}");

        let mut tx = db.begin(b).await.unwrap();
        let updated = sqlx::query(&format!("UPDATE {table} SET org_id = org_id"))
            .execute(tx.conn())
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(updated, 0, "org B updated org A's {table}");
        let deleted = sqlx::query(&format!("DELETE FROM {table}"))
            .execute(tx.conn())
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(deleted, 0, "org B deleted org A's {table}");
        tx.commit().await.unwrap();

        assert_eq!(
            count(&db, a, table).await,
            1,
            "org A's {table} row survives"
        );
    }

    // The search index: org A's customer and article left rows (many each);
    // org B sees, changes, and deletes none of them.
    for table in SEARCH_INDEX_TABLES {
        let theirs = count(&db, a, table).await;
        assert!(theirs > 0, "org A's {table} is empty");
        assert_eq!(count(&db, b, table).await, 0, "org B sees org A's {table}");
        let mut tx = db.begin(b).await.unwrap();
        for sql in [
            format!("UPDATE {table} SET org_id = org_id"),
            format!("DELETE FROM {table}"),
        ] {
            let n = sqlx::query(&sql)
                .execute(tx.conn())
                .await
                .unwrap()
                .rows_affected();
            assert_eq!(n, 0, "org B changed org A's {table}: {sql}");
        }
        tx.commit().await.unwrap();
        assert_eq!(
            count(&db, a, table).await,
            theirs,
            "org A's {table} survives"
        );
    }

    // A pinned transaction cannot write a row for, or move a row to, another
    // org: the policy's WITH CHECK refuses it (42501).
    for (table, insert) in [
        (
            "experiences",
            "INSERT INTO experiences (org_id, name) VALUES ($1, 'x')",
        ),
        (
            "customers",
            "INSERT INTO customers (org_id, email) VALUES ($1, 'x@example.com')",
        ),
        (
            "knowledge_articles",
            "INSERT INTO knowledge_articles (org_id, title, content) VALUES ($1, 't', 'c')",
        ),
        (
            "platform_events",
            "INSERT INTO platform_events (event_id, org_id, kind) \
             VALUES (gen_random_uuid(), $1, 'org.deleted')",
        ),
        (
            "customer_search_grams",
            "INSERT INTO customer_search_grams (org_id, customer_id, gram, name_grams) \
             VALUES ($1, gen_random_uuid(), 'sabc', 1)",
        ),
        (
            "knowledge_search_terms",
            "INSERT INTO knowledge_search_terms (org_id, article_id, term) \
             VALUES ($1, gen_random_uuid(), 'labc')",
        ),
        (
            "usage_outbox",
            "INSERT INTO usage_outbox (org_id, tool, billable) VALUES ($1, 'create_survey', true)",
        ),
    ] {
        let mut tx = db.begin(b).await.unwrap();
        let err = sqlx::query(insert)
            .bind(a)
            .execute(tx.conn())
            .await
            .unwrap_err();
        assert_eq!(sqlstate(&err), "42501", "org B wrote into org A's {table}");
        drop(tx);
    }
    let mut tx = db.begin(a).await.unwrap();
    let err = sqlx::query("UPDATE experiences SET org_id = $1")
        .bind(b)
        .execute(tx.conn())
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err), "42501", "org A moved a row to org B");
    drop(tx);
}

/// A request path that forgot to pin (current_org() NULL) reads and writes
/// nothing in a domain table: as `otto_app` with no org set, no tenant
/// policy matches any row.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn unpinned_otto_app_sees_no_domain_rows(pool: PgPool) {
    let admin = db(pool.clone());
    let a = org();
    seed(&admin, a).await;

    for table in DOMAIN_TABLES.iter().chain(SEARCH_INDEX_TABLES.iter()) {
        let mut tx = as_app_unpinned(&pool).await;
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} is readable unpinned");
        let deleted = sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(deleted, 0, "{table} is deletable unpinned");
        tx.rollback().await.unwrap();
    }

    let mut tx = as_app_unpinned(&pool).await;
    let err = sqlx::query("INSERT INTO experiences (org_id, name) VALUES ($1, 'x')")
        .bind(a)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err), "42501");
}

/// The platform_events retention sweep runs unpinned as otto_app: it needs
/// the maintenance marker to read or delete, and with it, it can.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_platform_events_sweep_needs_the_maintenance_marker(pool: PgPool) {
    let admin = db(pool.clone());
    let a = org();
    seed(&admin, a).await;

    let mut tx = as_app_unpinned(&pool).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0, "readable unpinned without the marker");
    let swept = sqlx::query("DELETE FROM platform_events")
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(swept, 0, "an unmarked unpinned DELETE emptied the log");
    tx.rollback().await.unwrap();

    let mut tx = as_app_maintenance(&pool).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 1, "the marked sweep reads");
    // The sweep's real shape: a WHERE on received_at needs the read policy.
    let swept = sqlx::query(
        "DELETE FROM platform_events WHERE received_at < now() + make_interval(days => 1)",
    )
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(swept, 1, "the marked sweep deletes");
    tx.rollback().await.unwrap();

    // Marked or not, the sweep cannot reach domain tables.
    let mut tx = as_app_maintenance(&pool).await;
    for table in DOMAIN_TABLES.iter().chain(SEARCH_INDEX_TABLES.iter()) {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} is readable under the maintenance marker");
    }
    tx.rollback().await.unwrap();

    // Pinned deletes (the org.deleted purge) need no marker, through a pool
    // whose every connection is otto_app.
    let app_db = otto_app_db(&pool).await;
    let mut tx = app_db.begin(a).await.unwrap();
    let n = sqlx::query("DELETE FROM platform_events WHERE org_id = $1")
        .bind(a)
        .execute(tx.conn())
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1, "a pinned DELETE");
    tx.rollback().await.unwrap();
    app_db.pool().close().await;
}

/// The usage outbox is written pinned and drained unpinned under the
/// maintenance marker: a pinned transaction sees its own org's rows only, a
/// marked unpinned one (the shipper, the sweep) every org's, and an unmarked
/// unpinned one (a request path that forgot to pin) sees, changes, deletes,
/// and writes nothing.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_usage_outbox_is_drained_across_orgs_only_under_the_marker(pool: PgPool) {
    let admin = db(pool.clone());
    let (a, b) = (org(), org());
    seed(&admin, a).await;
    seed(&admin, b).await;

    for table in ["usage_outbox", "usage_outbox_rejected"] {
        assert_eq!(count(&admin, a, table).await, 1);

        let mut tx = as_app_unpinned(&pool).await;
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} is readable unpinned without the marker");
        let changed = sqlx::query(&format!("UPDATE {table} SET tool = tool"))
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(changed, 0, "{table}: an unmarked unpinned UPDATE");
        let deleted = sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(deleted, 0, "{table}: an unmarked unpinned DELETE");
        tx.rollback().await.unwrap();

        let mut tx = as_app_unpinned(&pool).await;
        let err = sqlx::query(&format!(
            "INSERT INTO {table} (event_id, org_id, tool, billable, occurred_at{reason_col}) \
             VALUES (gen_random_uuid(), $1, 'submit_feedback', true, now(){reason_val})",
            reason_col = if table == "usage_outbox" {
                ""
            } else {
                ", reason"
            },
            reason_val = if table == "usage_outbox" { "" } else { ", 'x'" },
        ))
        .bind(a)
        .execute(&mut *tx)
        .await
        .unwrap_err();
        assert_eq!(
            sqlstate(&err),
            "42501",
            "{table}: an unmarked unpinned INSERT"
        );
        tx.rollback().await.unwrap();

        let mut tx = as_app_maintenance(&pool).await;
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 2, "the marked shipper reads {table} across orgs");
        let deleted = sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(deleted, 2, "the marked shipper deletes from {table}");
        tx.rollback().await.unwrap();
    }
}

/// A console session is the one row a request reads before it knows the org:
/// the cookie is how the org is learned. An unpinned transaction (a request
/// path that forgot to pin) still sees, changes, and deletes nothing; the
/// org comes only from `console_session_org`, which answers with the org of
/// the one unexpired row whose hash it is given, and never a token.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_console_session_is_found_before_its_org_only_through_the_lookup_function(pool: PgPool) {
    let admin = db(pool.clone());
    let (a, b) = (org(), org());
    seed(&admin, a).await;
    seed(&admin, b).await;
    let live: Vec<u8> = (0..32).collect();
    let expired: Vec<u8> = (100..132).collect();
    for (hash, expires) in [(&live, "now() + interval '1 day'"), (&expired, "now()")] {
        let mut tx = admin.begin(a).await.unwrap();
        sqlx::query(&format!(
            "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
             refresh_token_enc, access_expires_at, expires_at) \
             VALUES ($1, $2, gen_random_uuid(), '\\x00', '\\x00', now(), {expires})"
        ))
        .bind(hash)
        .bind(a)
        .execute(tx.conn())
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // Unpinned, without the function: nothing, whatever the statement.
    let mut tx = as_app_unpinned(&pool).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0, "console_sessions is readable unpinned");
    for sql in [
        "UPDATE console_sessions SET expires_at = now()",
        "DELETE FROM console_sessions",
    ] {
        let changed = sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(changed, 0, "unpinned: {sql}");
    }
    let err = sqlx::query(
        "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
         refresh_token_enc, access_expires_at, expires_at) \
         VALUES ($1, $2, gen_random_uuid(), '\\x00', '\\x00', now(), now())",
    )
    .bind(vec![7u8; 32])
    .bind(a)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&err), "42501", "unpinned INSERT");
    tx.rollback().await.unwrap();

    // The marked sweep may delete across orgs, but cannot write one.
    let mut tx = as_app_maintenance(&pool).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 4, "the marked sweep reads across orgs");
    let err = sqlx::query("UPDATE console_sessions SET org_id = $1")
        .bind(b)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err), "42501", "the marked sweep moved a session");
    tx.rollback().await.unwrap();

    // The function, called as otto_app with no org pinned.
    let lookup = |hash: Vec<u8>| {
        let pool = pool.clone();
        async move {
            let mut tx = as_app_unpinned(&pool).await;
            let org: Option<Uuid> = sqlx::query_scalar("SELECT console_session_org($1)")
                .bind(hash)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            // The setting it reads under ends with the call.
            let after: Option<String> =
                sqlx::query_scalar("SELECT current_setting('app.console_session_lookup', true)")
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            assert_ne!(after.as_deref(), Some("on"), "the lookup setting leaked");
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(n, 0, "the table is readable after the call");
            tx.rollback().await.unwrap();
            org
        }
    };
    assert_eq!(lookup(live.clone()).await, Some(a.as_uuid()));

    // Setting the lookup's marker by hand admits otto_app to nothing: the
    // lookup policy admits only the function's owner.
    let mut tx = as_app_unpinned(&pool).await;
    sqlx::query("SET LOCAL app.console_session_lookup = 'on'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, 0, "otto_app read sessions by setting the lookup marker");
    tx.rollback().await.unwrap();
    // It names no role: who it admits is read from the function's owner at
    // query time, so moving that ownership needs no ALTER POLICY.
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT unnest(roles)::text FROM pg_policies \
         WHERE tablename = 'console_sessions' AND policyname = 'console_sessions_lookup'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(roles, vec!["public".to_string()]);
    assert_eq!(lookup(expired).await, None, "an expired session");
    assert_eq!(lookup(vec![9u8; 32]).await, None, "an unknown hash");

    // Its shape: SECURITY DEFINER with a pinned search_path, executable by
    // otto_app and not by PUBLIC.
    let (definer, config): (bool, Vec<String>) = sqlx::query_as(
        "SELECT prosecdef, coalesce(proconfig, '{}') FROM pg_proc \
         WHERE proname = 'console_session_org'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(definer);
    assert!(
        config.iter().any(|c| c.starts_with("search_path=")),
        "{config:?}"
    );
    let grants: (bool, bool) = sqlx::query_as(
        "SELECT has_function_privilege('otto_app', 'console_session_org(bytea)', 'EXECUTE'), \
                EXISTS (SELECT 1 FROM pg_proc p, aclexplode(p.proacl) acl \
                        WHERE p.proname = 'console_session_org' AND acl.grantee = 0)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grants, (true, false), "(otto_app may, PUBLIC may not)");
}

/// Today the migrating role is a superuser, so `console_session_org` bypasses
/// RLS and the lookup policy is never what admits it. Once it is not (#16),
/// the policy is the only thing that does, and #26's runbook moves ownership
/// of the table and the function to a new least-privilege role. Here a fresh
/// `NOSUPERUSER NOBYPASSRLS` role takes both over with `ALTER ... OWNER TO`
/// alone (what `REASSIGN OWNED` does, which a per-test database cannot run
/// against the cluster superuser that migrated it), and no `ALTER POLICY`:
/// the lookup works; the new owner's plain `SELECT` sees nothing; a non-owner
/// that sets the lookup's marker by hand sees nothing; and with the policy
/// gone the lookup finds nothing, so the policy, not the ownership, is what
/// admits it. The role is cluster-wide, so it is dropped whatever the test
/// does.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_console_lookup_follows_its_function_to_a_non_superuser_owner(pool: PgPool) {
    let owner = format!("cxp_test_owner_{}", Uuid::new_v4().simple());
    let me: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(&format!(
        "CREATE ROLE {owner} NOLOGIN NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&pool)
    .await
    .unwrap();

    // Run in a task so a failed assertion still reaches the cleanup below.
    let outcome = tokio::spawn(lookup_as_non_superuser_owner(pool.clone(), owner.clone())).await;

    for sql in [
        format!("REASSIGN OWNED BY {owner} TO {me}"),
        format!("DROP OWNED BY {owner}"),
        format!("DROP ROLE {owner}"),
    ] {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn lookup_as_non_superuser_owner(pool: PgPool, owner: String) {
    let admin = db(pool.clone());
    let a = org();
    let hash: Vec<u8> = (0..32).collect();
    let mut tx = admin.begin(a).await.unwrap();
    sqlx::query(
        "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
         refresh_token_enc, access_expires_at, expires_at) \
         VALUES ($1, $2, gen_random_uuid(), '\\x00', '\\x00', now(), now() + interval '1 day')",
    )
    .bind(&hash)
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let policy = || async {
        let (roles, qual): (Vec<String>, String) = sqlx::query_as(
            "SELECT roles::text[], qual FROM pg_policies \
             WHERE tablename = 'console_sessions' AND policyname = 'console_sessions_lookup'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        (roles, qual)
    };
    let before = policy().await;
    // Ownership only: no ALTER POLICY.
    for sql in [
        format!("ALTER TABLE console_sessions OWNER TO {owner}"),
        format!("ALTER FUNCTION console_session_org(bytea) OWNER TO {owner}"),
    ] {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    assert_eq!(policy().await, before, "the policy was changed");
    let (superuser, bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = $1")
            .bind(&owner)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((superuser, bypass), (false, false));
    let owners: (String, String) = sqlx::query_as(
        "SELECT (SELECT tableowner::text FROM pg_tables WHERE tablename = 'console_sessions'), \
                pg_get_userbyid(proowner)::text FROM pg_proc \
         WHERE oid = 'console_session_org(bytea)'::regprocedure",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owners, (owner.clone(), owner.clone()));

    let lookup = |hash: Vec<u8>| {
        let pool = pool.clone();
        async move {
            let mut tx = as_app_unpinned(&pool).await;
            let org: Option<Uuid> = sqlx::query_scalar("SELECT console_session_org($1)")
                .bind(hash)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            org
        }
    };
    assert_eq!(
        lookup(hash.clone()).await,
        Some(a.as_uuid()),
        "the lookup, run as a non-superuser owner"
    );
    assert_eq!(lookup(vec![9u8; 32]).await, None);

    // Plain SELECTs, unpinned: the new owner without the marker, and
    // otto_app (not the owner) with the marker set by hand. Neither sees a
    // row.
    for (role, marker) in [(owner.as_str(), false), ("otto_app", true)] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(&format!("SET LOCAL ROLE {role}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        if marker {
            sqlx::query("SET LOCAL app.console_session_lookup = 'on'")
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(n, 0, "{role} (marker set: {marker}) read sessions unpinned");
        tx.rollback().await.unwrap();
    }

    // Without the policy, the lookup finds nothing: the policy, not the
    // ownership, is what admits it.
    sqlx::query("DROP POLICY console_sessions_lookup ON console_sessions")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        lookup(hash).await,
        None,
        "the lookup worked without its policy"
    );
}

/// The "Database role" runbook in `docs/deploy/fly.md` (#26) moves the
/// tables and functions from the role that created
/// them to `cxp_server` with `REASSIGN OWNED`, then clears the old role out
/// with `DROP OWNED BY` and `DROP ROLE`. `DROP OWNED BY` also drops every
/// policy whose only target is the dropped role; `console_sessions_lookup`
/// names no role (it admits the function's owner, read from `pg_proc` at
/// query time), so it must come through those steps unchanged and keep
/// admitting the lookup under the new owner. The control policy
/// `cxp_test_named`, aimed at the old role alone, must disappear: that proves
/// the steps really do act on policies. Both owners are
/// `NOSUPERUSER NOBYPASSRLS`, so the policy, not a bypass, is what admits
/// the lookup. The roles are cluster-wide, so they are dropped whatever the
/// test does.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_console_lookup_policy_survives_drop_owned_by_its_previous_owner(pool: PgPool) {
    let suffix = Uuid::new_v4().simple();
    let old = format!("cxp_test_old_owner_{suffix}");
    let new = format!("cxp_test_new_owner_{suffix}");
    let me: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    for role in [&old, &new] {
        sqlx::query(&format!(
            "CREATE ROLE {role} NOLOGIN NOSUPERUSER NOBYPASSRLS"
        ))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Run in a task so a failed assertion still reaches the cleanup below.
    let outcome = tokio::spawn(lookup_survives_drop_owned(
        pool.clone(),
        old.clone(),
        new.clone(),
    ))
    .await;

    // `old` is normally gone already; `new` owns what it took over.
    let cleanup = [
        format!(
            "DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{old}') THEN \
                 EXECUTE 'REASSIGN OWNED BY {old} TO {me}'; EXECUTE 'DROP OWNED BY {old}'; \
                 END IF; END $$"
        ),
        format!("REASSIGN OWNED BY {new} TO {me}"),
        format!("DROP OWNED BY {new}"),
        format!("DROP ROLE IF EXISTS {old}"),
        format!("DROP ROLE {new}"),
    ];
    for sql in cleanup {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn lookup_survives_drop_owned(pool: PgPool, old: String, new: String) {
    let admin = db(pool.clone());
    let a = org();
    let hash: Vec<u8> = (0..32).collect();
    let mut tx = admin.begin(a).await.unwrap();
    sqlx::query(
        "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
         refresh_token_enc, access_expires_at, expires_at) \
         VALUES ($1, $2, gen_random_uuid(), '\\x00', '\\x00', now(), now() + interval '1 day')",
    )
    .bind(&hash)
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let policy = |name: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Vec<String>, String)>(
                "SELECT roles::text[], qual FROM pg_policies \
                 WHERE tablename = 'console_sessions' AND policyname = $1",
            )
            .bind(name)
            .fetch_optional(&pool)
            .await
            .unwrap()
        }
    };
    let before = policy("console_sessions_lookup")
        .await
        .expect("the lookup policy exists");

    // The state before the runbook: `old` owns the table and the function,
    // and a policy targets `old` alone.
    for sql in [
        format!("ALTER TABLE console_sessions OWNER TO {old}"),
        format!("ALTER FUNCTION console_session_org(bytea) OWNER TO {old}"),
        format!(
            "CREATE POLICY cxp_test_named ON console_sessions FOR SELECT TO {old} USING (false)"
        ),
    ] {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }
    assert!(policy("cxp_test_named").await.is_some());

    // The runbook's steps.
    for sql in [
        format!("REASSIGN OWNED BY {old} TO {new}"),
        format!("DROP OWNED BY {old}"),
        format!("DROP ROLE {old}"),
    ] {
        sqlx::query(&sql).execute(&pool).await.unwrap();
    }

    assert!(
        policy("cxp_test_named").await.is_none(),
        "DROP OWNED BY kept a policy aimed only at the dropped role"
    );
    assert_eq!(
        policy("console_sessions_lookup").await,
        Some(before),
        "the lookup policy was dropped or changed"
    );
    let owners: (String, String) = sqlx::query_as(
        "SELECT (SELECT tableowner::text FROM pg_tables WHERE tablename = 'console_sessions'), \
                pg_get_userbyid(proowner)::text FROM pg_proc \
         WHERE oid = 'console_session_org(bytea)'::regprocedure",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owners, (new.clone(), new.clone()));
    // otto_app keeps what the requests need: the grants moved with the
    // ownership rather than leaving with the dropped role.
    let (table, function): (bool, bool) = sqlx::query_as(
        "SELECT has_table_privilege('otto_app', 'console_sessions', \
                                    'SELECT, INSERT, UPDATE, DELETE'), \
                has_function_privilege('otto_app', 'console_session_org(bytea)', 'EXECUTE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((table, function), (true, true), "otto_app's grants");

    let lookup = |hash: Vec<u8>| {
        let pool = pool.clone();
        async move {
            let mut tx = as_app_unpinned(&pool).await;
            let org: Option<Uuid> = sqlx::query_scalar("SELECT console_session_org($1)")
                .bind(hash)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            org
        }
    };
    assert_eq!(
        lookup(hash).await,
        Some(a.as_uuid()),
        "the lookup, under the new owner"
    );
    assert_eq!(lookup(vec![9u8; 32]).await, None);
}

/// Foreign-key checks bypass row-level security, so only the composite
/// `(org_id, x_id)` keys stop a row in one org pointing at another org's row.
/// Each attempt writes a row for org B (passing RLS) that names org A's id.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn composite_foreign_keys_refuse_cross_org_references(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let theirs = seed(&db, a).await;
    let ours = seed(&db, b).await;

    let attempts: Vec<(&str, String, Uuid)> = vec![
        (
            "a ticket for another org's customer",
            "INSERT INTO tickets (org_id, customer_id, subject) VALUES ($1, $2, 's')".into(),
            theirs.customer,
        ),
        (
            "feedback on another org's experience",
            format!(
                "INSERT INTO feedback (org_id, customer_id, feedback_type, experience_id) \
                 VALUES ($1, '{}', 'praise', $2)",
                ours.customer
            ),
            theirs.experience,
        ),
        (
            "a customer on another org's component",
            "INSERT INTO customers (org_id, email, component_id) VALUES ($1, 'z@example.com', $2)"
                .into(),
            theirs.component,
        ),
        (
            "an application under another org's experience",
            "INSERT INTO applications (org_id, experience_id, name) VALUES ($1, $2, 'x')".into(),
            theirs.experience,
        ),
        (
            "a page under another org's application",
            "INSERT INTO pages (org_id, application_id, name) VALUES ($1, $2, 'x')".into(),
            theirs.application,
        ),
        (
            "a component on another org's page",
            "INSERT INTO components (org_id, page_id, name) VALUES ($1, $2, 'x')".into(),
            theirs.page,
        ),
        (
            "a member of another org's segment",
            format!(
                "INSERT INTO segment_members (org_id, segment_id, customer_id) \
                 VALUES ($1, $2, '{}')",
                ours.customer
            ),
            theirs.segment,
        ),
        (
            "a message in another org's chat session",
            "INSERT INTO chat_messages (org_id, session_id, role, content) \
             VALUES ($1, $2, 'agent', 'hi')"
                .into(),
            theirs.session,
        ),
        (
            "a response to another org's survey",
            format!(
                "INSERT INTO survey_responses (org_id, survey_id, customer_id, answers) \
                 VALUES ($1, $2, '{}', '{{}}')",
                ours.customer
            ),
            theirs.survey,
        ),
        (
            "a search gram for another org's customer",
            "INSERT INTO customer_search_grams (org_id, customer_id, gram, name_grams) \
             VALUES ($1, $2, 'sxyz', 1)"
                .into(),
            theirs.customer,
        ),
        (
            "a search term for another org's article",
            "INSERT INTO knowledge_search_terms (org_id, article_id, term) \
             VALUES ($1, $2, 'lxyz')"
                .into(),
            theirs.article,
        ),
        (
            "a ticket moved onto another org's customer",
            "UPDATE tickets SET customer_id = $2 WHERE org_id = $1".into(),
            theirs.customer,
        ),
    ];
    for (what, sql, foreign) in attempts {
        let mut tx = db.begin(b).await.unwrap();
        let err = sqlx::query(&sql)
            .bind(b)
            .bind(foreign)
            .execute(tx.conn())
            .await
            .expect_err(what);
        assert_eq!(sqlstate(&err), "23503", "{what}");
        drop(tx);
    }

    // The same statements succeed with org B's own ids, so the refusals above
    // are the org mismatch and nothing else.
    let mut tx = db.begin(b).await.unwrap();
    sqlx::query("INSERT INTO tickets (org_id, customer_id, subject) VALUES ($1, $2, 's')")
        .bind(b)
        .bind(ours.customer)
        .execute(tx.conn())
        .await
        .unwrap();
    sqlx::query("INSERT INTO pages (org_id, application_id, name) VALUES ($1, $2, 'x')")
        .bind(b)
        .bind(ours.application)
        .execute(tx.conn())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
}

/// The catalog chain cascades: deleting an entity deletes the levels under
/// it. Every other reference to a deleted entity (from customers, tickets,
/// feedback, surveys) is nulled (`ON DELETE SET NULL (col)`), only that
/// column, never the row or its `org_id`. Each catalog level is deleted in its
/// own org so all four reference columns are exercised.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn catalog_deletes_cascade_down_and_null_references(pool: PgPool) {
    let db = db(pool);
    let levels = [
        ("experiences", "experience_id"),
        ("applications", "application_id"),
        ("pages", "page_id"),
        ("components", "component_id"),
    ];
    for (deleted, (table, _)) in levels.iter().enumerate() {
        let o = org();
        let s = seed(&db, o).await;
        let ids = [s.experience, s.application, s.page, s.component];

        let mut tx = db.begin(o).await.unwrap();
        let n = sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
            .bind(ids[deleted])
            .execute(tx.conn())
            .await
            .unwrap()
            .rows_affected();
        assert_eq!(n, 1, "deleting from {table}");

        // The deleted level and everything under it is gone; above it stays.
        for (level, (catalog, _)) in levels.iter().enumerate() {
            let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {catalog}"))
                .fetch_one(tx.conn())
                .await
                .unwrap();
            let expected = if level >= deleted { 0 } else { 1 };
            assert_eq!(n, expected, "{catalog} after deleting from {table}");
        }

        for referencing in ["customers", "tickets", "feedback", "surveys"] {
            for (level, (_, column)) in levels.iter().enumerate() {
                let (value, org_id): (Option<Uuid>, OrgId) =
                    sqlx::query_as(&format!("SELECT {column}, org_id FROM {referencing}"))
                        .fetch_one(tx.conn())
                        .await
                        .unwrap_or_else(|e| {
                            panic!("{referencing} row went after deleting from {table}: {e}")
                        });
                assert_eq!(
                    org_id, o,
                    "{referencing}.org_id after deleting from {table}"
                );
                let expected = if level >= deleted {
                    None
                } else {
                    Some(ids[level])
                };
                assert_eq!(
                    value, expected,
                    "{referencing}.{column} after deleting from {table}"
                );
            }
        }
        tx.rollback().await.unwrap();
    }
}

/// Owned children cascade with their customer; rows a customer does not own
/// survive it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn deleting_a_customer_cascades_to_its_owned_rows(pool: PgPool) {
    let db = db(pool);
    let a = org();
    let s = seed(&db, a).await;

    let mut tx = db.begin(a).await.unwrap();
    sqlx::query("DELETE FROM customers WHERE id = $1")
        .bind(s.customer)
        .execute(tx.conn())
        .await
        .unwrap();
    for table in [
        "tickets",
        "interactions",
        "feedback",
        "survey_responses",
        "chat_sessions",
        "chat_messages",
        "segment_members",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(tx.conn())
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} outlived its customer");
    }
    // Surveys and segments are not owned by a customer.
    for table in ["surveys", "segments"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(tx.conn())
            .await
            .unwrap();
        assert_eq!(n, 1, "{table} went with a customer");
    }
    tx.rollback().await.unwrap();
}

/// A customer's email is unique within its org, not across orgs.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn customer_email_is_unique_per_org(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let insert = "INSERT INTO customers (org_id, email) VALUES ($1, 'same@example.com')";
    for o in [a, b] {
        let mut tx = db.begin(o).await.unwrap();
        sqlx::query(insert)
            .bind(o)
            .execute(tx.conn())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let mut tx = db.begin(a).await.unwrap();
    let err = sqlx::query(insert)
        .bind(a)
        .execute(tx.conn())
        .await
        .unwrap_err();
    assert_eq!(sqlstate(&err), "23505");
}

/// Structural invariants, read back from the catalog: every foreign key
/// between public tables is (org_id, x_id) -> (org_id, id), every tenant
/// table with an id carries UNIQUE (org_id, id), otto_app may read and write
/// every table, every non-unique index but the two cross-org queues leads
/// with org_id, and the objects the baseline dropped are gone.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_schema_keeps_its_structural_invariants(pool: PgPool) {
    let fks: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT c.conname::text, \
                (SELECT string_agg(a.attname, ',' ORDER BY k.ord) \
                   FROM unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) \
                   JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum), \
                (SELECT string_agg(a.attname, ',' ORDER BY k.ord) \
                   FROM unnest(c.confkey) WITH ORDINALITY AS k(attnum, ord) \
                   JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.attnum) \
         FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace \
         WHERE c.contype = 'f' AND n.nspname = 'public' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!fks.is_empty());
    for (name, cols, referenced) in &fks {
        let cols: Vec<&str> = cols.split(',').collect();
        assert!(
            cols.len() == 2 && cols[0] == "org_id",
            "{name} is not an (org_id, x_id) foreign key: {cols:?}"
        );
        assert_eq!(
            referenced, "org_id,id",
            "{name} must reference (org_id, id), in that order"
        );
    }

    for table in DOMAIN_TABLES.iter().filter(|t| **t != "segment_members") {
        let has: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
               SELECT 1 FROM pg_constraint c \
               WHERE c.conrelid = $1::regclass AND c.contype = 'u' \
                 AND (SELECT array_agg(a.attname::text ORDER BY a.attname) \
                        FROM pg_attribute a \
                        WHERE a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey)) \
                     = ARRAY['id', 'org_id'])",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(has, "{table} lacks UNIQUE (org_id, id)");

        let org_not_null: bool = sqlx::query_scalar(
            "SELECT attnotnull FROM pg_attribute WHERE attrelid = $1::regclass AND attname = 'org_id'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(org_not_null, "{table}.org_id is nullable");
    }

    // otto_app's grants, privilege by privilege (`has_table_privilege` with
    // a list answers whether ANY is held, so each is asked alone).
    let all = ["SELECT", "INSERT", "UPDATE", "DELETE"];
    let mut expected: Vec<(&str, &[&str])> = DOMAIN_TABLES
        .iter()
        .chain(SEARCH_INDEX_TABLES.iter())
        .chain(BOOKKEEPING_TABLES.iter())
        .map(|t| (*t, &all[..]))
        .collect();
    // Tombstones are permanent and written by the connecting role, so a
    // tenant transaction only reads them; removed members expire only
    // through expire_removed_members(); the migrator's ledger is not the app's.
    expected.push(("deleted_orgs", &["SELECT"]));
    expected.push(("removed_members", &["SELECT", "INSERT", "UPDATE"]));
    expected.push(("_sqlx_migrations", &[]));
    for (table, granted) in expected {
        for privilege in all {
            let has: bool = sqlx::query_scalar("SELECT has_table_privilege('otto_app', $1, $2)")
                .bind(table)
                .bind(privilege)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(
                has,
                granted.contains(&privilege),
                "otto_app {privilege} on {table}"
            );
        }
    }

    // Every foreign key is covered: its columns, in order, lead some index
    // on its table, so deleting a parent (or the org.deleted purge) never
    // scans the child.
    let uncovered: Vec<String> = sqlx::query_scalar(
        "SELECT c.conname::text FROM pg_constraint c \
         JOIN pg_namespace n ON n.oid = c.connamespace \
         WHERE c.contype = 'f' AND n.nspname = 'public' \
           AND NOT EXISTS ( \
             SELECT 1 FROM pg_index i \
             WHERE i.indrelid = c.conrelid \
               AND (i.indkey::int2[])[0:cardinality(c.conkey) - 1] = c.conkey) \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        uncovered.is_empty(),
        "foreign keys with no covering index: {uncovered:?}"
    );

    // Every non-unique index leads with org_id, so a tenant query (always
    // under `org_id = current_org()`) can use it; only the two cross-org
    // background queues do not. Unique indexes (primary keys, event_id) are
    // identities, not tenant filters.
    let not_org_led: Vec<String> = sqlx::query_scalar(
        "SELECT ic.relname::text FROM pg_index i \
         JOIN pg_class ic ON ic.oid = i.indexrelid \
         JOIN pg_namespace n ON n.oid = ic.relnamespace \
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] \
         WHERE n.nspname = 'public' AND NOT i.indisunique AND a.attname <> 'org_id' \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        not_org_led,
        ["platform_events_received_idx", "usage_outbox_due_idx"],
        "only the cross-org queues may lead with something other than org_id"
    );

    // No GIN index: under FORCE ROW LEVEL SECURITY its non-leakproof
    // operators can never be index conditions for a tenant query.
    let gin: Vec<String> = sqlx::query_scalar(
        "SELECT ic.relname::text FROM pg_index i \
         JOIN pg_class ic ON ic.oid = i.indexrelid \
         JOIN pg_am am ON am.oid = ic.relam \
         JOIN pg_namespace n ON n.oid = ic.relnamespace \
         WHERE n.nspname = 'public' AND am.amname <> 'btree' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(gin.is_empty(), "non-btree indexes: {gin:?}");

    let gone: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class \
         WHERE relname IN ('teams', 'feedback_embeddings') \
         UNION ALL SELECT extname::text FROM pg_extension \
           WHERE extname IN ('vector', 'uuid-ossp', 'pgcrypto') \
         UNION ALL SELECT attname::text FROM pg_attribute \
           WHERE attrelid = 'feedback'::regclass AND attnum > 0 AND NOT attisdropped \
             AND attname IN ('experience', 'application', 'page', 'component')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(gone.is_empty(), "dropped objects are back: {gone:?}");
}

/// A rolled-back release must migrate cleanly against a database that a
/// newer release already migrated: the applied version it does not know is
/// ignored (`ignore_missing`), see `cxp_core::MIGRATOR`.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_release_boots_against_a_database_a_newer_release_migrated(pool: PgPool) {
    let newest = cxp_core::MIGRATOR
        .iter()
        .map(|m| m.version)
        .max()
        .expect("migrations are embedded");
    sqlx::query(
        "INSERT INTO _sqlx_migrations \
         (version, description, success, checksum, execution_time) \
         VALUES ($1, 'from a newer release', true, '\\xdeadbeef'::bytea, 0)",
    )
    .bind(newest + 1)
    .execute(&pool)
    .await
    .unwrap();

    cxp_core::MIGRATOR.run(&pool).await.unwrap();

    // The newer release's row is left alone, not removed or re-applied.
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let known = cxp_core::MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .count();
    assert_eq!(rows, i64::try_from(known).unwrap() + 1);

    // The boot path (`cxp_core::migrate`) uses the same migrator.
    cxp_core::migrate(&db(pool)).await.unwrap();
}

/// A newer release's migration recorded as failed still refuses to boot,
/// even though this release does not know its version.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_failed_unknown_migration_still_refuses_to_boot(pool: PgPool) {
    let newest = cxp_core::MIGRATOR
        .iter()
        .map(|m| m.version)
        .max()
        .expect("migrations are embedded");
    sqlx::query(
        "INSERT INTO _sqlx_migrations \
         (version, description, success, checksum, execution_time) \
         VALUES ($1, 'failed in a newer release', false, '\\xdeadbeef'::bytea, 0)",
    )
    .bind(newest + 1)
    .execute(&pool)
    .await
    .unwrap();

    let err = cxp_core::MIGRATOR.run(&pool).await.unwrap_err();
    assert!(
        matches!(err, MigrateError::Dirty(v) if v == newest + 1),
        "expected Dirty({}), got {err:?}",
        newest + 1
    );
}

/// Ignoring unknown versions must not disable integrity: a known
/// migration whose applied checksum differs still refuses to boot.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_tampered_known_migration_still_refuses_to_boot(pool: PgPool) {
    let tampered = sqlx::query(
        "UPDATE _sqlx_migrations SET checksum = '\\xdeadbeef'::bytea WHERE version = 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(tampered.rows_affected(), 1, "version 1 must exist");

    let err = cxp_core::MIGRATOR.run(&pool).await.unwrap_err();
    assert!(
        matches!(err, MigrateError::VersionMismatch(1)),
        "expected VersionMismatch(1), got {err:?}"
    );
}

/// Roles and databases a test created, dropped when it ends, failed or not:
/// both are cluster-wide and outlive the test database. The drop runs on its
/// own thread and runtime (`Drop` cannot await), as the test pool's
/// superuser, databases first (a role that owns one cannot be dropped).
struct TestRoles {
    options: sqlx::postgres::PgConnectOptions,
    names: Vec<String>,
    databases: Vec<String>,
}

impl TestRoles {
    fn new(pool: &PgPool) -> Self {
        Self {
            options: pool.connect_options().as_ref().clone(),
            names: Vec::new(),
            databases: Vec::new(),
        }
    }

    /// `CREATE DATABASE name`, registered for the drop. Returns its name.
    async fn database(&mut self, pool: &PgPool, prefix: &str) -> String {
        let name = format!("{prefix}_{}", Uuid::new_v4().simple());
        self.databases.push(name.clone());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(pool)
            .await
            .unwrap();
        name
    }

    /// `CREATE ROLE name LOGIN PASSWORD ... <attributes>`, registered for
    /// the drop before it is created. Returns a `Db` logged in as it.
    async fn login(&mut self, pool: &PgPool, prefix: &str, attributes: &str) -> (String, Db) {
        let name = format!("{prefix}_{}", Uuid::new_v4().simple());
        let password = Uuid::new_v4().simple().to_string();
        self.names.push(name.clone());
        sqlx::query(&format!(
            "CREATE ROLE {name} LOGIN PASSWORD '{password}' {attributes}"
        ))
        .execute(pool)
        .await
        .unwrap();
        let options = self.options.clone().username(&name).password(&password);
        let as_role = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        (name, Db::from_pool(as_role))
    }

    /// `CREATE ROLE name NOLOGIN <attributes>`, registered for the drop.
    async fn nologin(&mut self, pool: &PgPool, prefix: &str, attributes: &str) -> String {
        let name = format!("{prefix}_{}", Uuid::new_v4().simple());
        self.names.push(name.clone());
        sqlx::query(&format!("CREATE ROLE {name} NOLOGIN {attributes}"))
            .execute(pool)
            .await
            .unwrap();
        name
    }
}

impl Drop for TestRoles {
    fn drop(&mut self) {
        let options = self.options.clone();
        let names = std::mem::take(&mut self.names);
        let databases = std::mem::take(&mut self.databases);
        let dropped = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime to drop test roles");
            rt.block_on(async move {
                use sqlx::Connection;
                let mut conn = sqlx::PgConnection::connect_with(&options).await?;
                for database in &databases {
                    sqlx::query(&format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)"))
                        .execute(&mut conn)
                        .await?;
                }
                for name in names.iter().rev() {
                    for stmt in [
                        format!(
                            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                             WHERE usename = '{name}'"
                        ),
                        // A role that owns objects here (the ownership
                        // checks make some) cannot be dropped until they
                        // go back to the test superuser.
                        format!(
                            "DO $$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{name}') \
                             THEN REASSIGN OWNED BY {name} TO CURRENT_USER; DROP OWNED BY {name}; \
                             END IF; END $$"
                        ),
                        format!("DROP ROLE IF EXISTS {name}"),
                    ] {
                        sqlx::query(&stmt).execute(&mut conn).await?;
                    }
                }
                Ok::<_, sqlx::Error>(())
            })
        })
        .join();
        match dropped {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("could not drop test roles: {e}"),
            Err(_) => eprintln!("could not drop test roles: the drop thread panicked"),
        }
    }
}

/// `(user, role, privileges)` of each privileged role `inspect` found.
fn found(role: &cxp_core::db_role::DbRole) -> Vec<(&str, &str, &[cxp_core::db_role::Privilege])> {
    role.privileged
        .iter()
        .map(|p| (p.user.as_str(), p.name.as_str(), p.privileges.as_slice()))
        .collect()
}

/// The boot check's view of a login role (#16): a plain role (even a member
/// of `otto_app`) is not privileged; SUPERUSER, BYPASSRLS, CREATEROLE,
/// REPLICATION, membership in a role with any of them (one `SET ROLE`
/// away), and membership in a cross-service predefined role are, and are
/// named.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_database_role_check_sees_every_superuser_equivalent(pool: PgPool) {
    use cxp_core::db_role::{inspect, Privilege::*};
    let mut roles = TestRoles::new(&pool);

    let su = inspect(&db(pool.clone())).await.unwrap();
    assert_eq!(su.session_user, su.current_user);
    // The test pool is a superuser, named alone (whatever else it holds).
    let [(user, name, privileges)] = found(&su)[..] else {
        panic!("{}", su.summary())
    };
    assert_eq!(
        (user, name),
        (su.current_user.as_str(), su.current_user.as_str())
    );
    assert!(privileges.contains(&Superuser), "{}", su.summary());

    let bypass_group = roles
        .nologin(&pool, "cxp_t_bypass_group", "NOSUPERUSER BYPASSRLS")
        .await;
    let base = "NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOREPLICATION";
    let (plain, plain_db) = roles.login(&pool, "cxp_t_plain", base).await;
    let (bypass, bypass_db) = roles
        .login(&pool, "cxp_t_bypass", "NOSUPERUSER BYPASSRLS")
        .await;
    let (member, member_db) = roles.login(&pool, "cxp_t_member", base).await;
    let (createrole, createrole_db) = roles
        .login(&pool, "cxp_t_createrole", "NOSUPERUSER CREATEROLE")
        .await;
    let (replication, replication_db) = roles
        .login(&pool, "cxp_t_replication", "NOSUPERUSER REPLICATION")
        .await;
    let (exec, exec_db) = roles.login(&pool, "cxp_t_exec", base).await;
    let (read_all, read_all_db) = roles.login(&pool, "cxp_t_read_all", base).await;
    for stmt in [
        format!("GRANT {bypass_group} TO {member}"),
        format!("GRANT pg_execute_server_program TO {exec}"),
        format!("GRANT pg_read_all_data TO {read_all}"),
        // otto_app is not privileged: membership in it, which every
        // deployment needs, must not count.
        format!("GRANT otto_app TO {plain}, {member}, {exec}"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }

    let mut seen = Vec::new();
    for (name, as_role) in [
        (&plain, plain_db),
        (&bypass, bypass_db),
        (&member, member_db),
        (&createrole, createrole_db),
        (&replication, replication_db),
        (&exec, exec_db),
        (&read_all, read_all_db),
    ] {
        let role = inspect(&as_role).await.unwrap();
        as_role.pool().close().await;
        assert_eq!((&role.session_user, &role.current_user), (name, name));
        seen.push(role);
    }
    let [plain_r, bypass_r, member_r, createrole_r, replication_r, exec_r, read_all_r] = &seen[..]
    else {
        unreachable!()
    };
    assert!(!plain_r.is_privileged(), "{}", plain_r.summary());
    assert_eq!(
        found(bypass_r),
        [(bypass.as_str(), bypass.as_str(), &[BypassRls][..])]
    );
    assert_eq!(
        found(member_r),
        [(member.as_str(), bypass_group.as_str(), &[BypassRls][..])]
    );
    assert!(
        member_r
            .summary()
            .contains(&format!("is a member of {bypass_group:?} (BYPASSRLS)")),
        "{}",
        member_r.summary()
    );
    assert_eq!(
        found(createrole_r),
        [(createrole.as_str(), createrole.as_str(), &[CreateRole][..])]
    );
    assert_eq!(
        found(replication_r),
        [(
            replication.as_str(),
            replication.as_str(),
            &[Replication][..]
        )]
    );
    assert_eq!(
        found(exec_r),
        [(
            exec.as_str(),
            "pg_execute_server_program",
            &[PredefinedRole][..]
        )]
    );
    assert_eq!(
        found(read_all_r),
        [(read_all.as_str(), "pg_read_all_data", &[PredefinedRole][..])]
    );
    assert!(
        read_all_r
            .summary()
            .contains(r#"is a member of "pg_read_all_data""#),
        "{}",
        read_all_r.summary()
    );
}

/// A superuser login whose connections start with `SET ROLE` to a plain
/// role (`?options=-c role=...`, or `ALTER ROLE ... SET role`) is still a
/// superuser session, one `RESET ROLE` away: the check reads the session
/// user too, and names it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_database_role_check_sees_a_superuser_session_behind_set_role(pool: PgPool) {
    use cxp_core::db_role::{inspect, Privilege::*};
    let mut roles = TestRoles::new(&pool);
    let plain = roles
        .nologin(&pool, "cxp_t_plain", "NOSUPERUSER NOBYPASSRLS")
        .await;

    let set = format!("SET ROLE {plain}");
    let as_plain = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _meta| {
            let set = set.clone();
            Box::pin(async move {
                sqlx::query(&set).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let role = inspect(&Db::from_pool(as_plain.clone())).await.unwrap();
    as_plain.close().await;

    assert_eq!(role.current_user, plain);
    assert_ne!(role.session_user, plain);
    assert!(role.is_privileged(), "{}", role.summary());
    let session = role.session_user.as_str();
    // Only the session user is privileged here.
    let [(user, name, privileges)] = found(&role)[..] else {
        panic!("{}", role.summary())
    };
    assert_eq!((user, name), (session, session));
    assert!(privileges.contains(&Superuser), "{}", role.summary());
    assert!(
        role.summary().contains(&format!(
            "database role {plain:?} (session user {session:?}): session user {session:?} is SUPERUSER"
        )),
        "{}",
        role.summary()
    );
}

/// The rest of what a deployment refuses (#16, round 2): CREATEDB, the
/// predefined roles that reach other sessions (`pg_signal_backend`,
/// `pg_monitor`, `pg_read_all_stats`, `pg_create_subscription`, and
/// `pg_maintain` where the server has it), and a Postgres 16 membership that
/// grants `SET` without `INHERIT`, still one `SET ROLE` from BYPASSRLS.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_database_role_check_sees_the_cross_service_roles_and_set_only_membership(
    pool: PgPool,
) {
    use cxp_core::db_role::{inspect, Privilege::*};
    let mut roles = TestRoles::new(&pool);
    let base = "NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB NOREPLICATION";

    let (createdb, createdb_db) = roles
        .login(&pool, "cxp_t_createdb", "NOSUPERUSER CREATEDB")
        .await;
    let role = inspect(&createdb_db).await.unwrap();
    createdb_db.pool().close().await;
    assert_eq!(
        found(&role),
        [(createdb.as_str(), createdb.as_str(), &[CreateDb][..])]
    );
    assert!(role.summary().contains("is CREATEDB"), "{}", role.summary());

    // Looked up by name: whichever this server has (pg_maintain is 17+).
    let present: Vec<String> = sqlx::query_scalar(
        "SELECT rolname::text FROM pg_roles WHERE rolname = ANY($1) ORDER BY rolname",
    )
    .bind(
        [
            "pg_signal_backend",
            "pg_monitor",
            "pg_read_all_stats",
            "pg_create_subscription",
            "pg_maintain",
        ]
        .map(String::from)
        .to_vec(),
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for required in [
        "pg_signal_backend",
        "pg_monitor",
        "pg_read_all_stats",
        "pg_create_subscription",
    ] {
        assert!(present.iter().any(|r| r == required), "{required} missing");
    }
    for predefined in &present {
        let (login, as_role) = roles.login(&pool, "cxp_t_predefined", base).await;
        sqlx::query(&format!("GRANT {predefined} TO {login}"))
            .execute(&pool)
            .await
            .unwrap();
        let role = inspect(&as_role).await.unwrap();
        as_role.pool().close().await;
        // pg_monitor brings pg_read_all_stats (and more) with it: the
        // granted role is among those named.
        assert!(
            found(&role)
                .iter()
                .any(|&(u, n, p)| u == login && n == predefined && p == [PredefinedRole]),
            "{predefined}: {}",
            role.summary()
        );
    }

    // GRANT ... WITH INHERIT FALSE, SET TRUE (Postgres 16): none of the
    // group's privileges apply until SET ROLE, which the member may do.
    let bypass_group = roles
        .nologin(&pool, "cxp_t_bypass_group", "NOSUPERUSER BYPASSRLS")
        .await;
    let (set_only, set_only_db) = roles.login(&pool, "cxp_t_set_only", base).await;
    sqlx::query(&format!(
        "GRANT {bypass_group} TO {set_only} WITH INHERIT FALSE, SET TRUE"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let (inherits, set_ok): (bool, bool) = sqlx::query_as(
        "SELECT pg_has_role($1::text::name, $2::text::name, 'USAGE'), \
                pg_has_role($1::text::name, $2::text::name, 'SET')",
    )
    .bind(&set_only)
    .bind(&bypass_group)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((inherits, set_ok), (false, true), "the grant is SET-only");
    let role = inspect(&set_only_db).await.unwrap();
    set_only_db.pool().close().await;
    assert_eq!(
        found(&role),
        [(set_only.as_str(), bypass_group.as_str(), &[BypassRls][..])]
    );

    // A database owner is implicitly a member of pg_database_owner, which
    // is not refused: the e2e least-privilege test owns its database and
    // boots; here, the predefined role itself is not flagged.
    let flagged: bool = sqlx::query_scalar(
        "SELECT rolname = ANY($1) FROM pg_roles WHERE rolname = 'pg_database_owner'",
    )
    .bind(
        cxp_core::db_role::PREDEFINED_ROLES
            .map(String::from)
            .to_vec(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!flagged);
}

/// Cross-database `CONNECT` and the shared-instance signal (#16, round 2),
/// with uniquely named databases standing in for the other services'
/// (`inspect_with`), so the shared test cluster never looks like `otto-db`
/// to the tests running beside this one.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_database_role_check_sees_other_databases_and_a_shared_instance(pool: PgPool) {
    use cxp_core::db_role::{inspect, inspect_with, DatabaseAccess};
    let mut roles = TestRoles::new(&pool);
    let base = "NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB NOREPLICATION";
    let service = roles.database(&pool, "cxp_t_svc").await;
    let other = roles.database(&pool, "cxp_t_other").await;
    let services = [service.as_str()];
    let (login, as_role) = roles.login(&pool, "cxp_t_reach", base).await;
    let group = roles.nologin(&pool, "cxp_t_group", base).await;
    let owner = roles.nologin(&pool, "cxp_t_owner", base).await;
    sqlx::query(&format!("GRANT {group} TO {login}"))
        .execute(&pool)
        .await
        .unwrap();
    let access = |database: &str, through: &str| DatabaseAccess {
        user: login.clone(),
        database: database.into(),
        through: through.into(),
    };

    // The real list finds no other service here, so nothing is shared, and
    // PUBLIC's default CONNECT on the per-test databases is not counted.
    let role = inspect(&as_role).await.unwrap();
    assert!(!role.shares_the_instance(), "{}", role.summary());
    assert!(!role.is_refused_in_a_deployment(), "{}", role.summary());

    // Another service's database counts through PUBLIC; any other database
    // does not.
    let role = inspect_with(&as_role, &services).await.unwrap();
    assert_eq!(role.shared_with, std::slice::from_ref(&service));
    assert_eq!(role.other_databases, [access(&service, "PUBLIC")]);
    assert!(role.is_refused_in_a_deployment());
    assert!(!role.is_privileged());

    // Revoked from PUBLIC (R3): out of reach, still a shared instance.
    sqlx::query(&format!("REVOKE CONNECT ON DATABASE {service} FROM PUBLIC"))
        .execute(&pool)
        .await
        .unwrap();
    let role = inspect_with(&as_role, &services).await.unwrap();
    assert!(role.other_databases.is_empty(), "{}", role.summary());
    assert!(role.shares_the_instance());
    assert!(!role.is_refused_in_a_deployment());

    // A grant to a role it belongs to (a later grant to otto_app) counts on
    // any database, and on another service's after the revoke too.
    for stmt in [
        format!("GRANT CONNECT ON DATABASE {other} TO {group}"),
        format!("GRANT CONNECT ON DATABASE {service} TO {group}"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }
    let role = inspect_with(&as_role, &services).await.unwrap();
    let mut expected = vec![access(&other, &group), access(&service, &group)];
    expected.sort_by(|a, b| a.database.cmp(&b.database));
    assert_eq!(role.other_databases, expected);
    for stmt in [
        format!("REVOKE CONNECT ON DATABASE {other} FROM {group}"),
        format!("REVOKE CONNECT ON DATABASE {service} FROM {group}"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }
    assert!(inspect_with(&as_role, &services)
        .await
        .unwrap()
        .other_databases
        .is_empty());

    // Membership in a database's owning role counts.
    for stmt in [
        format!("ALTER DATABASE {other} OWNER TO {owner}"),
        format!("GRANT {owner} TO {login}"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }
    let role = inspect_with(&as_role, &services).await.unwrap();
    assert_eq!(role.other_databases, [access(&other, &owner)]);
    assert!(
        role.summary().contains(&format!(
            "can connect to database {other:?} (through {owner:?})"
        )),
        "{}",
        role.summary()
    );
    as_role.pool().close().await;
}

/// `(kind, name, owner)` of each object `inspect` found owned.
fn owned(role: &cxp_core::db_role::DbRole) -> Vec<(&str, &str, &str)> {
    role.owned
        .iter()
        .map(|o| (o.kind, o.name.as_str(), o.owner.as_str()))
        .collect()
}

/// What the runtime role owns and may TRUNCATE or add a TRIGGER to (#27):
/// each is a way around row-level security (an owner may `NO FORCE` RLS or
/// drop a policy; TRUNCATE ignores RLS; a trigger runs as whoever fires
/// it), so the check names every one, however it is held.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_database_role_check_sees_what_a_role_owns_and_can_truncate(pool: PgPool) {
    use cxp_core::db_role::inspect;
    let mut roles = TestRoles::new(&pool);
    let base = "NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB NOREPLICATION";

    // The superuser pool owns everything; its ownership is not read.
    let su = inspect(&db(pool.clone())).await.unwrap();
    assert!(su.owned.is_empty(), "{}", su.summary());
    assert!(su.table_privileges.is_empty(), "{}", su.summary());

    let (plain, plain_db) = roles.login(&pool, "cxp_t_plain", base).await;
    let (table_owner, table_owner_db) = roles.login(&pool, "cxp_t_tbl_owner", base).await;
    let group_owner = roles.nologin(&pool, "cxp_t_grp_owner", base).await;
    let (member, member_db) = roles.login(&pool, "cxp_t_member", base).await;
    let (truncate, truncate_db) = roles.login(&pool, "cxp_t_truncate", base).await;
    let (trigger, trigger_db) = roles.login(&pool, "cxp_t_trigger", base).await;
    let (db_owner, db_owner_db) = roles.login(&pool, "cxp_t_db_owner", base).await;
    // TRUNCATE held by a group the login can SET ROLE to but does not
    // inherit from: NOINHERIT, or (Postgres 16) a grant WITH INHERIT FALSE.
    let truncate_group = roles.nologin(&pool, "cxp_t_trunc_grp", base).await;
    let (noinherit, noinherit_db) = roles
        .login(&pool, "cxp_t_noinherit", &format!("{base} NOINHERIT"))
        .await;
    let (set_only, set_only_db) = roles.login(&pool, "cxp_t_set_only", base).await;
    for stmt in [
        format!("GRANT otto_app TO {plain}, {table_owner}, {member}, {truncate}, {trigger}"),
        format!("ALTER TABLE tickets OWNER TO {table_owner}"),
        format!("ALTER TABLE segments OWNER TO {group_owner}"),
        format!("GRANT {group_owner} TO {member}"),
        format!("GRANT TRUNCATE ON customers TO {truncate}"),
        format!("GRANT TRIGGER ON feedback TO {trigger}"),
        format!("GRANT TRUNCATE ON customers TO {truncate_group}"),
        format!("GRANT {truncate_group} TO {noinherit}"),
        format!("GRANT {truncate_group} TO {set_only} WITH INHERIT FALSE, SET TRUE"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }

    // A member of otto_app only: nothing owned, nothing to truncate.
    let role = inspect(&plain_db).await.unwrap();
    assert!(role.owned.is_empty(), "{}", role.summary());
    assert_eq!(role.owned_total, 0);
    assert!(role.table_privileges.is_empty(), "{}", role.summary());
    assert!(!role.owns_objects());
    assert!(!role.can_alter_tables());
    assert!(
        role.summary().ends_with(
            "; it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"
        ),
        "{}",
        role.summary()
    );
    plain_db.pool().close().await;

    // The owner of one table (and, with it, its indexes).
    let role = inspect(&table_owner_db).await.unwrap();
    assert!(
        owned(&role).contains(&("table", "tickets", table_owner.as_str())),
        "{}",
        role.summary()
    );
    assert!(role.owned.iter().all(|o| o.user == table_owner));
    // Its indexes and owned sequences come with it, counted from the
    // catalog so the test follows the schema; the table is listed first
    // (kinds in order) and the list stops at the cap.
    let expected: i64 = sqlx::query_scalar(
        "SELECT 1 \
           + (SELECT count(*) FROM pg_index WHERE indrelid = 'tickets'::regclass) \
           + (SELECT count(*) FROM pg_depend d \
              JOIN pg_class s ON s.oid = d.objid AND s.relkind = 'S' \
              WHERE d.refobjid = 'tickets'::regclass AND d.deptype IN ('a', 'i'))",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let cap = cxp_core::db_role::MAX_REPORTED;
    assert_eq!(role.owned_total, expected, "{}", role.summary());
    assert_eq!(
        role.owned.len() as i64,
        expected.min(cap),
        "{}",
        role.summary()
    );
    assert_eq!(
        owned(&role)[0],
        ("table", "tickets", table_owner.as_str()),
        "{}",
        role.summary()
    );
    // Owned relations are not reported again as table privileges.
    assert!(role.table_privileges.is_empty(), "{}", role.summary());
    assert!(role.owns_objects());
    assert!(role.can_alter_tables());
    assert!(
        role.summary().contains(r#"table "tickets""#),
        "{}",
        role.summary()
    );
    table_owner_db.pool().close().await;

    // A member of a NOLOGIN owner: one SET ROLE away, named as the owner.
    let role = inspect(&member_db).await.unwrap();
    assert!(
        owned(&role).contains(&("table", "segments", group_owner.as_str())),
        "{}",
        role.summary()
    );
    assert!(role.owned.iter().all(|o| o.user == member));
    assert!(role.can_alter_tables());
    member_db.pool().close().await;

    // TRUNCATE and TRIGGER, held without owning the table.
    let role = inspect(&truncate_db).await.unwrap();
    assert!(role.owned.is_empty(), "{}", role.summary());
    let privileges: Vec<_> = role
        .table_privileges
        .iter()
        .map(|t| (t.user.as_str(), t.table.as_str(), t.privileges.clone()))
        .collect();
    assert_eq!(
        privileges,
        [(truncate.as_str(), "customers", vec!["TRUNCATE"])]
    );
    assert!(role.can_alter_tables());
    assert!(
        role.summary().contains(r#"can TRUNCATE table "customers""#),
        "{}",
        role.summary()
    );
    truncate_db.pool().close().await;
    let role = inspect(&trigger_db).await.unwrap();
    assert!(role.owned.is_empty(), "{}", role.summary());
    let privileges: Vec<_> = role
        .table_privileges
        .iter()
        .map(|t| (t.user.as_str(), t.table.as_str(), t.privileges.clone()))
        .collect();
    assert_eq!(
        privileges,
        [(trigger.as_str(), "feedback", vec!["TRIGGER"])]
    );
    assert!(role.can_alter_tables());
    trigger_db.pool().close().await;

    // TRUNCATE one SET ROLE away counts, inherited or not.
    for (login, as_login) in [(&noinherit, noinherit_db), (&set_only, set_only_db)] {
        let role = inspect(&as_login).await.unwrap();
        let privileges: Vec<_> = role
            .table_privileges
            .iter()
            .map(|t| (t.user.as_str(), t.table.as_str(), t.privileges.clone()))
            .collect();
        assert_eq!(
            privileges,
            [(login.as_str(), "customers", vec!["TRUNCATE"])],
            "{}",
            role.summary()
        );
        assert!(role.can_alter_tables());
        as_login.pool().close().await;
    }

    // The database's owner: the database, and schema public through
    // pg_database_owner, which every database owner is implicitly in.
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(&format!(
        "ALTER DATABASE \"{database}\" OWNER TO {db_owner}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let role = inspect(&db_owner_db).await.unwrap();
    let found = owned(&role);
    assert!(
        found.contains(&("database", database.as_str(), db_owner.as_str())),
        "{}",
        role.summary()
    );
    assert!(
        found.contains(&("schema", "public", "pg_database_owner")),
        "{}",
        role.summary()
    );
    assert!(role.can_alter_tables());
    assert!(
        role.summary()
            .contains(&format!("it owns database {database:?}")),
        "{}",
        role.summary()
    );
    db_owner_db.pool().close().await;
}

/// The runtime role needs, beyond `otto_app`, only `INSERT` on
/// `deleted_orgs` (the `org.deleted` tombstone, written as itself), which
/// migration 0005 grants to `cxp_runtime` (#27).
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn missing_runtime_grants_names_the_tombstone_insert(pool: PgPool) {
    use cxp_core::db_role::missing_runtime_grants;
    let mut roles = TestRoles::new(&pool);
    let base = "NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB NOREPLICATION";

    assert!(missing_runtime_grants(&db(pool.clone()))
        .await
        .unwrap()
        .is_empty());

    let (login, as_login) = roles.login(&pool, "cxp_t_runtime", base).await;
    sqlx::query(&format!("GRANT otto_app TO {login}"))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        missing_runtime_grants(&as_login).await.unwrap(),
        ["INSERT on deleted_orgs"]
    );

    // cxp_runtime is cluster-wide and shared by concurrent tests: created
    // if missing, never dropped.
    for stmt in [
        "DO $$ BEGIN CREATE ROLE cxp_runtime NOLOGIN; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
            .to_string(),
        "GRANT INSERT ON deleted_orgs TO cxp_runtime".to_string(),
        format!("GRANT cxp_runtime TO {login}"),
    ] {
        sqlx::query(&stmt).execute(&pool).await.unwrap();
    }
    assert!(missing_runtime_grants(&as_login).await.unwrap().is_empty());
    as_login.pool().close().await;
}

/// Before migrations there is no `deleted_orgs` to need a grant on.
#[sqlx::test(migrations = false)]
async fn missing_runtime_grants_is_empty_before_migrations(pool: PgPool) {
    assert!(cxp_core::db_role::missing_runtime_grants(&db(pool))
        .await
        .unwrap()
        .is_empty());
}

/// Where `cxp_runtime` exists before migrating (the runbook creates it),
/// migration 0005 grants it the tombstone insert.
#[sqlx::test(migrations = false)]
async fn migration_0005_grants_the_tombstone_insert_to_cxp_runtime(pool: PgPool) {
    sqlx::query(
        "DO $$ BEGIN CREATE ROLE cxp_runtime NOLOGIN; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    cxp_core::migrate(&db(pool.clone())).await.unwrap();
    let granted: bool = sqlx::query_scalar(
        "SELECT has_table_privilege('cxp_runtime', 'public.deleted_orgs', 'INSERT')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(granted);
}

/// Migration 0006 pins both SECURITY DEFINER functions' search_path with
/// `pg_temp` last and schema-qualifies every call in them, so nothing a
/// lesser role creates (an overload in a schema it may CREATE in, a
/// temporary object) is resolved as the owner (#27).
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn migration_0006_pins_the_security_definer_functions(pool: PgPool) {
    let rows: Vec<(String, bool, Vec<String>, String, bool)> = sqlx::query_as(
        "SELECT p.oid::regprocedure::text, p.prosecdef, coalesce(p.proconfig, '{}'), p.prosrc, \
                EXISTS (SELECT 1 FROM aclexplode(coalesce(p.proacl, acldefault('f', p.proowner))) a \
                        WHERE a.grantee = 0) \
         FROM pg_proc p WHERE p.prosecdef AND p.pronamespace = 'public'::regnamespace \
         ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        names,
        ["console_session_org(bytea)", "expire_removed_members()"],
        "every SECURITY DEFINER function is covered here"
    );
    for (name, definer, config, src, public_execute) in &rows {
        assert!(definer, "{name}");
        assert!(
            config
                .iter()
                .any(|c| c == "search_path=pg_catalog, public, pg_temp"),
            "{name}: {config:?}"
        );
        assert!(!public_execute, "{name}: PUBLIC may execute it");
        // Every now(), make_interval(, and count( is pg_catalog's.
        for call in ["now()", "make_interval(", "count("] {
            let unqualified = src
                .match_indices(call)
                .filter(|(at, _)| !src[..*at].ends_with("pg_catalog."))
                .count();
            assert_eq!(unqualified, 0, "{name} calls {call} unqualified: {src}");
        }
    }
    let lookup = rows
        .iter()
        .find(|r| r.0.starts_with("console_session_org"))
        .unwrap();
    assert!(
        lookup
            .2
            .iter()
            .any(|c| c == "app.console_session_lookup=on"),
        "{:?}",
        lookup.2
    );
    assert!(lookup.3.contains("pg_catalog.now()"), "{}", lookup.3);
    let expire = rows
        .iter()
        .find(|r| r.0.starts_with("expire_removed_members"))
        .unwrap();
    assert!(
        expire.3.contains("pg_catalog.make_interval(secs => 600)"),
        "{}",
        expire.3
    );
}
