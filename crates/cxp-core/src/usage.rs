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
//! - **Only the rows' own faults give a row up.** A row the platform refuses
//!   in its receipt moves to `usage_outbox_rejected` with the reason, logged
//!   at `error`. A whole-batch 4xx refusal is split in halves and resent
//!   until the refused events stand alone (within half the claim lease and
//!   `split_budget` requests per pass); a row refused alone, or one whose
//!   batch came back with a receipt that does not account for it, has that
//!   counted against it, and is moved aside after [`MAX_ROW_FAILURES`] such
//!   failures. When every event sent alone is refused and none is accepted,
//!   the refusal is about the requests, not the rows, and counts against
//!   none of them. A failure that is not about the rows (the
//!   platform unreachable, a 5xx, 408, or 429, this service's credential
//!   refused, the org or URL unknown) keeps every row in the outbox, retried
//!   with backoff for as long as it lasts and still counted toward its
//!   period's quota. The sweep forgets rejected rows after
//!   `platform_events::REJECTED_USAGE_KEEP_DAYS`.
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
/// How many failures about a usage row itself (see
/// [`ShipError::faults_the_rows`]) it survives before it is given up on and
/// moved to `usage_outbox_rejected`. Failures that are not about the rows
/// never count: those rows are retried, with backoff, until they ship.
pub const MAX_ROW_FAILURES: i32 = 20;

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
    /// The most requests one pass sends while splitting a refused batch.
    /// Whatever is still unsent then is kept for the next pass, as it is
    /// once half the claim lease has gone.
    pub split_budget: usize,
}

