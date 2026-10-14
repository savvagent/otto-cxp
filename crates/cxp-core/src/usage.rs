//! Usage metering: the `BILLABLE` tools, the `Meter` that charges
//! one `usage_outbox` row in the tool's own transaction (with optional quota
//! enforcement) and reports the org's usage, and the shipper (`ship_once`,
//! `run`) that drains the outbox to the platform.
//!
//! - **Recording is local and atomic.** [`Meter::charge`] writes a
//!   `usage_outbox` row in the tool's own transaction, so a failed call rolls
//!   its record back and a successful one is recorded even while the platform
//!   is down.
//! - **Shipping is a background task** ([`run`]) that posts rows to the
//!   platform and deletes what it answered for. The platform dedupes on the
//!   row's `event_id`, so retries are free.
//! - **Nothing retries forever.** A row the platform refuses (in its receipt,
//!   or by refusing the whole batch with a 4xx about the batch itself), or one
//!   still unshipped after [`MAX_SHIP_ATTEMPTS`] attempts, moves to
//!   `usage_outbox_rejected` with the reason, logged at `error`. The sweep
//!   forgets those after `platform_events::REJECTED_USAGE_KEEP_DAYS`.
//! - **Enforcing** reads the platform's cached usage status (and counts this
//!   org's unshipped rows from the platform's current billing period on top).
//!   Off by default (`CXP_ENFORCE_QUOTAS`). If the platform cannot answer, the
//!   call is allowed and still recorded.
//!
//! Only the four [`BILLABLE`] tools are recorded. Everything else is free and
//! records nothing; [`Meter::charge`] refuses any other name as an internal
//! error, so a free tool cannot be metered by mistake.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use otto_resource::{PlatformClient, UsageEvent, UsageStatus};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::{Db, Tx};
use serde::Serialize;
use sqlx::FromRow;
use tokio::sync::watch;
use uuid::Uuid;

use crate::error::{Error, Result};

/// The tools that consume the plan's allowance, and the only ones recorded.
pub const BILLABLE: &[&str] = &[
    "submit_feedback",
    "create_survey",
    "send_survey",
    "submit_survey_response",
];

pub fn is_billable(tool: &str) -> bool {
    BILLABLE.contains(&tool)
}

const QUOTA_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const FAIL_OPEN_FOR: Duration = Duration::from_secs(5);
/// The share of the allowance at or past which [`Status::warning`] is set.
pub const WARN_AT: f64 = 0.8;
/// How many times a usage row is offered to the platform before it is given
/// up on and moved to `usage_outbox_rejected`. With the default backoff
/// (5 s doubling to 15 min) that is roughly three and a half hours of
/// failures.
pub const MAX_SHIP_ATTEMPTS: i32 = 20;

#[derive(Clone)]
pub struct Meter {
    platform: Arc<PlatformClient>,
    fail_open: Arc<Mutex<HashMap<OrgId, Instant>>>,
    pub enforce: bool,
    pub upgrade_url: String,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter")
            .field("enforce", &self.enforce)
            .finish_non_exhaustive()
    }
}

impl Meter {
    pub fn new(
        platform: Arc<PlatformClient>,
        enforce: bool,
        upgrade_url: impl Into<String>,
    ) -> Self {
        Self {
            platform,
            fail_open: Arc::default(),
            enforce,
            upgrade_url: upgrade_url.into(),
        }
    }

    /// Look the org's standing up before a transaction opens, so a slow
    /// platform never holds a pooled connection. A no-op unless enforcing.
    pub async fn warm(&self, org: OrgId) {
        if self.enforce {
            let _ = self.status_for(org).await;
        }
    }

