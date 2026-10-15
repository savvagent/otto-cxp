//! Helpers shared by every `crates/cxp-core/tests/*.rs` file. Include with
//! `mod common;`. Each test binary uses a different subset, hence the
//! crate-level `dead_code` allowance.
//!
//! Tests run under `#[sqlx::test(migrator = "cxp_core::MIGRATOR")]`, which
//! hands each test a fresh, migrated database on the cluster `DATABASE_URL`
//! names, connecting as a superuser. A superuser bypasses row-level
//! security, so a plain pool query proves nothing about isolation: tenant
//! work goes through [`db`]'s `Db::begin` (which assumes the tenant role,
//! `cxp_app`), and
//! unpinned checks go through [`as_app_unpinned`], [`as_app_maintenance`], or
//! [`tenant_role_db`].

#![allow(dead_code)]

use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

/// The test database as a [`Db`]. `Db::begin(org)` on it runs as `cxp_app`
/// pinned to `org`, exactly as a request does.
pub fn db(pool: PgPool) -> Db {
    cxp_core::tenant_db(pool)
}

/// A fresh, random org id. The platform issues these; nothing here checks
/// them against an orgs table, so any uuid is an org.
pub fn org() -> OrgId {
    OrgId::new()
}

/// A fresh, random user id.
pub fn user() -> UserId {
    UserId::new()
}