impl Default for ShipperConfig {
    fn default() -> Self {
        Self {
            batch: otto_resource::MAX_USAGE_BATCH,
            poll: Duration::from_secs(5),
            claim_lease: Duration::from_secs(120),
            base_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(15 * 60),
            split_budget: 64,
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
    /// The platform answered with a receipt that does not account for every
    /// event of the batch exactly once, so nothing in it was trusted.
    #[error("the platform's usage receipt was not trusted: {0}")]
    Receipt(String),
    /// A claimed row changed while the platform call was in flight (its
    /// claim lease ran out and another shipper took it). Nothing was deleted.
    #[error("the usage outbox changed under a claim: {0}")]
    Claim(String),
    /// A pass ran out of time (half its claim lease) or of requests while
    /// splitting a refused batch. The rows it did not get to were not sent.
    #[error("a usage pass stopped splitting a refused batch before sending every piece: {0}")]
    Unfinished(String),
    /// The platform refused every event of a split batch on its own, and
    /// accepted none: the refusal is about the requests (a contract change, a
    /// gateway refusing everything), not about any one event.
    #[error("the platform refused every event of a usage batch: {0}")]
    RefusedAll(otto_resource::Error),
    #[error("outbox database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl ShipError {
    /// Whether the failure passes without anyone acting: the platform
    /// unreachable, failing (5xx), timing out (408), or rate limiting (429),
    /// or a claim race between shippers. Anything else (this service's
    /// credential refused, an org or URL the platform does not know, an
    /// unreadable answer or receipt, the outbox database failing) needs an
    /// operator. Either way the rows stay in the outbox.
    pub fn is_retriable(&self) -> bool {
        match self {
            ShipError::Platform(e) => {
                e.is_retriable() || matches!(e, otto_resource::Error::Status { status: 408, .. })
            }
            ShipError::Claim(_) | ShipError::Unfinished(_) => true,
            ShipError::Receipt(_) | ShipError::RefusedAll(_) | ShipError::Db(_) => false,
        }
    }

    /// Whether the failure is about the rows themselves, and so counts toward
    /// [`MAX_ROW_FAILURES`]: a receipt that does not account for them, or a
    /// 4xx refusal of them. Never a transport failure, a 5xx, 408, or 429,
    /// this service's credential refused (401/403), an org or URL the
    /// platform does not know (404, `NotFound`, `InvalidUrl`), a refusal of
    /// every event alike, or a pass that ran out of time: rows waiting on
    /// those are retried until they ship.
    pub fn faults_the_rows(&self) -> bool {
        match self {
            ShipError::Receipt(_) => true,
            ShipError::Platform(e) => refuses_batch(e),
            ShipError::Claim(_)
            | ShipError::Unfinished(_)
            | ShipError::RefusedAll(_)
            | ShipError::Db(_) => false,
        }
    }
}

/// Claim a batch of due rows, ship them, delete what the platform answered
/// for. Returns how many rows were claimed. Never loses a row: whatever was
/// not answered for is rescheduled with backoff.
///
/// - Events the platform refuses in its receipt move to
///   `usage_outbox_rejected` with its reason.
/// - A batch it refuses whole (a 4xx about the request, 413 included; see
///   `refuses_batch`) is split in halves and each half resent, recursively,
///   so one bad event cannot take the rest down with it. An event still
///   refused when sent alone is kept, and the refusal counts against it, so
///   it moves to `usage_outbox_rejected` only after [`MAX_ROW_FAILURES`] of
///   them. If every event sent alone was refused and nothing was accepted,
///   the refusal is about the requests, not the events: the whole batch is
///   kept without counting it against any row ([`ShipError::RefusedAll`]).
/// - Splitting stops once half the claim lease has gone or
///   `split_budget` requests have been sent; the pieces not yet sent are
///   kept for the next pass ([`ShipError::Unfinished`]), not counted
///   against the rows.
/// - Any other failure keeps the rows it covers, and every piece not yet
///   sent, for the next pass. Only failures about the rows themselves
///   ([`ShipError::faults_the_rows`]) count toward [`MAX_ROW_FAILURES`].
///
/// A receipt is trusted only if it accounts for every event exactly once:
/// its counts add up to the batch, and its rejected ids are distinct and all
/// in the batch. Anything else keeps every row of that batch and retries it.
///
/// Returns the first failure, if any piece failed, after settling the rest.
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
            service: None,
            meter: None,
            quantity: None,
        })
        .collect();

    // Rows the platform answered for (to delete), and which of those it
    // refused, with its reason (to move aside).
    let mut answered: Vec<i64> = Vec::with_capacity(ids.len());
    let mut refused: Vec<(Uuid, String)> = Vec::new();
    // Rows to keep and retry, with why.
    let mut failed: Vec<(Vec<i64>, ShipError)> = Vec::new();
    // Rows the platform refused when sent alone, with its refusal.
    let mut alone_refused: Vec<(i64, otto_resource::Error)> = Vec::new();
    // Whether the platform answered any request of this pass with a receipt.
    let mut any_receipt = false;
    // Ranges of `events` still to send, the next one last.
    let mut pending: Vec<std::ops::Range<usize>> = Vec::new();
    pending.push(0..events.len());
    let started = Instant::now();
    let deadline = cfg.claim_lease / 2;
    let mut requests = 0usize;
    while let Some(range) = pending.pop() {
        if requests > 0 && (requests >= cfg.split_budget || started.elapsed() >= deadline) {
            pending.push(range);
            let mut kept = Vec::new();
            for rest in pending.drain(..) {
                kept.extend_from_slice(&ids[rest]);
            }
            let msg = format!(
                "sent {requests} requests in {:?} (budget {} requests, {:?}); {} of {} events \
                 answered, {} kept for the next pass",
                started.elapsed(),
                cfg.split_budget,
                deadline,
                answered.len(),
                ids.len(),
                kept.len()
            );
            tracing::warn!("stopped splitting a refused usage batch: {msg}");
            failed.push((kept, ShipError::Unfinished(msg)));
            break;
        }
        requests += 1;
        let batch = &events[range.clone()];
        match platform.ship_usage(batch).await {
            Ok(receipt) => {
                any_receipt = true;
                match check_receipt(&receipt, batch) {
                    Ok(()) => {
                        answered.extend_from_slice(&ids[range]);
                        refused
                            .extend(receipt.rejected.into_iter().map(|r| (r.event_id, r.reason)));
                    }
                    Err(msg) => {
                        tracing::error!("{msg}; keeping every row of that batch and retrying it");
                        failed.push((ids[range].to_vec(), ShipError::Receipt(msg)));
                    }
                }
            }
            Err(e) if refuses_batch(&e) && batch.len() > 1 => {
                tracing::warn!(events = batch.len(), error = %e,
                    "the platform refused a usage batch; resending it in halves to find the \
                     events it refuses");
                let mid = range.start + batch.len() / 2;
                pending.push(mid..range.end);
                pending.push(range.start..mid);
            }
            Err(e) if refuses_batch(&e) => alone_refused.push((ids[range.start], e)),
            Err(e) => {
                // Every piece not yet sent would meet the same platform: keep
                // them all for the next pass rather than wait out a failure
                // per piece.
                let mut kept = ids[range].to_vec();
                for rest in pending.drain(..) {
                    kept.extend_from_slice(&ids[rest]);
                }
                failed.push((kept, e.into()));
            }
        }
    }

    // Events refused alone count against their rows, unless every event
    // sent alone was refused and nothing at all was accepted: then the
    // platform is refusing the requests, whatever they hold.
    if alone_refused.len() > 1 && !any_receipt {
        tracing::error!(events = alone_refused.len(), error = %alone_refused[0].1,
            "the platform refused every event in a batch; not counting it against the rows");
        let kept = alone_refused.iter().map(|(id, _)| *id).collect();
        let (_, e) = alone_refused.swap_remove(0);
        failed.push((kept, ShipError::RefusedAll(e)));
    } else {
        for (id, e) in alone_refused {
            tracing::warn!(error = %e, "the platform refused a usage event sent alone; \
                keeping it and counting the refusal against it");
            failed.push((vec![id], e.into()));
        }
    }

    let settled = if answered.is_empty() {
        Ok(())
    } else {
        settle(db, &answered, &refused).await
    };
    if let Err(e @ ShipError::Claim(_)) = &settled {
        // Rolled back, so nothing is deleted on answers this pass could not
        // match up.
        tracing::error!(error = %e, "keeping every answered row and retrying it");
        reschedule(db, &answered, e, cfg).await;
    }
    // Whatever settling did, the pieces that failed are rescheduled with
    // their own errors (rows a failed settle left behind wait out the claim
    // lease).
    let mut first = settled.err();
    for (kept, e) in failed {
        reschedule(db, &kept, &e, cfg).await;
        first.get_or_insert(e);
    }
    match first {
        Some(e) => Err(e),
        None => Ok(ids.len()),
    }
}

