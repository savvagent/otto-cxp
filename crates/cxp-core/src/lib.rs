//! `cxp-core` — otto-cxp's domain: customers and their interactions and
//! segments, support tickets, the knowledge base and chat, feedback and
//! surveys, the experience catalog, the dashboard metrics, and the plumbing a
//! resource server of the otto platform needs (authentication, usage
//! metering, lifecycle events, the web console's sign-in sessions).
//!
//! This is otto-cxp's own database, separate from the platform's. Tenant
//! isolation comes from `otto_tenant::{Db, Tx}`: every tenant query is a
//! method on an [`otto_tenant::Tx`], which cannot exist without an org, and
//! runs under row-level security as `cxp_app` ([`TENANT_ROLE`]). Query
//! methods are extension traits (`customers::CustomersExt`, `tickets::TicketsExt`, ...) because `Tx`
//! is defined in otto-tenant. Request paths open their transactions with
//! `platform_events::begin_live`, never `Db::begin` directly, so a deleted org
//! or removed member is refused.
//!
//! No SQL lives outside this crate.

pub mod analysis;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod console_sessions;
pub mod customers;
pub mod dashboard;
pub mod db_role;
pub mod error;
pub mod feedback;
pub mod interactions;
pub mod knowledge;
mod maintenance;
pub mod migrate;
pub mod paging;
pub mod platform_events;
pub mod scopes;
pub mod sealed;
pub mod search;
pub mod segments;
pub mod tickets;
pub mod usage;

pub use error::{Error, Result};
pub use migrate::{migrate, MIGRATOR};
pub use paging::Page;

/// How long a request waits for a pooled connection before giving up. The
/// server's pool is built with it, and the console's refresh budget counts it.
pub const POOL_ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The role every tenant transaction assumes (`SET LOCAL ROLE`): otto-cxp's own (#28), never
/// the `otto_app` the otto services on a shared instance may share, so a privilege another
/// service grants `otto_app` never reaches a tenant transaction here. A NOLOGIN role holding
/// `otto_cxp`'s tenant grants and nothing else (migration 0007); the runtime login is a member
/// of it (and inherits its grants for the background work it does as itself).
pub const TENANT_ROLE: &str = "cxp_app";

/// The only way to make an [`otto_tenant::Db`] in this workspace: `Db::from_pool` and
/// `Db::connect` default to the shared `otto_app` and are forbidden elsewhere (`clippy.toml`),
/// so production and every test pin [`TENANT_ROLE`] alike.
///
/// otto-tenant skips `SET LOCAL ROLE` when the role is missing or not assumable rather than
/// failing; `cxp-server`'s boot refuses that on every machine (`boot::check_tenant_role`).
pub fn tenant_db(pool: sqlx::PgPool) -> otto_tenant::Db {
    #[allow(clippy::disallowed_methods)] // the one sanctioned constructor
    otto_tenant::Db::from_pool(pool)
        .with_tenant_role(TENANT_ROLE)
        .expect("TENANT_ROLE is a valid role name")
}

/// Whether the database answers a trivial query: the readiness probe. Touches
/// no tenant table, so it needs no org.
pub async fn ping(db: &otto_tenant::Db) -> Result<()> {
    sqlx::query("SELECT 1").execute(db.pool()).await?;
    Ok(())
}

/// A `LIKE`/`ILIKE` pattern matching `q` anywhere, with `q`'s own `\`, `%`,
/// and `_` escaped (Postgres' default `LIKE` escape is `\`).
pub(crate) fn contains_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_tenant_role_is_otto_cxps_own_and_every_db_assumes_it() {
        assert_eq!(super::TENANT_ROLE, "cxp_app");
        assert!(otto_tenant::db::validate_role_name(super::TENANT_ROLE).is_ok());
        assert_ne!(super::TENANT_ROLE, otto_tenant::db::DEFAULT_TENANT_ROLE);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://x@127.0.0.1:1/none")
            .unwrap();
        assert_eq!(super::tenant_db(pool).tenant_role(), super::TENANT_ROLE);
    }

    #[test]
    fn contains_pattern_escapes_wildcards() {
        assert_eq!(super::contains_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
        assert_eq!(super::contains_pattern("plain"), "%plain%");
    }
}