/// A `Db` whose every connection runs as the tenant role (`cxp_app`), so even unpinned
/// statements are subject to RLS (the test pool's superuser bypasses it).
/// `Db::begin` still pins and works. Close `.pool()` before the test returns
/// so sqlx can drop the test database.
pub async fn tenant_role_db(pool: &PgPool) -> Db {
    let set_role = format!("SET ROLE {}", cxp_core::TENANT_ROLE);
    Box::leak(format!("SET ROLE {}", cxp_core::TENANT_ROLE).into_boxed_str());
    let app_pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _meta| {
            let set_role = set_role.clone();
            Box::pin(async move {
                sqlx::query(&set_role).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    cxp_core::tenant_db(app_pool)
}

/// A transaction as the tenant role (`cxp_app`) with no org pinned, so `current_org()` is
/// NULL. This is what a request path that forgot to pin would see.
pub async fn as_app_unpinned(pool: &PgPool) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {}", cxp_core::TENANT_ROLE))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// [`as_app_unpinned`] with the maintenance marker set, as background
/// maintenance (`cxp_core::maintenance`) sets it.
pub async fn as_app_maintenance(pool: &PgPool) -> Transaction<'static, Postgres> {
    let mut tx = as_app_unpinned(pool).await;
    sqlx::query("SET LOCAL app.maintenance = 'on'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

/// The SQLSTATE of a failed statement: `23503` foreign-key violation,
/// `23505` unique violation, `42501` row-level-security refusal.
pub fn sqlstate(err: &sqlx::Error) -> String {
    err.as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.into_owned())
        .unwrap_or_else(|| format!("not a database error: {err}"))
}

// -------------------------------------------------------- stub team platform

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use cxp_core::catalog::{verify_team, VerifiedTeam};
use otto_resource::{ClientConfig, PlatformClient, TeamInfo};
use uuid::Uuid;

#[derive(Default)]
struct Teams {
    /// team -> the org it belongs to
    teams: Mutex<HashMap<Uuid, Uuid>>,
    /// Answer every team lookup with a 503.
    down: AtomicBool,
}

/// A stub otto platform answering `GET /internal/orgs/{org}/teams/{team}`
/// for the teams added to it, so tests can obtain [`VerifiedTeam`]s the only
/// way there is: [`verify_team`].
pub struct TeamPlatform {
    pub client: Arc<PlatformClient>,
    state: Arc<Teams>,
}

/// Like the real platform's, but it answers a known team whatever org the
/// path names, so a cross-org lookup exercises the org check in
/// `verify_team` itself.
async fn team_lookup(
    State(t): State<Arc<Teams>>,
    Path((_org, team)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamInfo>, StatusCode> {
    if t.down.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    match t.teams.lock().unwrap().get(&team) {
        Some(org) => Ok(Json(TeamInfo {
            id: team,
            org_id: *org,
            slug: "team".into(),
            name: "Team".into(),
        })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn team_platform() -> TeamPlatform {
    let state: Arc<Teams> = Arc::default();
    let app = Router::new()
        .route("/internal/orgs/{org}/teams/{team}", get(team_lookup))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = PlatformClient::new(ClientConfig::new(
        format!("http://{addr}"),
        "https://cxp.example.com/mcp",
        "otto_rs_test",
    ))
    .unwrap();
    TeamPlatform {
        client: Arc::new(client),
        state,
    }
}

impl TeamPlatform {
    /// A new team of `org`, known to the platform.
    pub fn add(&self, org: OrgId) -> Uuid {
        let team = Uuid::new_v4();
        self.state.teams.lock().unwrap().insert(team, org.as_uuid());
        team
    }

    /// A new team of `org`, verified.
    pub async fn team(&self, org: OrgId) -> VerifiedTeam {
        let id = self.add(org);
        verify_team(&self.client, org, id).await.unwrap()
    }

    pub fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
    }
}

// ------------------------------------------------- the search oracle (#17)

use cxp_core::customers::CustomersExt;
use cxp_core::knowledge::KnowledgeExt;
use cxp_core::search::TOTAL_CAP;
use cxp_core::{Error, Page};

/// `search_customers` as it was before #17: the match predicate evaluated on
/// every customer of the org, ordered as search orders. `$2` is the `ILIKE`
/// pattern, `$3` the query, `$4`/`$5` limit and offset.
const CUSTOMERS_BY_SCAN: &str = "SELECT id FROM customers \
     WHERE org_id = $1 AND (email ILIKE $2 OR name ILIKE $2 OR name % $3) \
     ORDER BY CASE WHEN email ILIKE $2 THEN 0 ELSE 1 END, \
              similarity(name, $3) DESC NULLS LAST, created_at DESC, id \
     LIMIT $4 OFFSET $5";
const CUSTOMERS_BY_SCAN_COUNT: &str = "SELECT count(*) FROM customers \
     WHERE org_id = $1 AND (email ILIKE $2 OR name ILIKE $2 OR name % $3)";

/// `search_articles` as it was before #17. `$2` is the query, `$3` the title
/// pattern, `$4` the limit.
const ARTICLES_BY_SCAN: &str = "SELECT id FROM knowledge_articles \
     WHERE org_id = $1 AND published = true \
       AND (search_tsv @@ plainto_tsquery('english', $2) OR title ILIKE $3) \
     ORDER BY ts_rank(search_tsv, plainto_tsquery('english', $2)) DESC, created_at DESC, id \
     LIMIT $4";
const ARTICLES_BY_SCAN_COUNT: &str = "SELECT count(*) FROM knowledge_articles \
     WHERE org_id = $1 AND published = true \
       AND (search_tsv @@ plainto_tsquery('english', $2) OR title ILIKE $3)";

/// `cxp_core::contains_pattern`, which is crate-private.
pub fn like_pattern(q: &str) -> String {
    let mut out = String::from("%");
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// What a search gave, or what the scan gives: the page's ids, the count,
/// and whether the count is capped.
#[derive(Debug, PartialEq)]
pub struct Searched {
    pub ids: Vec<Uuid>,
    pub total: i64,
    pub capped: bool,
}

/// The scan's answer, as search must give it: every match counted up to
/// the cap, `capped` exactly when there are more.
fn as_capped(ids: Vec<Uuid>, count: i64) -> Searched {
    Searched {
        ids,
        total: count.min(TOTAL_CAP),
        capped: count > TOTAL_CAP,
    }
}

/// `search_customers` and the pre-#17 scan, for one query and page, in the
/// same transaction. `None` when search refuses the query as invalid.
pub async fn customers_both_ways(
    tx: &mut otto_tenant::Tx<'_>,
    q: &str,
    page: Page,
) -> Option<(Searched, Searched)> {
    let org = tx.org();
    let found = match tx.search_customers(&permit().await, q, page).await {
        Ok(found) => found,
        Err(Error::Invalid(_)) => return None,
        Err(e) => panic!("{q:?}: {e:?}"),
    };
    let q = q.trim();
    let ids: Vec<Uuid> = sqlx::query_scalar(CUSTOMERS_BY_SCAN)
        .bind(org)
        .bind(like_pattern(q))
        .bind(q)
        .bind(page.limit().min((TOTAL_CAP - page.offset()).max(0)))
        .bind(page.offset())
        .fetch_all(tx.conn())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(CUSTOMERS_BY_SCAN_COUNT)
        .bind(org)
        .bind(like_pattern(q))
        .bind(q)
        .fetch_one(tx.conn())
        .await
        .unwrap();
    Some((
        Searched {
            ids: found.items.iter().map(|c| c.id).collect(),
            total: found.total,
            capped: found.total_capped,
        },
        as_capped(ids, count),
    ))
}

/// `search_articles` and the pre-#17 scan, as [`customers_both_ways`].
pub async fn articles_both_ways(
    tx: &mut otto_tenant::Tx<'_>,
    q: &str,
) -> Option<(Searched, Searched)> {
    let org = tx.org();
    let found = match tx.search_articles(&permit().await, q).await {
        Ok(found) => found,
        Err(Error::Invalid(_)) => return None,
        Err(e) => panic!("{q:?}: {e:?}"),
    };
    let q = q.trim();
    let ids: Vec<Uuid> = sqlx::query_scalar(ARTICLES_BY_SCAN)
        .bind(org)
        .bind(q)
        .bind(like_pattern(q))
        .bind(cxp_core::knowledge::SEARCH_LIMIT)
        .fetch_all(tx.conn())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(ARTICLES_BY_SCAN_COUNT)
        .bind(org)
        .bind(q)
        .bind(like_pattern(q))
        .fetch_one(tx.conn())
        .await
        .unwrap();
    Some((
        Searched {
            ids: found.items.iter().map(|a| a.id).collect(),
            total: found.total,
            capped: found.total_capped,
        },
        as_capped(ids, count),
    ))
}

/// A small deterministic generator (xorshift64*) for the random-corpus
/// tests: the same seed gives the same corpus and queries on every run, so a
/// failure reproduces.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn pick<'a, T: ?Sized>(&mut self, of: &[&'a T]) -> &'a T {
        of[self.below(of.len())]
    }

    /// A substring of `s` of 1..=`max` characters, at a random position.
    pub fn substring(&mut self, s: &str, max: usize) -> String {
        let chars: Vec<char> = s.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        let len = 1 + self.below(max.min(chars.len()));
        let start = self.below(chars.len() - len + 1);
        chars[start..start + len].iter().collect()
    }
}

/// Whether search's answer agrees with the scan's under the contract: when
/// `total_capped` is false, it is identical; when true, `total` is a lower
/// bound of the scan's count and every item is a match (`matches`, the ids
/// of every match the scan finds).
pub fn within_contract(found: &Searched, scanned_count: i64, matches: &[Uuid]) -> bool {
    found.capped
        && found.total <= scanned_count.min(TOTAL_CAP)
        && found.ids.iter().all(|id| matches.contains(id))
}

/// Every customer of this org matching `q` under the pre-#17 predicate.
pub async fn customer_matches(tx: &mut otto_tenant::Tx<'_>, q: &str) -> Vec<Uuid> {
    let q = q.trim();
    sqlx::query_scalar(
        "SELECT id FROM customers \
         WHERE org_id = $1 AND (email ILIKE $2 OR name ILIKE $2 OR name % $3)",
    )
    .bind(tx.org())
    .bind(like_pattern(q))
    .bind(q)
    .fetch_all(tx.conn())
    .await
    .unwrap()
}

/// A search permit from a limiter of this test's own (the server shares
/// one per process; tests run in parallel and must not compete).
pub async fn permit() -> cxp_core::search::SearchPermit {
    cxp_core::search::SearchLimiter::new(1)
        .acquire()
        .await
        .unwrap()
}