/// In one transaction, move each `refused` event (which must be among the
/// `answered` rows) to `usage_outbox_rejected` with its reason, then delete
/// every `answered` row. A refused event that does not match exactly one
/// answered row (it changed under the claim lease) rolls everything back.
async fn settle(
    db: &Db,
    answered: &[i64],
    refused: &[(Uuid, String)],
) -> std::result::Result<(), ShipError> {
    let mut tx = crate::maintenance::begin(db).await?;
    for (event_id, reason) in refused {
        tracing::error!(%event_id, %reason,
            "the platform refused a usage event; moving it to usage_outbox_rejected");
        let moved = sqlx::query(&format!(
            "{MOVE_TO_REJECTED} AND event_id = $3 {INSERT_REJECTED} left($2, 500) FROM moved"
        ))
        .bind(answered)
        .bind(reason)
        .bind(event_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if moved != 1 {
            return Err(ShipError::Claim(format!(
                "moving rejected usage event {event_id} affected {moved} rows, not 1"
            )));
        }
    }
    sqlx::query("DELETE FROM usage_outbox WHERE id = ANY($1)")
        .bind(answered)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// The start of a statement moving claimed rows (`$1`, ids) into
/// `usage_outbox_rejected`: a caller may narrow the `WHERE`, then appends
/// [`INSERT_REJECTED`] and the reason expression.
const MOVE_TO_REJECTED: &str = "WITH moved AS ( \
       DELETE FROM usage_outbox WHERE id = ANY($1)";
const INSERT_REJECTED: &str = "RETURNING event_id, org_id, user_id, tool, billable, occurred_at, \
       attempts, row_failures) \
     INSERT INTO usage_outbox_rejected \
       (event_id, org_id, user_id, tool, billable, occurred_at, reason) \
     SELECT event_id, org_id, user_id, tool, billable, occurred_at,";

/// Whether the platform refused the request itself, so that resending it
/// unchanged can never succeed: a 4xx about the request (413 included). Not
/// this service's credential (401/403), a wrong URL (404), a timeout (408),
/// or rate limiting (429): those are fixed on this side or pass, and the
/// events are fine.
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

async fn reschedule(db: &Db, ids: &[i64], failure: &ShipError, cfg: &ShipperConfig) {
    if let Err(e) = try_reschedule(db, ids, failure, cfg).await {
        // The claim lease still holds the rows back; this only delays a retry.
        tracing::warn!(error = %e, "could not record a usage shipping failure");
    }
}

/// Back the rows off and record why. A failure about the rows themselves
/// ([`ShipError::faults_the_rows`]) also counts against them, and any that
/// have now had [`MAX_ROW_FAILURES`] of those are given up on; any other
/// failure leaves them to retry until they ship.
async fn try_reschedule(
    db: &Db,
    ids: &[i64],
    failure: &ShipError,
    cfg: &ShipperConfig,
) -> sqlx::Result<()> {
    let error: String = failure.to_string().chars().take(400).collect();
    let counts = failure.faults_the_rows();
    let mut tx = crate::maintenance::begin(db).await?;
    sqlx::query(
        "UPDATE usage_outbox SET last_error = $2, row_failures = row_failures + $5, \
                next_attempt_at = now() + make_interval(secs => \
                    LEAST($3::float8 * power(2, LEAST(attempts - 1, 20)), $4::float8)) \
         WHERE id = ANY($1)",
    )
    .bind(ids)
    .bind(&error)
    .bind(cfg.base_backoff.as_secs_f64())
    .bind(cfg.max_backoff.as_secs_f64())
    .bind(i32::from(counts))
    .execute(&mut *tx)
    .await?;
    let gave_up = if counts {
        sqlx::query(&format!(
            "{MOVE_TO_REJECTED} AND row_failures >= $3 {INSERT_REJECTED} \
             'given up after ' || row_failures || ' failures about the row itself; \
             last error: ' || $2 FROM moved"
        ))
        .bind(ids)
        .bind(&error)
        .bind(MAX_ROW_FAILURES)
        .execute(&mut *tx)
        .await?
        .rows_affected()
    } else {
        0
    };
    tx.commit().await?;
    if gave_up > 0 {
        tracing::error!(rows = gave_up, failures = MAX_ROW_FAILURES, last_error = %error,
            "gave up shipping usage; moved to usage_outbox_rejected");
    }
    Ok(())
}

/// Drain the outbox until `shutdown` flips, with a final pass on the way out.
/// Returns that final pass's failure, if it failed (the rows stay in the
/// outbox for the next start); the caller decides with
/// [`ShipError::is_retriable`] whether that should fail the exit.
///
/// A failure that passes on its own is logged at `warn`; one that needs an
/// operator at `error`.
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
                        retry_in = ?wait,
                        "the platform rejected this resource server's credential; usage stays in \
                         the outbox and is retried until the platform accepts it. Check \
                         CXP_INTROSPECTION_SECRET"
                    ),
                    e if e.is_retriable() => tracing::warn!(error = %e, retry_in = ?wait,
                        "usage shipping failed; the rows stay in the outbox and are retried"),
                    e => tracing::error!(error = %e, retry_in = ?wait,
                        "usage shipping failed and needs attention; the rows stay in the outbox \
                         and are retried"),
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
                        if e.is_retriable() {
                            tracing::warn!(error = %e,
                                "the final usage flush could not ship; rows stay in the outbox \
                                 for the next start");
                        } else {
                            tracing::error!(error = %e,
                                "final usage flush failed; rows stay in the outbox");
                        }
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
            service: None,
            meters: Vec::new(),
            budget: None,
            blocked_reason: None,
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
    fn status(status: u16) -> ShipError {
        ShipError::Platform(otto_resource::Error::Status {
            status,
            body: String::new(),
        })
    }

    /// Only failures about the rows count toward giving them up; everything
    /// else (unreachable, overloaded, rate limited, a refused credential, an
    /// org or URL the platform does not know) is retried until it ships.
    #[test]
    fn only_failures_about_the_rows_count_toward_giving_up() {
        for counts in [
            status(400),
            status(413),
            status(422),
            ShipError::Receipt("short".into()),
        ] {
            assert!(counts.faults_the_rows(), "{counts}");
        }
        for never in [
            status(401),
            status(403),
            status(404),
            status(408),
            status(429),
            status(500),
            status(503),
            ShipError::Platform(otto_resource::Error::Unauthorized),
            ShipError::Platform(otto_resource::Error::NotFound),
            ShipError::Platform(otto_resource::Error::InvalidUrl("x".into())),
            ShipError::Platform(otto_resource::Error::Decode("x".into())),
            ShipError::Claim("raced".into()),
            ShipError::Unfinished("out of time".into()),
            ShipError::RefusedAll(otto_resource::Error::Status {
                status: 422,
                body: "x".into(),
            }),
            ShipError::Db(sqlx::Error::PoolTimedOut),
        ] {
            assert!(!never.faults_the_rows(), "{never}");
        }
    }

    #[test]
    fn only_failures_that_pass_on_their_own_are_retriable() {
        for passes in [
            status(408),
            status(429),
            status(500),
            status(503),
            ShipError::Claim("raced".into()),
            ShipError::Unfinished("out of time".into()),
        ] {
            assert!(passes.is_retriable(), "{passes}");
        }
        for needs_attention in [
            status(400),
            status(404),
            ShipError::Platform(otto_resource::Error::Unauthorized),
            ShipError::Platform(otto_resource::Error::NotFound),
            ShipError::Receipt("short".into()),
            ShipError::RefusedAll(otto_resource::Error::Status {
                status: 422,
                body: "x".into(),
            }),
            ShipError::Db(sqlx::Error::PoolTimedOut),
        ] {
            assert!(!needs_attention.is_retriable(), "{needs_attention}");
        }
    }
}
