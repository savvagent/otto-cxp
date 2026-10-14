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

/// Bookkeeping tables carrying a `<table>_tenant_isolation` policy.
const BOOKKEEPING_TABLES: [&str; 3] = ["platform_events", "usage_outbox", "usage_outbox_rejected"];

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
        "INSERT INTO knowledge_articles (org_id, title, content) \
         VALUES ($1, 'Refunds', 'How refunds work')"
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
    ];
    let mut tx = db.begin(org).await.unwrap();
    for sql in &statements {
        // Postgres refuses a bind carrying more parameters than the statement
        // references, so every statement is prefixed to reference all ten.
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
            .execute(tx.conn())
            .await
            .unwrap_or_else(|e| panic!("seeding failed on {sql}: {e}"));
    }
    tx.commit().await.unwrap();
    s
}

/// Prefix a statement with a no-op that references all ten seed parameters,
/// so each one's type is known even where `sql` does not use it.
fn with_param_types(sql: &str) -> String {
    format!(
        "WITH _p AS (SELECT $1::uuid, $2::uuid, $3::uuid, $4::uuid, $5::uuid, $6::uuid, \
         $7::uuid, $8::uuid, $9::uuid, $10::text) {sql}"
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

    for table in DOMAIN_TABLES {
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
    for table in DOMAIN_TABLES {
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
