//! Helpers shared by every `crates/cxp-core/tests/*.rs` file. Include with
//! `mod common;`. Each test binary uses a different subset, hence the
//! crate-level `dead_code` allowance.
//!
//! Tests run under `#[sqlx::test(migrator = "cxp_core::MIGRATOR")]`, which
//! hands each test a fresh, migrated database on the cluster `DATABASE_URL`
//! names, connecting as a superuser. A superuser bypasses row-level
//! security, so a plain pool query proves nothing about isolation: tenant
//! work goes through [`db`]'s `Db::begin` (which assumes `otto_app`), and
//! unpinned checks go through [`as_app_unpinned`], [`as_app_maintenance`], or
//! [`otto_app_db`].

#![allow(dead_code)]

use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

/// The test database as a [`Db`]. `Db::begin(org)` on it runs as `otto_app`
/// pinned to `org`, exactly as a request does.
pub fn db(pool: PgPool) -> Db {
    Db::from_pool(pool)
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

/// A `Db` whose every connection runs as `otto_app`, so even unpinned
/// statements are subject to RLS (the test pool's superuser bypasses it).
/// `Db::begin` still pins and works. Close `.pool()` before the test returns
/// so sqlx can drop the test database.
pub async fn otto_app_db(pool: &PgPool) -> Db {
    let app_pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE otto_app").execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    Db::from_pool(app_pool)
}

/// A transaction as `otto_app` with no org pinned, so `current_org()` is
/// NULL. This is what a request path that forgot to pin would see.
pub async fn as_app_unpinned(pool: &PgPool) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE otto_app")
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
