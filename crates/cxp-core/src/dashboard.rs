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

pub trait DashboardExt {
    /// Counts of customers, tickets (all and open), feedback, surveys, and
    /// experiences.
    fn dashboard_metrics(&mut self) -> impl Future<Output = Result<DashboardMetrics>> + Send;

    /// Ticket counts by status, one aggregate.
    fn ticket_counts_by_status(
        &mut self,
    ) -> impl Future<Output = Result<TicketStatusCounts>> + Send;

    /// The `metrics://dashboard` payload.
    fn dashboard_summary(&mut self) -> impl Future<Output = Result<DashboardSummary>> + Send;
}

impl DashboardExt for Tx<'_> {
    async fn dashboard_metrics(&mut self) -> Result<DashboardMetrics> {
        let org = self.org();
        Ok(sqlx::query_as(
            "SELECT \
               (SELECT count(*) FROM customers   WHERE org_id = $1) AS total_customers, \
               (SELECT count(*) FROM tickets     WHERE org_id = $1) AS total_tickets, \
               (SELECT count(*) FROM tickets     WHERE org_id = $1 AND status = 'open') AS open_tickets, \
               (SELECT count(*) FROM feedback    WHERE org_id = $1) AS total_feedback, \
               (SELECT count(*) FROM surveys     WHERE org_id = $1) AS total_surveys, \
               (SELECT count(*) FROM experiences WHERE org_id = $1) AS total_experiences",
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
            "SELECT \
               (SELECT count(*) FROM tickets WHERE org_id = $1) AS total_tickets, \
               (SELECT count(*) FROM tickets WHERE org_id = $1 AND status = 'open') AS open_tickets, \
               (SELECT count(*) FROM tickets WHERE org_id = $1 AND status = 'in_progress') AS in_progress_tickets, \
               (SELECT count(*) FROM tickets WHERE org_id = $1 AND status = 'waiting') AS waiting_tickets, \
               (SELECT count(*) FROM tickets WHERE org_id = $1 AND status = 'resolved') AS resolved_tickets, \
               (SELECT count(*) FROM tickets WHERE org_id = $1 AND status = 'closed') AS closed_tickets, \
               (SELECT (100.0 * count(*) FILTER (WHERE rating >= 4) / NULLIF(count(rating), 0))::float8 \
                  FROM feedback WHERE org_id = $1) AS csat_score, \
               (SELECT avg(rating)::float8 FROM feedback WHERE org_id = $1) AS average_rating",
        )
        .bind(org)
        .fetch_one(self.conn())
        .await?)
    }
}
