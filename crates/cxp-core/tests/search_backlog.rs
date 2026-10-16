//! Migration 0004 under the role production connects as (#16, #17): a
//! login role that is not a superuser and not BYPASSRLS, owns the database,
//! and is a member of `otto_app`. Row-level security binds it, so the
//! migration sees no org's rows and indexes none: rows that predate 0004
//! stay unindexed, and search does not find them, until an ordinary write
//! indexes them (the update below, one org at a time in that org's own
//! pinned transaction, or the superuser rebuild in docs/deploy/fly.md).
//! Since #34 search reads no backlog of such rows; production had none.

mod common;

use std::borrow::Cow;

use common::{articles_both_ways, customers_both_ways, org};
use cxp_core::customers::CustomersExt;
use cxp_core::knowledge::KnowledgeExt;
use cxp_core::Page;
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// A login role set up as docs/deploy/fly.md sets up `cxp_server`, owning
/// this test's database. Returns a pool connected as it.
async fn least_privilege_owner(pool: &PgPool, role: &str) -> PgPool {
    let database = pool.connect_options().get_database().unwrap().to_string();
    let password = Uuid::new_v4().simple().to_string();
    for stmt in [
        format!(
            "CREATE ROLE {role} LOGIN PASSWORD '{password}' \
             NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE"
        ),
        "DO $$ BEGIN CREATE ROLE otto_app NOLOGIN; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
            .to_string(),
        format!("GRANT otto_app TO {role}"),
        format!("GRANT SET ON PARAMETER app.console_session_lookup TO {role}"),
        format!("ALTER DATABASE \"{database}\" OWNER TO {role}"),
        format!("REVOKE CONNECT ON DATABASE \"{database}\" FROM PUBLIC"),
        format!("GRANT CONNECT ON DATABASE \"{database}\" TO {role}"),
    ] {
        sqlx::query(&stmt).execute(pool).await.unwrap();
    }
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .username(role)
        .password(&password);
    PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap()
}

/// The role is cluster-wide: whatever the test did, end its sessions and
/// drop it (and what it owns) before sqlx drops the database.
async fn drop_role(pool: &PgPool, role: &str) {
    for stmt in [
        format!("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '{role}'"),
        format!("REASSIGN OWNED BY {role} TO CURRENT_USER"),
        format!("DROP OWNED BY {role}"),
        format!("DROP ROLE {role}"),
    ] {
        sqlx::query(&stmt).execute(pool).await.unwrap();
    }
}

/// Every migration before 0004: the schema a database had before #17.
fn before_the_index() -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            cxp_core::MIGRATOR
                .iter()
                .filter(|m| m.version < 4)
                .cloned()
                .collect(),
        ),
        ignore_missing: true,
        locking: true,
        no_tx: false,
    }
}

/// Customers and articles written before the index existed.
async fn seed(db: &Db, org: OrgId, customers: i32, articles: i32) {
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(
        "INSERT INTO customers (org_id, email, name, created_at) \
         SELECT $1, lower(f) || '.sailor' || n || '@' || d, f || ' Sailor' || n, \
                now() - make_interval(secs => n) \
         FROM generate_series(1, $2) AS n, \
              LATERAL (SELECT (ARRAY['Grace','Alan','Ada','Linus','Zoë'])[1 + n % 5] AS f, \
                              (ARRAY['navy.example','example.com'])[1 + n % 2] AS d) AS x",
    )
    .bind(org)
    .bind(customers)
    .execute(tx.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published, created_at) \
         SELECT $1, 'Guide ' || n, repeat('gadget ', 1 + n % 7) || 'manual for widgets', \
                n % 10 <> 0, now() - make_interval(secs => n) \
         FROM generate_series(1, $2) AS n",
    )
    .bind(org)
    .bind(articles)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

const CUSTOMER_QUERIES: &[&str] = &[
    "grace",
    "Sailor1",
    "navy",
    "ad",
    "zoe",
    "Linus Sailr42",
    "x",
];
const ARTICLE_QUERIES: &[&str] = &["gadget", "widget manual", "guide 1", "gu", "zebra"];

