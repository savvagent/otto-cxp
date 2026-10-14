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