    /// Record one call of a [`BILLABLE`] tool in `tx`, or refuse it if the org
    /// is out of budget. Any other tool name is a programming error in the
    /// caller, not the agent's request: it is refused with [`Error::Internal`]
    /// without recording anything.
    ///
    /// The quota is soft at the boundary: concurrent calls can each see the
    /// same count before either commits, so an org can end a few calls over.
    pub async fn charge(&self, tx: &mut Tx<'_>, user: UserId, tool: &str) -> Result<()> {
        if !is_billable(tool) {
            tracing::error!(tool, "refusing to meter a tool that is not billable");
            return Err(Error::Internal(format!(
                "{tool} is not a billable tool; only {} are metered",
                BILLABLE.join(", ")
            )));
        }
        let org = tx.org();
        if self.enforce {
            if let Some(mut status) = self.status_for(org).await {
                // Only rows from the platform's current period: a row from an
                // earlier one, still unshipped, is not this period's usage.
                let period_start = status
                    .period_start
                    .and_hms_opt(0, 0, 0)
                    .unwrap_or_default()
                    .and_utc();
                let unshipped: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM usage_outbox \
                     WHERE org_id = $1 AND billable AND occurred_at >= $2",
                )
                .bind(org)
                .bind(period_start)
                .fetch_one(tx.conn())
                .await?;
                status.billable_count += unshipped;
                if status.is_blocked() {
                    return Err(Error::QuotaExceeded {
                        tool: tool.to_string(),
                        used: status.billable_count,
                        included: status.included_ops,
                        plan: plan_name(&status.plan),
                        upgrade_url: self.upgrade_url.clone(),
                    });
                }
            }
        }
        sqlx::query(
            "INSERT INTO usage_outbox (org_id, user_id, tool, billable) VALUES ($1, $2, $3, true)",
        )
        .bind(org)
        .bind(user)
        .bind(tool)
        .execute(tx.conn())
        .await?;
        Ok(())
    }

    async fn status_for(&self, org: OrgId) -> Option<UsageStatus> {
        // The map holds only deadlines, valid whatever a panicking holder
        // was doing, so a poisoned lock is used as is.
        let until = self
            .fail_open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&org)
            .copied();
        if let Some(until) = until {
            if until > Instant::now() {
                return None;
            }
        }
        let failure = match tokio::time::timeout(
            QUOTA_LOOKUP_TIMEOUT,
            self.platform.usage_status(org.as_uuid()),
        )
        .await
        {
            Ok(Ok(status)) => {
                self.fail_open
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&org);
                return Some(status);
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "timed out".to_string(),
        };
        tracing::warn!(org = %org, reason = %failure,
            "could not read usage from the platform; allowing billable calls for {FAIL_OPEN_FOR:?}");
        let mut m = self
            .fail_open
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Prune expired entries here, so the map holds only orgs inside
        // their fail-open window rather than every org that ever failed.
        let now = Instant::now();
        m.retain(|_, until| *until > now);
        m.insert(org, now + FAIL_OPEN_FOR);
        None
    }

    /// The org's standing, for the free `whoami` and `usage` tools.
    pub async fn report(&self, org: OrgId) -> Result<Status> {
        let usage = self
            .platform
            .usage_status(org.as_uuid())
            .await
            .map_err(|e| Error::platform_for(org, e))?;
        Ok(Status::new(usage, self.enforce))
    }
}

pub(crate) fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Where an org stands against its monthly allowance (shared by every otto-*
/// service the org uses).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub plan: String,
    pub included_ops: i64,
    pub billable_used: i64,
    pub remaining: i64,
    pub total_calls: i64,
    pub period_start: chrono::NaiveDate,
    /// True once 80 % of the allowance ([`WARN_AT`]) is used, and always on a
    /// plan that includes none. Worth telling a human.
    pub warning: bool,
    pub hard_stop: bool,
    /// Whether this server refuses billable calls over the allowance at all.
    pub enforced: bool,
}

impl Status {
    fn new(u: UsageStatus, enforced: bool) -> Self {
        let remaining = u.included_ops.saturating_sub(u.billable_count).max(0);
        let warning =
            u.included_ops <= 0 || u.billable_count as f64 >= u.included_ops as f64 * WARN_AT;
        Self {
            plan: plan_name(&u.plan),
            included_ops: u.included_ops,
            billable_used: u.billable_count,
            remaining,
            total_calls: u.total_count,
            period_start: u.period_start,
            warning,
            hard_stop: u.hard_stop,
            enforced,
        }
    }
}

