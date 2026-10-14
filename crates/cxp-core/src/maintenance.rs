//! Unpinned, cross-org background access (story S4): `begin` opens a
//! transaction with no org pinned and `app.maintenance = 'on'` set, the marker
//! the retention policies on `platform_events` require. The only way
//! background code reaches more than one org's rows.
//!
//! The unpinned row-level policies on `platform_events`
//! (`platform_events_retention_read`, `platform_events_maintenance_delete`)
//! grant access only when no org is pinned **and** the transaction carries the
//! marker. Unpinned alone is not enough: a request path that forgot to pin an
//! org is unpinned too, and must see nothing.
//!
//! **Only background maintenance code may call [`begin`]**: the retention
//! sweep ([`crate::platform_events::sweep`]). Never a request handler or a
//! tool, which always go through a pinned transaction
//! ([`crate::platform_events::begin_live`]). It is `pub(crate)` so nothing
//! outside this crate can reach it.

use otto_tenant::Db;
use sqlx::{Postgres, Transaction};

use crate::error::Result;

/// Open an unpinned transaction on `db`'s pool with the maintenance marker
/// set for its duration (`SET LOCAL`, so it ends with the transaction and
/// never leaks to the next user of the connection).
pub(crate) async fn begin(db: &Db) -> Result<Transaction<'static, Postgres>> {
    let mut tx = db.pool().begin().await?;
    sqlx::query("SET LOCAL app.maintenance = 'on'")
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
