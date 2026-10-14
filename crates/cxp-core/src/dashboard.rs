//! Dashboard metrics (story S8): `DashboardExt` on `otto_tenant::Tx`,
//! computed as SQL aggregates, with its model structs.
//!
//! Nothing here lists rows in Rust: each method is one statement over the
//! caller's organization.

use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::future::Future;

use crate::error::Result;

/// Headline counts for the REST dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardMetrics {
    pub total_customers: i64,
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub total_feedback: i64,
    pub total_surveys: i64,
    pub total_experiences: i64,
}

/// Tickets by status. `total` is the sum of the five statuses.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketStatusCounts {
    pub total: i64,
    pub open: i64,
    pub in_progress: i64,
    pub waiting: i64,
    pub resolved: i64,
    pub closed: i64,
}

/// What `metrics://dashboard` reports: ticket counts by status and
/// satisfaction over every rated feedback row.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub total_tickets: i64,
    pub open_tickets: i64,
    pub in_progress_tickets: i64,
    pub waiting_tickets: i64,
    pub resolved_tickets: i64,
    pub closed_tickets: i64,
    /// Percent of ratings that are 4 or 5; `None` with no ratings.
    pub csat_score: Option<f64>,
    /// Mean rating; `None` with no ratings.
    pub average_rating: Option<f64>,
}

/// Dashboard aggregates for one organization's transaction. Each method is one
/// statement that scans each table once; none can fail except on a database
/// error, and an organization with no rows gets zeros (and `None` ratings).
pub trait DashboardExt {
    /// Counts of customers, tickets (all and open), feedback, surveys, and
    /// experiences.
    fn dashboard_metrics(&mut self) -> impl Future<Output = Result<DashboardMetrics>> + Send;

    /// Ticket counts by status, one aggregate.
    fn ticket_counts_by_status(
        &mut self,
    ) -> impl Future<Output = Result<TicketStatusCounts>> + Send;

    /// The `metrics://dashboard` payload: ticket counts by status, and CSAT
    /// and mean rating over every rated feedback row (unrated rows are ignored).
    fn dashboard_summary(&mut self) -> impl Future<Output = Result<DashboardSummary>> + Send;
}

impl DashboardExt for Tx<'_> {
    async fn dashboard_metrics(&mut self) -> Result<DashboardMetrics> {
        let org = self.org();
        Ok(sqlx::query_as(
            "SELECT c.n AS total_customers, t.total AS total_tickets, t.open AS open_tickets, \
                    f.n AS total_feedback, s.n AS total_surveys, e.n AS total_experiences \
             FROM (SELECT count(*) AS n FROM customers WHERE org_id = $1) c, \
                  (SELECT count(*) AS total, count(*) FILTER (WHERE status = 'open') AS open \
                     FROM tickets WHERE org_id = $1) t, \
                  (SELECT count(*) AS n FROM feedback WHERE org_id = $1) f, \
                  (SELECT count(*) AS n FROM surveys WHERE org_id = $1) s, \
                  (SELECT count(*) AS n FROM experiences WHERE org_id = $1) e",
        )
        .bind(org)
        .fetch_one(self.conn())
        .await?)
    }

    async fn ticket_counts_by_status(&mut self) -> Result<TicketStatusCounts> {
        let org = self.org();
        Ok(sqlx::query_as(
            "SELECT count(*) AS total, \
               count(*) FILTER (WHERE status = 'open') AS open, \
               count(*) FILTER (WHERE status = 'in_progress') AS in_progress, \
               count(*) FILTER (WHERE status = 'waiting') AS waiting, \
               count(*) FILTER (WHERE status = 'resolved') AS resolved, \
               count(*) FILTER (WHERE status = 'closed') AS closed \
             FROM tickets WHERE org_id = $1",
        )
        .bind(org)
        .fetch_one(self.conn())
        .await?)
    }

    async fn dashboard_summary(&mut self) -> Result<DashboardSummary> {
        let org = self.org();
        Ok(sqlx::query_as(
            "SELECT t.total AS total_tickets, t.open AS open_tickets, \
                    t.in_progress AS in_progress_tickets, t.waiting AS waiting_tickets, \
                    t.resolved AS resolved_tickets, t.closed AS closed_tickets, \
                    f.csat AS csat_score, f.avg AS average_rating \
             FROM (SELECT count(*) AS total, \
                          count(*) FILTER (WHERE status = 'open') AS open, \
                          count(*) FILTER (WHERE status = 'in_progress') AS in_progress, \
                          count(*) FILTER (WHERE status = 'waiting') AS waiting, \
                          count(*) FILTER (WHERE status = 'resolved') AS resolved, \
                          count(*) FILTER (WHERE status = 'closed') AS closed \
                     FROM tickets WHERE org_id = $1) t, \
                  (SELECT (100.0 * count(*) FILTER (WHERE rating >= 4) \
                             / NULLIF(count(rating), 0))::float8 AS csat, \
                          avg(rating)::float8 AS avg \
                     FROM feedback WHERE org_id = $1) f",
        )
        .bind(org)
        .fetch_one(self.conn())
        .await?)
    }
}
