//! `cxp-core` — otto-cxp's domain: customers and their interactions and
//! segments, support tickets, the knowledge base and chat, feedback and
//! surveys, the experience catalog, the dashboard metrics, and the plumbing a
//! resource server of the otto platform needs (authentication, usage
//! metering, lifecycle events).
//!
//! This is otto-cxp's own database, separate from the platform's. Tenant
//! isolation comes from `otto_tenant::{Db, Tx}`: every tenant query is a
//! method on an [`otto_tenant::Tx`], which cannot exist without an org, and
//! runs under row-level security as `otto_app`. Query methods are extension
//! traits (`customers::CustomersExt`, `tickets::TicketsExt`, ...) because `Tx`
//! is defined in otto-tenant. Request paths open their transactions with
//! `platform_events::begin_live`, never `Db::begin` directly, so a deleted org
//! or removed member is refused.
//!
//! No SQL lives outside this crate.

pub mod analysis;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod customers;
pub mod dashboard;
pub mod error;
pub mod feedback;
pub mod interactions;
pub mod knowledge;
mod maintenance;
pub mod migrate;
pub mod paging;
pub mod platform_events;
pub mod scopes;
pub mod segments;
pub mod tickets;
pub mod usage;

pub use error::{Error, Result};
pub use migrate::{migrate, MIGRATOR};
pub use paging::Page;

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
    #[test]
    fn contains_pattern_escapes_wildcards() {
        assert_eq!(super::contains_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
        assert_eq!(super::contains_pattern("plain"), "%plain%");
    }
}