// --------------------------------------------------------------------- shipper

#[derive(Debug, Clone)]
pub struct ShipperConfig {
    pub batch: usize,
    pub poll: Duration,
    pub claim_lease: Duration,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for ShipperConfig {
    fn default() -> Self {
        Self {
            batch: otto_resource::MAX_USAGE_BATCH,
            poll: Duration::from_secs(5),
            claim_lease: Duration::from_secs(120),
            base_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(15 * 60),
        }
    }
}

#[derive(Debug, FromRow)]
struct Row {
    id: i64,
    event_id: Uuid,
    org_id: Uuid,
    user_id: Option<Uuid>,
    tool: String,
    billable: bool,
    occurred_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    #[error("the platform did not accept the batch: {0}")]
    Platform(#[from] otto_resource::Error),
    #[error("outbox database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Claim a batch of due rows, ship them, delete what the platform answered
/// for. Returns how many rows were claimed. Never loses a row: on a failure
/// that can pass nothing is deleted and the rows are rescheduled with
/// backoff. Rows the platform refused (in its receipt, or by refusing the
/// whole batch), and rows that have failed [`MAX_SHIP_ATTEMPTS`] times, move
/// to `usage_outbox_rejected` with the reason.
///
/// A receipt is trusted only if it accounts for every event exactly once:
/// its counts add up to the batch, and its rejected ids are distinct and all
/// in the batch. Anything else keeps every row and retries the batch.
///
/// Runs unpinned across every org, in maintenance transactions
/// (`crate::maintenance`).
pub async fn ship_once(
    db: &Db,
    platform: &PlatformClient,
    cfg: &ShipperConfig,
) -> std::result::Result<usize, ShipError> {
    let mut claim = crate::maintenance::begin(db).await?;
    let mut rows: Vec<Row> = sqlx::query_as(
        "UPDATE usage_outbox SET attempts = attempts + 1, \
                next_attempt_at = now() + make_interval(secs => $2) \
         WHERE id IN ( \
             SELECT id FROM usage_outbox WHERE next_attempt_at <= now() \
             ORDER BY next_attempt_at, id LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, event_id, org_id, user_id, tool, billable, occurred_at",
    )
    .bind(cfg.batch as i64)
    .bind(cfg.claim_lease.as_secs_f64())
    .fetch_all(&mut *claim)
    .await?;
    // Committed before shipping, so the claim lease holds the rows back from
    // a concurrent shipper while the platform call is in flight.
    claim.commit().await?;
    if rows.is_empty() {
        return Ok(0);
    }
    rows.sort_by_key(|r| r.id);
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let events: Vec<UsageEvent> = rows
        .iter()
        .map(|r| UsageEvent {
            event_id: r.event_id,
            org_id: r.org_id,
            user_id: r.user_id,
            tool: r.tool.clone(),
            billable: r.billable,
            occurred_at: r.occurred_at,
        })
        .collect();

    let receipt = match platform.ship_usage(&events).await {
        Ok(r) => r,
        Err(e) if refuses_batch(&e) => {
            let reason = format!("the platform refused the batch: {e}");
            tracing::error!(rows = ids.len(), error = %e,
                "the platform refused a usage batch outright; moving it to usage_outbox_rejected");
            let mut tx = crate::maintenance::begin(db).await?;
            move_to_rejected(&mut tx, &ids, &reason).await?;
            tx.commit().await?;
            return Err(e.into());
        }
        Err(e) => {
            reschedule(db, &ids, &e.to_string(), cfg).await;
            return Err(e.into());
        }
    };
    if let Err(msg) = check_receipt(&receipt, &events) {
        tracing::error!("{msg}; keeping every row and retrying the batch");
        reschedule(db, &ids, &msg, cfg).await;
        return Err(otto_resource::Error::Decode(msg).into());
    }

    let mut tx = crate::maintenance::begin(db).await?;
    for r in &receipt.rejected {
        tracing::error!(event_id = %r.event_id, reason = %r.reason,
            "the platform refused a usage event; moving it to usage_outbox_rejected");
        let moved = sqlx::query(&format!(
            "{MOVE_TO_REJECTED} AND event_id = $3 {INSERT_REJECTED} left($2, 500) FROM moved"
        ))
        .bind(&ids)
        .bind(&r.reason)
        .bind(r.event_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if moved != 1 {
            // Validated above, so only a row that changed under the claim
            // lease gets here. Roll back, so nothing is deleted on a receipt
            // this pass could not match up.
            drop(tx);
            let msg = format!(
                "moving rejected usage event {} affected {moved} rows, not 1",
                r.event_id
            );
            tracing::error!("{msg}; keeping every row and retrying the batch");
            reschedule(db, &ids, &msg, cfg).await;
            return Err(otto_resource::Error::Decode(msg).into());
        }
    }
    sqlx::query("DELETE FROM usage_outbox WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ids.len())
}

/// The start of a statement moving claimed rows (`$1`, ids) into
/// `usage_outbox_rejected`: a caller may narrow the `WHERE`, then appends
/// [`INSERT_REJECTED`] and the reason expression.
const MOVE_TO_REJECTED: &str = "WITH moved AS ( \
       DELETE FROM usage_outbox WHERE id = ANY($1)";
const INSERT_REJECTED: &str = "RETURNING event_id, org_id, user_id, tool, billable, occurred_at, \
       attempts) \
     INSERT INTO usage_outbox_rejected \
       (event_id, org_id, user_id, tool, billable, occurred_at, reason) \
     SELECT event_id, org_id, user_id, tool, billable, occurred_at,";

/// Move every row of `ids` to `usage_outbox_rejected` with `reason`.
async fn move_to_rejected(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    ids: &[i64],
    reason: &str,
) -> sqlx::Result<u64> {
    Ok(sqlx::query(&format!(
        "{MOVE_TO_REJECTED} {INSERT_REJECTED} left($2, 500) FROM moved"
    ))
    .bind(ids)
    .bind(reason)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Whether the platform refused the batch itself, so that resending it can
/// never succeed: a 4xx about the request. Not this service's credential
/// (401/403), a wrong URL (404), a timeout (408), or rate limiting (429):
/// those are fixed on this side or pass, and the batch is fine.
fn refuses_batch(e: &otto_resource::Error) -> bool {
    matches!(e, otto_resource::Error::Status { status, .. }
        if (400..500).contains(status) && ![401, 403, 404, 408, 429].contains(status))
}

/// Whether `receipt` accounts for every event of `batch` exactly once.
fn check_receipt(
    receipt: &otto_resource::UsageReceipt,
    batch: &[UsageEvent],
) -> std::result::Result<(), String> {
    let answered = receipt.accepted as usize + receipt.duplicates as usize + receipt.rejected.len();
    if answered != batch.len() {
        return Err(format!(
            "the platform accounted for {answered} of {} usage events",
            batch.len()
        ));
    }
    let shipped: HashSet<Uuid> = batch.iter().map(|e| e.event_id).collect();
    let mut seen = HashSet::new();
    for r in &receipt.rejected {
        if !shipped.contains(&r.event_id) {
            return Err(format!(
                "the platform rejected usage event {}, which was not in the batch",
                r.event_id
            ));
        }
        if !seen.insert(r.event_id) {
            return Err(format!(
                "the platform rejected usage event {} more than once",
                r.event_id
            ));
        }
    }
    Ok(())
}

async fn reschedule(db: &Db, ids: &[i64], error: &str, cfg: &ShipperConfig) {
    if let Err(e) = try_reschedule(db, ids, error, cfg).await {
        // The claim lease still holds the rows back; this only delays a retry.
        tracing::warn!(error = %e, "could not record a usage shipping failure");
    }
}

/// Back the rows off, then give up on any that have now failed
/// [`MAX_SHIP_ATTEMPTS`] times.
async fn try_reschedule(
    db: &Db,
    ids: &[i64],
    error: &str,
    cfg: &ShipperConfig,
) -> sqlx::Result<()> {
    let error: String = error.chars().take(400).collect();
    let mut tx = crate::maintenance::begin(db).await?;
    sqlx::query(
        "UPDATE usage_outbox SET last_error = $2, \
                next_attempt_at = now() + make_interval(secs => \
                    LEAST($3::float8 * power(2, LEAST(attempts - 1, 20)), $4::float8)) \
         WHERE id = ANY($1)",
    )
    .bind(ids)
    .bind(&error)
    .bind(cfg.base_backoff.as_secs_f64())
    .bind(cfg.max_backoff.as_secs_f64())
    .execute(&mut *tx)
    .await?;
    let gave_up = sqlx::query(&format!(
        "{MOVE_TO_REJECTED} AND attempts >= $3 {INSERT_REJECTED} \
         'not shipped after ' || attempts || ' attempts; last error: ' || $2 FROM moved"
    ))
    .bind(ids)
    .bind(&error)
    .bind(MAX_SHIP_ATTEMPTS)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    if gave_up > 0 {
        tracing::error!(rows = gave_up, attempts = MAX_SHIP_ATTEMPTS, last_error = %error,
            "gave up shipping usage; moved to usage_outbox_rejected");
    }
    Ok(())
}

/// Drain the outbox until `shutdown` flips, with a final pass on the way out.
/// Returns that final pass's failure, if it failed (the rows stay in the
/// outbox for the next start), so the process can exit non-zero.
pub async fn run(
    db: Db,
    platform: Arc<PlatformClient>,
    cfg: ShipperConfig,
    mut shutdown: watch::Receiver<bool>,
) -> std::result::Result<(), ShipError> {
    let mut failures: u32 = 0;
    loop {
        let wait = match ship_once(&db, &platform, &cfg).await {
            Ok(n) => {
                failures = 0;
                if n >= cfg.batch {
                    Duration::ZERO
                } else {
                    cfg.poll
                }
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let wait = cfg
                    .poll
                    .saturating_mul(1u32 << failures.min(6))
                    .min(Duration::from_secs(60));
                match &e {
                    ShipError::Platform(otto_resource::Error::Unauthorized) => tracing::error!(
                        "the platform rejected this resource server's credential; usage is \
                         accumulating in the outbox. Check CXP_INTROSPECTION_SECRET"
                    ),
                    other => {
                        tracing::warn!(error = %other, retry_in = ?wait, "usage shipping failed")
                    }
                }
                wait
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                return match ship_once(&db, &platform, &cfg).await {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        tracing::error!(error = %e,
                            "final usage flush failed; rows stay in the outbox");
                        Err(e)
                    }
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_the_four_writes_are_billable() {
        assert_eq!(
            BILLABLE,
            [
                "submit_feedback",
                "create_survey",
                "send_survey",
                "submit_survey_response"
            ]
        );
        for t in [
            "whoami",
            "usage",
            "create_customer",
            "create_ticket",
            "list_feedback",
            "get_survey",
            "search_knowledge",
        ] {
            assert!(!is_billable(t), "{t}");
        }
    }

    #[test]
    fn plan_names_are_capitalized() {
        assert_eq!(plan_name("free"), "Free");
        assert_eq!(plan_name(""), "");
    }

    fn usage(billable: i64, included: i64) -> UsageStatus {
        UsageStatus {
            org_id: Uuid::nil(),
            plan: "team".into(),
            period_start: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            billable_count: billable,
            total_count: billable + 3,
            included_ops: included,
            hard_stop: false,
            features: Default::default(),
        }
    }

    #[test]
    fn status_warns_past_eighty_percent_and_never_goes_negative() {
        let s = Status::new(usage(79, 100), false);
        assert!(!s.warning);
        assert_eq!(s.remaining, 21);
        let s = Status::new(usage(80, 100), true);
        assert!(s.warning && s.enforced);
        let s = Status::new(usage(150, 100), false);
        assert_eq!(s.remaining, 0);
        assert_eq!(s.plan, "Team");
        assert_eq!(s.total_calls, 153);
    }
}
