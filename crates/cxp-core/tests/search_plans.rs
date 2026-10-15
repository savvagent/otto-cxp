//! The search statements' generic plans keep every per-candidate step a
//! per-candidate probe (#41). sqlx prepares each statement once per
//! connection, and Postgres moves to a generic plan after five runs; in orgs
//! of a few thousand to about 100k rows the planner then flattened the
//! customer `matches` lateral into a hash join over all of the org's
//! customers, and turned the article title check into a hashed subplan over
//! every published article. The statements fence those steps (`LIMIT 1` in
//! the lateral, `OFFSET 0` in each correlated `EXISTS`), which the planner
//! cannot see through whatever the statistics say.
//!
//! These tests check the fences, not costs: each plans the statement as a
//! request does (as `otto_app`, org pinned, `force_generic_plan`) with the
//! planner pushed toward the bad alternative (`enable_nestloop = off` makes
//! a flattened lateral a hash or merge join; index scans off make a per-row
//! probe a scan per candidate, so a hashed subplan wins wherever one is
//! allowed), and asserts the bad alternative is not there. Without the
//! fences both tests fail.

mod common;

use common::{db, org};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use serde_json::Value;
use sqlx::PgPool;

/// The tables a search reads per candidate; none may be read whole.
const SEARCH_TABLES: [&str; 4] = [
    "customers",
    "customer_search_grams",
    "knowledge_articles",
    "knowledge_search_terms",
];

/// Index scans, bitmap scans, and index-only scans off: a per-row probe is
/// then a scan per candidate, and the planner hashes any subplan it may.
const NO_INDEXES: [&str; 3] = [
    "enable_indexscan = off",
    "enable_indexonlyscan = off",
    "enable_bitmapscan = off",
];

/// One org with a few hundred customers and published articles, another
/// org beside it, and fresh statistics.
async fn seed(pool: &PgPool, db: &Db, org: OrgId) {
    for o in [org, common::org()] {
        let mut tx = db.begin(o).await.unwrap();
        sqlx::query(
            "INSERT INTO customers (org_id, email, name) \
             SELECT $1, 'person' || g || '@example.com', 'Person Number ' || g \
             FROM generate_series(1, 400) AS g",
        )
        .bind(o)
        .execute(tx.conn())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO knowledge_articles (org_id, title, content, published) \
             SELECT $1, 'Article title ' || g, 'Body text about topic number ' || g, true \
             FROM generate_series(1, 300) AS g",
        )
        .bind(o)
        .execute(tx.conn())
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    sqlx::query(
        "ANALYZE customers, customer_search_grams, knowledge_articles, knowledge_search_terms",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// `sql`'s generic plan as `EXPLAIN (FORMAT JSON)` gives it, planned in a
/// tenant transaction under `settings`. `types` and `args` are the
/// statement's parameter types and literal arguments (constants of these
/// tests).
async fn generic_plan(
    db: &Db,
    org: OrgId,
    settings: &[&str],
    types: &str,
    sql: &str,
    args: &str,
) -> Value {
    let mut tx = db.begin(org).await.unwrap();
    for s in settings
        .iter()
        .chain(&["plan_cache_mode = force_generic_plan"])
    {
        sqlx::query(&format!("SET LOCAL {s}"))
            .execute(tx.conn())
            .await
            .unwrap();
    }
    sqlx::raw_sql(&format!("PREPARE plan_check {types} AS {sql}"))
        .execute(tx.conn())
        .await
        .unwrap();
    let (plan,): (Value,) =
        sqlx::query_as(&format!("EXPLAIN (FORMAT JSON) EXECUTE plan_check {args}"))
            .fetch_one(tx.conn())
            .await
            .unwrap();
    sqlx::raw_sql("DEALLOCATE plan_check")
        .execute(tx.conn())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    plan[0]["Plan"].clone()
}

/// `node` and every node under it.
fn subtree(node: &Value) -> Vec<&Value> {
    let mut out = vec![node];
    if let Some(children) = node.get("Plans").and_then(Value::as_array) {
        for child in children {
            out.extend(subtree(child));
        }
    }
    out
}

/// The node `EXPLAIN` names `name` ("CTE matches", "SubPlan 7").
fn named<'a>(root: &'a Value, name: &str) -> &'a Value {
    subtree(root)
        .into_iter()
        .find(|n| n["Subplan Name"] == name)
        .unwrap_or_else(|| panic!("no {name} in the plan: {root:#}"))
}