/// Search agrees with the pre-#17 scan for this org, exactly (none of these
/// reach the cap).
async fn search_is_exact(db: &Db, org: OrgId) {
    let mut tx = db.begin(org).await.unwrap();
    for q in CUSTOMER_QUERIES {
        let (indexed, scanned) = customers_both_ways(&mut tx, q, Page::new(Some(200), None))
            .await
            .unwrap();
        assert_eq!(indexed, scanned, "customers, query {q:?}");
    }
    for q in ARTICLE_QUERIES {
        let (indexed, scanned) = articles_both_ways(&mut tx, q).await.unwrap();
        assert_eq!(indexed, scanned, "articles, query {q:?}");
    }
    tx.commit().await.unwrap();
}

/// Index this org's rows that predate the index, as any write to them
/// would: setting the flag fires the sync triggers.
async fn index_org(db: &Db, org: OrgId) {
    let mut tx = db.begin(org).await.unwrap();
    for table in ["customers", "knowledge_articles"] {
        sqlx::query(&format!(
            "UPDATE {table} SET search_indexed = true WHERE org_id = $1"
        ))
        .bind(org)
        .execute(tx.conn())
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn the_index_is_built_per_org_under_a_least_privilege_owner(pool: PgPool) {
    let role = format!("cxp_search_{}", Uuid::new_v4().simple());
    let outcome = tokio::spawn(build_under(pool.clone(), role.clone())).await;
    drop_role(&pool, &role).await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn build_under(pool: PgPool, role: String) {
    let as_role = least_privilege_owner(&pool, &role).await;
    let (superuser, bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&as_role)
            .await
            .unwrap();
    assert_eq!((superuser, bypass), (false, false));

    // A database as it was before #17, with rows in two orgs.
    before_the_index().run(&as_role).await.unwrap();
    let db = Db::from_pool(as_role.clone());
    let (a, b) = (org(), org());
    seed(&db, a, 150, 130).await;
    seed(&db, b, 30, 20).await;

    // 0004, as the owner. Row-level security hides every org's rows from
    // it, and it needs none of them: it indexes nothing and refuses nothing.
    cxp_core::migrate(&db).await.unwrap();
    assert_eq!(
        count(&pool, "SELECT count(*) FROM customer_search_grams").await,
        0
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM knowledge_search_terms").await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM customers WHERE NOT search_indexed"
        )
        .await,
        180
    );

    // Search reads only the index, so org A's rows that predate it are not
    // found yet.
    let mut tx = db.begin(a).await.unwrap();
    assert!(tx
        .search_customers(&common::permit().await, "grace", Page::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(tx
        .search_articles(&common::permit().await, "gadget")
        .await
        .unwrap()
        .items
        .is_empty());
    tx.commit().await.unwrap();

    // An ordinary write indexes them: the triggers write the index rows.
    index_org(&db, a).await;
    let left = |table: &str, org: OrgId| {
        format!(
            "SELECT count(*) FROM {table} WHERE NOT search_indexed AND org_id = '{}'",
            org.as_uuid()
        )
    };
    assert_eq!(count(&pool, &left("customers", a)).await, 0);
    assert_eq!(count(&pool, &left("knowledge_articles", a)).await, 0);
    // Org B's rows are untouched by A's update (row-level security).
    assert_eq!(count(&pool, &left("customers", b)).await, 30);
    search_is_exact(&db, a).await;

    index_org(&db, b).await;
    assert_eq!(count(&pool, &left("customers", b)).await, 0);
    search_is_exact(&db, b).await;

    // Everything is indexed: every customer, and every published article,
    // has index rows, and no unpublished one does.
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM customers c WHERE NOT EXISTS ( \
               SELECT 1 FROM customer_search_grams g \
               WHERE g.org_id = c.org_id AND g.customer_id = c.id)"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM knowledge_articles a WHERE published <> EXISTS ( \
               SELECT 1 FROM knowledge_search_terms t \
               WHERE t.org_id = a.org_id AND t.article_id = a.id)"
        )
        .await,
        0
    );
    search_is_exact(&db, a).await;
    search_is_exact(&db, b).await;
    as_role.close().await;
}