/// The subplans the plan hashes: every `hashed SubPlan N` in any node's
/// filter or condition text.
fn hashed_subplans(root: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for node in subtree(root) {
        let Some(fields) = node.as_object() else {
            continue;
        };
        for text in fields.values().filter_map(Value::as_str) {
            for (i, _) in text.match_indices("hashed SubPlan ") {
                let rest = &text[i + "hashed ".len()..];
                let end = rest
                    .char_indices()
                    .skip("SubPlan ".len())
                    .find(|(_, c)| !c.is_ascii_digit())
                    .map_or(rest.len(), |(j, _)| j);
                names.push(rest[..end].to_string());
            }
        }
    }
    names
}

/// The nodes under `node` that read one of the search tables.
fn search_table_reads(node: &Value) -> Vec<&Value> {
    subtree(node)
        .into_iter()
        .filter(|n| {
            n["Relation Name"]
                .as_str()
                .is_some_and(|r| SEARCH_TABLES.contains(&r))
        })
        .collect()
}

/// No hashed subplan reads a search table.
fn assert_no_hashed_search_table(root: &Value, what: &str) {
    for name in hashed_subplans(root) {
        let reads = search_table_reads(named(root, &name));
        assert!(
            reads.is_empty(),
            "{what}: {name} is hashed and reads {:?}",
            reads
                .iter()
                .map(|n| (&n["Node Type"], &n["Relation Name"]))
                .collect::<Vec<_>>()
        );
    }
}

fn customer_args(org: OrgId) -> String {
    format!("('{}', '%person%', 'person', 50, 0)", org.as_uuid())
}

fn article_args(org: OrgId) -> String {
    format!(
        "('{}', 'article title', '%article title%', 20)",
        org.as_uuid()
    )
}

const CUSTOMER_TYPES: &str = "(uuid, text, text, int8, int8)";
const ARTICLE_TYPES: &str = "(uuid, text, text, int8)";

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn customer_matches_probe_each_candidate(pool: PgPool) {
    let db = db(pool.clone());
    let org = org();
    seed(&pool, &db, org).await;
    let plan = generic_plan(
        &db,
        org,
        &["enable_nestloop = off"],
        CUSTOMER_TYPES,
        cxp_core::customers::search_customers_sql(),
        &customer_args(org),
    )
    .await;
    let matches = named(&plan, "CTE matches");
    for node in subtree(matches) {
        let kind = node["Node Type"].as_str().unwrap_or_default();
        assert!(
            kind != "Hash Join" && kind != "Merge Join",
            "matches joins its candidates to customers with a {kind}: {matches:#}"
        );
    }
    for read in search_table_reads(matches) {
        if read["Relation Name"] == "customers" {
            let kind = read["Node Type"].as_str().unwrap_or_default();
            assert!(
                kind == "Index Scan" || kind == "Index Only Scan",
                "matches reads customers with a {kind}: {matches:#}"
            );
        }
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn search_index_probes_are_never_hashed(pool: PgPool) {
    let db = db(pool.clone());
    let org = org();
    seed(&pool, &db, org).await;
    let customers = generic_plan(
        &db,
        org,
        &NO_INDEXES,
        CUSTOMER_TYPES,
        cxp_core::customers::search_customers_sql(),
        &customer_args(org),
    )
    .await;
    assert_no_hashed_search_table(&customers, "search_customers");
    let articles = generic_plan(
        &db,
        org,
        &NO_INDEXES,
        ARTICLE_TYPES,
        cxp_core::knowledge::search_articles_sql(),
        &article_args(org),
    )
    .await;
    assert_no_hashed_search_table(&articles, "search_articles");
}
