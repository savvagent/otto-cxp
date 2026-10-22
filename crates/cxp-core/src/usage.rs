//! Usage metering: the [`BILLABLE`] tools and the value meter each one
//! charges, the `Meter` that charges one `usage_outbox` row in the tool's own
//! transaction (refusing it while billing is blocked) and reports the org's
//! usage, and the shipper (`ship_once`, `run`) that drains the outbox to the
//! platform.
//!
//! - **Meters.** This service declares two value meters
//!   ([`METER_DECLARATIONS`]): [`SURVEYS_SENT`], charged by `send_survey`,
//!   and [`RESPONSES_COLLECTED`], charged by `submit_feedback` and
//!   `submit_survey_response` ([`BILLABLE_METERS`]). Each call records
//!   quantity 1 on its tool's meter.
//! - **Recording is local and atomic.** [`Meter::charge`] writes a
//!   `usage_outbox` row in the tool's own transaction, so a failed call rolls
//!   its record back and a successful one is recorded even while the platform
//!   is down.
//! - **Shipping is a background task** ([`run`]) that first declares the
//!   meters, then posts rows (with their `meter` and `quantity`) to the
//!   platform and deletes what it answered for. Nothing ships until the
//!   declaration is answered; the rows wait in the outbox. The platform
//!   dedupes on the row's `event_id`, so retries are free.
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
//!   with backoff for as long as it lasts. The sweep forgets rejected rows
//!   after `platform_events::REJECTED_USAGE_KEEP_DAYS`.
//! - **Enforcement is always on, and fails open.** [`Meter::charge`] reads
//!   the platform's cached usage status and refuses a billable call while
//!   the platform says billable calls are blocked
//!   ([`UsageStatus::billable_blocked`]: this service's budget hard stop, or
//!   payment required), as [`Error::BillingBlocked`]. If the platform cannot
//!   answer, the call is allowed and still recorded.
//!
//! Only the three [`BILLABLE`] tools are recorded. Everything else, including
//! `create_survey`, is free and records nothing; [`Meter::charge`] refuses
//! any other name as an internal error, so a free tool cannot be metered by
//! mistake.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use otto_resource::{MeterDeclaration, PlatformClient, UsageEvent, UsageStatus};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::{Db, Tx};
use serde::Serialize;
use sqlx::FromRow;
use tokio::sync::watch;
use uuid::Uuid;

use crate::error::{BlockedReason, Error, Result};

/// The `surveys_sent` value meter: one per survey sent to a customer.
pub const SURVEYS_SENT: &str = "otto-cxp/surveys_sent";
/// The `responses_collected` value meter: one per piece of feedback or
/// survey response recorded.
pub const RESPONSES_COLLECTED: &str = "otto-cxp/responses_collected";

/// The meters this service declares to the platform: (key, display name,
/// unit), in the order `whoami` and `usage` report them.
pub const METER_DECLARATIONS: &[(&str, &str, &str)] = &[
    (SURVEYS_SENT, "Surveys sent", "survey"),
    (RESPONSES_COLLECTED, "Responses collected", "response"),
];

/// Each billable tool and the value meter it charges: the one definition of
/// what is billed. Every other tool is free.
pub const BILLABLE_METERS: &[(&str, &str)] = &[
    ("send_survey", SURVEYS_SENT),
    ("submit_feedback", RESPONSES_COLLECTED),
    ("submit_survey_response", RESPONSES_COLLECTED),
];

/// The tools of [`BILLABLE_METERS`], in its order, and the only ones
/// recorded. A copy of that table's tools, kept equal to it by a unit test, so
/// callers can use a plain `&[&str]`.
pub const BILLABLE: &[&str] = &["send_survey", "submit_feedback", "submit_survey_response"];

/// The value meter `tool` charges, or `None` for a free tool.
///
/// ```
/// use cxp_core::usage::{meter_for, SURVEYS_SENT};
/// assert_eq!(meter_for("send_survey"), Some(SURVEYS_SENT));
/// assert_eq!(meter_for("create_survey"), None);
/// ```
pub fn meter_for(tool: &str) -> Option<&'static str> {
    BILLABLE_METERS
        .iter()
        .find(|(t, _)| *t == tool)
        .map(|(_, meter)| *meter)
}

pub fn is_billable(tool: &str) -> bool {
    meter_for(tool).is_some()
}

const QUOTA_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const FAIL_OPEN_FOR: Duration = Duration::from_secs(5);
/// The percent of this service's budget at or past which [`Status::warning`]
/// is set.
pub const WARN_AT_PERCENT: i64 = 80;
/// How many failures about a usage row itself (see
/// [`ShipError::faults_the_rows`]) it survives before it is given up on and
/// moved to `usage_outbox_rejected`. Failures that are not about the rows
/// never count: those rows are retried, with backoff, until they ship.
pub const MAX_ROW_FAILURES: i32 = 20;

#[derive(Clone)]
pub struct Meter {
    platform: Arc<PlatformClient>,
    fail_open: Arc<Mutex<HashMap<OrgId, Instant>>>,
    /// Where a refused caller is sent to fix billing.
    pub upgrade_url: String,
}

/// An org's billing standing, looked up by [`Meter::warm`] before a billable
/// tool opens its transaction and spent by [`Meter::charge`] in the
/// transaction that follows. It carries the org it was looked up for, and
/// `charge` refuses it in another org's transaction.
#[derive(Debug)]
#[must_use = "pass it to Meter::charge"]
pub struct Warmed {
    org: OrgId,
    /// `None` when the platform gave no answer: the charge fails open.
    status: Option<UsageStatus>,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter")
            .field("upgrade_url", &self.upgrade_url)
            .finish_non_exhaustive()
    }
}

impl Meter {
    pub fn new(platform: Arc<PlatformClient>, upgrade_url: impl Into<String>) -> Self {
        Self {
            platform,
            fail_open: Arc::default(),
            upgrade_url: upgrade_url.into(),
        }
    }

    /// Look up whether the org's billing is blocked, before the tool's
    /// transaction opens, and hand the answer to [`Meter::charge`]. This is
    /// the only place a billable call waits on the platform, so a slow
    /// platform never holds a pooled connection: `charge` makes no platform
    /// call of its own. With no answer from the platform the [`Warmed`]
    /// carries none, and `charge` fails open.
    ///
    /// ```text
    /// let warmed = meter.warm(org).await;
    /// let mut tx = begin_live(&db, org, Some(user)).await?;
    /// meter.charge(&mut tx, user, "send_survey", &warmed).await?;
    /// ```
    pub async fn warm(&self, org: OrgId) -> Warmed {
        Warmed {
            org,
            status: self.status_for(org).await,
        }
    }

    /// Record one call of a [`BILLABLE`] tool in `tx`, on its meter with
    /// quantity 1, or refuse it with [`Error::BillingBlocked`] while the
    /// platform says billable calls are blocked. Any other tool name is a
    /// programming error in the caller, not the agent's request: it is
    /// refused with [`Error::Internal`] without recording anything.
    ///
    /// Makes no platform call, so the transaction's connection never waits
    /// on the platform: the standing is the one [`Meter::warm`] looked up
    /// before the transaction opened. A `warmed` for another org than the
    /// transaction's is a programming error ([`Error::Internal`]); nothing is
    /// recorded.
    ///
    /// Fails open: with no answer from the platform, the call is recorded.
    /// The refusal is soft at the boundary: concurrent calls can each pass a
    /// status looked up just before it turns blocked.
    pub async fn charge(
        &self,
        tx: &mut Tx<'_>,
        user: UserId,
        tool: &str,
        warmed: &Warmed,
    ) -> Result<()> {
        let Some(meter) = meter_for(tool) else {
            tracing::error!(tool, "refusing to meter a tool that is not billable");
            return Err(Error::Internal(format!(
                "{tool} is not a billable tool; only {} are metered",
                BILLABLE.join(", ")
            )));
        };
        let org = tx.org();
        if warmed.org != org {
            tracing::error!(tool, "refusing to charge with another org's standing");
            return Err(Error::Internal(format!(
                "the standing looked up for one org was used to charge {tool} in another"
            )));
        }
        if let Some(reason) = warmed.status.as_ref().and_then(blocked_reason) {
            return Err(Error::BillingBlocked {
                reason,
                tool: tool.to_string(),
                url: self.upgrade_url.clone(),
            });
        }
        sqlx::query(
            "INSERT INTO usage_outbox (org_id, user_id, tool, billable, meter, quantity) \
             VALUES ($1, $2, $3, true, $4, 1)",
        )
        .bind(org)
        .bind(user)
        .bind(tool)
        .bind(meter)
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
            Ok(Err(e)) => e,
            Err(_) => {
                return self.fail_open_after(org, "timed out", None);
            }
        };
        let kind = match &failure {
            otto_resource::Error::Status { status, .. } => status.to_string(),
            otto_resource::Error::Unauthorized => "401".to_string(),
            otto_resource::Error::NotFound => "404".to_string(),
            otto_resource::Error::Decode(_) => "decode".to_string(),
            otto_resource::Error::InvalidUrl(_) => "invalid_url".to_string(),
            _ => "transport".to_string(),
        };
        self.fail_open_after(org, &kind, Some(&failure))
    }

    /// Log a failed usage lookup and open the org's fail-open window. Only
    /// the failure `kind` is logged above `debug`; the platform's text, which
    /// this service does not control, is `debug` only.
    fn fail_open_after(
        &self,
        org: OrgId,
        kind: &str,
        failure: Option<&otto_resource::Error>,
    ) -> Option<UsageStatus> {
        if matches!(
            failure,
            Some(otto_resource::Error::Decode(_) | otto_resource::Error::InvalidUrl(_))
        ) {
            tracing::error!(org = %org, kind,
                "could not use the platform's usage answer (a version mismatch or bad URL); \
                 billing enforcement is off until this is fixed, allowing billable calls for \
                 {FAIL_OPEN_FOR:?}");
        } else {
            tracing::warn!(org = %org, kind,
                "could not read usage from the platform; allowing billable calls for {FAIL_OPEN_FOR:?}");
        }
        if let Some(e) = failure {
            tracing::debug!(org = %org, error = %e, "usage lookup failed");
        }
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
        Ok(Status::new(usage))
    }
}

/// Why billable calls must be refused now, or `None` when they may proceed.
/// `payment_required` is its own reason; anything else that blocks (the
/// budget's hard stop, a reason this build does not know, a blocked budget
/// with no reason) is [`BlockedReason::BudgetExceeded`].
fn blocked_reason(status: &UsageStatus) -> Option<BlockedReason> {
    if !status.billable_blocked() {
        return None;
    }
    Some(match status.blocked_reason.as_deref() {
        Some(otto_resource::PAYMENT_REQUIRED) => BlockedReason::PaymentRequired,
        _ => BlockedReason::BudgetExceeded,
    })
}

pub(crate) fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Where an org stands this month: this service's meters and budget, and
/// whether its billable calls are blocked. The legacy fields describe the
/// family-wide bucket shared by every otto-* service and are deprecated.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// **Deprecated** (the family-wide bucket): the org's legacy plan name.
    pub plan: String,
    /// **Deprecated** (the family-wide bucket): operations the legacy plan
    /// includes.
    pub included_ops: i64,
    /// **Deprecated** (the family-wide bucket): billable calls this month
    /// across every service.
    pub billable_used: i64,
    /// **Deprecated** (the family-wide bucket): operations left, never
    /// negative.
    pub remaining: i64,
    /// **Deprecated** (the family-wide bucket): every call this month across
    /// every service.
    pub total_calls: i64,
    /// First day of the UTC billing month the figures are for.
    pub period_start: chrono::NaiveDate,
    /// True while billable calls are blocked, or once this service's budget
    /// is at least 80 % spent ([`WARN_AT_PERCENT`]). Worth telling a human.
    pub warning: bool,
    /// **Deprecated** (the family-wide bucket): never refuses anything.
    pub hard_stop: bool,
    /// Whether this server refuses billable calls while billing is blocked.
    /// Always true; kept so existing readers of the field keep working.
    pub enforced: bool,
    /// The service the platform reports these meters and budget for, if it is
    /// a slug (`[a-z0-9-]`, 1 to 64 characters). `None` from an older platform that does not name it, or before the operator
    /// assigns this service a slug.
    pub service: Option<String>,
    /// This month's usage of each meter this service declares, in
    /// [`METER_DECLARATIONS`] order; 0 for one with no usage yet.
    pub meters: Vec<MeterOut>,
    /// This service's monthly budget, if the org has set one.
    pub budget: Option<BudgetOut>,
    /// Why billable calls are refused now: `payment_required`, or
    /// `budget_exceeded` for every other block (including a reason this
    /// build does not know and a blocked budget with no reason), exactly as
    /// [`Meter::charge`] refuses them; `None` while they are not.
    pub blocked_reason: Option<String>,
}

/// One declared meter's usage this month, in [`Status::meters`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeterOut {
    /// The meter's key, `otto-cxp/<meter>`.
    pub meter: String,
    pub display_name: String,
    pub unit: String,
    pub quantity: u64,
}

/// This service's budget this month, in [`Status::budget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BudgetOut {
    /// The budget for one UTC calendar month, in US cents.
    pub monthly_budget_cents: i64,
    /// What this service has cost this month, in US cents; `None` when the
    /// platform could not compute it.
    pub spend_cents: Option<i64>,
    /// Whole percent of the budget spent; can exceed 100. `None` when spend
    /// is unknown.
    pub percent_used: Option<i64>,
    /// The alert thresholds (percent) that have fired this month.
    pub thresholds_reached: Vec<u8>,
    /// Whether the org refuses billable calls once spend reaches the budget.
    pub hard_stop: bool,
    /// Whether the budget refuses billable calls now.
    pub blocked: bool,
    /// Why `blocked` is set, or why spend is unknown: `budget_exceeded` or
    /// `spend_unavailable`; any other text from the platform becomes `None`.
    pub reason: Option<String>,
}

impl From<otto_resource::BudgetStatus> for BudgetOut {
    fn from(b: otto_resource::BudgetStatus) -> Self {
        Self {
            monthly_budget_cents: b.monthly_budget_cents,
            spend_cents: b.spend_cents,
            percent_used: b.percent_used,
            thresholds_reached: b.thresholds_reached,
            hard_stop: b.hard_stop,
            blocked: b.blocked,
            reason: b.reason.filter(|r| {
                r == otto_resource::BUDGET_EXCEEDED || r == otto_resource::SPEND_UNAVAILABLE
            }),
        }
    }
}

/// Whether `s` is a service slug, `^[a-z0-9-]{1,64}$`: the only platform
/// text [`Status::service`] echoes.
fn is_slug(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Status {
    fn new(u: UsageStatus) -> Self {
        let remaining = u.included_ops.saturating_sub(u.billable_count).max(0);
        let warning = u.billable_blocked()
            || u.budget
                .as_ref()
                .and_then(|b| b.percent_used)
                .is_some_and(|p| p >= WARN_AT_PERCENT);
        let refused_with = blocked_reason(&u).map(|r| r.code().to_string());
        let meters = METER_DECLARATIONS
            .iter()
            .map(|(key, display_name, unit)| MeterOut {
                meter: (*key).to_string(),
                display_name: (*display_name).to_string(),
                unit: (*unit).to_string(),
                quantity: u.meter(key).map_or(0, |m| m.quantity),
            })
            .collect();
        Self {
            plan: plan_name(&u.plan),
            included_ops: u.included_ops,
            billable_used: u.billable_count,
            remaining,
            total_calls: u.total_count,
            period_start: u.period_start,
            warning,
            hard_stop: u.hard_stop,
            enforced: true,
            service: u.service.filter(|s| is_slug(s)),
            meters,
            budget: u.budget.map(BudgetOut::from),
            blocked_reason: refused_with,
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
    meter: Option<String>,
    quantity: Option<i64>,
}

impl Row {
    /// The quantity to ship, or `None` for a metered row whose quantity the
    /// platform would refuse (missing, not positive, or above
    /// [`otto_resource::MAX_METER_QUANTITY`]). An unmetered row is always
    /// shippable, with no quantity.
    fn shippable_quantity(&self) -> Option<Option<u64>> {
        match (&self.meter, self.quantity) {
            (None, None) => Some(None),
            (Some(_), Some(q)) => u64::try_from(q)
                .ok()
                .filter(|q| (1..=otto_resource::MAX_METER_QUANTITY).contains(q))
                .map(Some),
            _ => None,
        }
    }
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
/// - A metered row whose quantity the platform would refuse (above
///   `MAX_METER_QUANTITY`) is never sent: it moves to `usage_outbox_rejected`
///   with the reason `invalid_quantity`, and is counted in the returned
///   number. If every claimed row is like that, no request is made.
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
         RETURNING id, event_id, org_id, user_id, tool, billable, occurred_at, \
                   meter, quantity",
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
    let claimed = rows.len();
    // A metered row whose quantity the platform would refuse is never sent
    // (and never with a guessed quantity): it moves straight to
    // `usage_outbox_rejected`. The outbox CHECK makes a missing or
    // non-positive quantity unreachable; only one above the cap can get here.
    let mut shippable: Vec<(Row, Option<u64>)> = Vec::with_capacity(claimed);
    let mut invalid_ids: Vec<i64> = Vec::new();
    for r in rows {
        match r.shippable_quantity() {
            Some(quantity) => shippable.push((r, quantity)),
            None => invalid_ids.push(r.id),
        }
    }
    if !invalid_ids.is_empty() {
        reject_invalid_quantity(db, &invalid_ids).await?;
    }
    if shippable.is_empty() {
        return Ok(claimed);
    }
    let ids: Vec<i64> = shippable.iter().map(|(r, _)| r.id).collect();
    let events: Vec<UsageEvent> = shippable
        .iter()
        .map(|(r, quantity)| UsageEvent {
            event_id: r.event_id,
            org_id: r.org_id,
            user_id: r.user_id,
            tool: r.tool.clone(),
            billable: r.billable,
            occurred_at: r.occurred_at,
            service: None,
            meter: r.meter.clone(),
            quantity: *quantity,
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
        None => Ok(claimed),
    }
}

/// Move claimed rows whose quantity the platform would refuse to
/// `usage_outbox_rejected` with the reason `invalid_quantity`, unsent.
async fn reject_invalid_quantity(db: &Db, ids: &[i64]) -> std::result::Result<(), ShipError> {
    let mut tx = crate::maintenance::begin(db).await?;
    sqlx::query(&format!(
        "{MOVE_TO_REJECTED} {INSERT_REJECTED} 'invalid_quantity' FROM moved"
    ))
    .bind(ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::error!(
        rows = ids.len(),
        "usage rows with a quantity the platform would refuse; moved to usage_outbox_rejected \
         unsent"
    );
    Ok(())
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
/// [`INSERT_REJECTED`] and the reason expression. It returns `row_failures`
/// because the give-up reason quotes it; nothing reads `attempts`.
const MOVE_TO_REJECTED: &str = "WITH moved AS ( \
       DELETE FROM usage_outbox WHERE id = ANY($1)";
const INSERT_REJECTED: &str = "RETURNING event_id, org_id, user_id, tool, billable, occurred_at, \
       meter, quantity, row_failures) \
     INSERT INTO usage_outbox_rejected \
       (event_id, org_id, user_id, tool, billable, occurred_at, meter, quantity, reason) \
     SELECT event_id, org_id, user_id, tool, billable, occurred_at, meter, quantity,";

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

/// How often a stuck meter declaration is logged.
const DECLARE_LOG_EVERY: Duration = Duration::from_secs(60);

/// The most rows [`rows_waiting`] counts in production; past it the count is
/// reported as `10000+`.
pub const WAITING_CAP: i64 = 10_000;

/// A count of `usage_outbox` rows that stops at a cap, so counting never
/// scans a backlog without bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowsWaiting {
    /// The rows counted: at most the cap.
    pub count: i64,
    /// Whether more rows than the cap were waiting.
    pub capped: bool,
}

impl std::fmt::Display for RowsWaiting {
    /// The count, or `<cap>+` when it stopped at the cap.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.capped {
            write!(f, "{}+", self.count)
        } else {
            write!(f, "{}", self.count)
        }
    }
}

/// Rows waiting in `usage_outbox`, for the declaration log: one `count(*)`
/// over at most `cap + 1` rows, in a maintenance transaction, so a backlog
/// during an outage cannot make the count itself expensive. Production passes
/// [`WAITING_CAP`].
///
/// ```no_run
/// # async fn f(db: &otto_tenant::Db) -> Result<(), sqlx::Error> {
/// let n = cxp_core::usage::rows_waiting(db, cxp_core::usage::WAITING_CAP).await?;
/// println!("{n}"); // "12", or "10000+" at the cap
/// # Ok(()) }
/// ```
pub async fn rows_waiting(db: &Db, cap: i64) -> std::result::Result<RowsWaiting, sqlx::Error> {
    let mut tx = crate::maintenance::begin(db).await?;
    let seen: i64 =
        sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM usage_outbox LIMIT $1) s")
            .bind(cap.saturating_add(1))
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(RowsWaiting {
        count: seen.min(cap),
        capped: seen > cap,
    })
}

/// [`rows_waiting`] at the production cap, as the string the logs carry.
async fn waiting(db: &Db) -> Option<String> {
    match rows_waiting(db, WAITING_CAP).await {
        Ok(n) => Some(n.to_string()),
        Err(e) => {
            tracing::debug!(error = %e, "could not count the usage rows waiting");
            None
        }
    }
}

/// Declare [`METER_DECLARATIONS`] with the platform, retrying with capped
/// exponential backoff ([`ShipperConfig::base_backoff`] to
/// [`ShipperConfig::max_backoff`]) until the platform answers. Returns `false`
/// if `shutdown` came first.
///
/// Every failure retries: a transport error, any status (5xx, 401, 409
/// `service_not_set`, any other 4xx), and an answer this client cannot decode.
/// Nothing may ship until the platform accepts the declaration (a metered
/// event for an undeclared meter would be refused and moved aside), and the
/// outbox keeps every row meanwhile, so a platform that cannot take the
/// declaration delays billing and loses none. On the first failure, then at
/// most once a minute, it logs at `error` with the kind of failure (the
/// status, `"decode"`, `"invalid_url"`, or `"transport"`) and the number of
/// rows waiting (one `count(*)` per log, capped at [`WAITING_CAP`] and shown
/// as `10000+` past it, so the small shared database is not polled); the platform's text is `debug` only.
///
/// A receipt that rejects a key is not a failure to retry: the same
/// declaration would be refused again. Each rejected key is logged at `error`
/// (a deploy bug) and the other meter's rows still ship; the platform answers
/// for each event of the undeclared meter itself.
async fn declare_meters(
    db: &Db,
    platform: &PlatformClient,
    cfg: &ShipperConfig,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let declarations: Vec<MeterDeclaration> = METER_DECLARATIONS
        .iter()
        .map(|(key, display, unit)| MeterDeclaration::new(*key, *display, *unit))
        .collect();
    let mut attempt: u32 = 0;
    let mut last_logged: Option<tokio::time::Instant> = None;
    loop {
        if *shutdown.borrow() {
            return false;
        }
        let result = tokio::select! {
            r = platform.declare_meters(&declarations) => r,
            _ = shutdown.changed() => return false,
        };
        let e = match result {
            Ok(receipt) => {
                for r in &receipt.rejected {
                    // Only our own constant key is logged at `error`; what the
                    // platform echoed back stays at `debug`.
                    match METER_DECLARATIONS.iter().find(|(key, _, _)| *key == r.key) {
                        Some((key, _, _)) => tracing::error!(
                            key = *key,
                            "the platform refused to declare a usage meter; its usage will be \
                             refused until the declaration is fixed"
                        ),
                        None => tracing::error!(
                            "the platform refused a meter key this service did not send (see debug); its usage will \
                             be refused until the declaration is fixed"
                        ),
                    }
                    tracing::debug!(key = %r.key, platform_text = %r.reason, "usage meter refused");
                }
                tracing::info!(
                    meters = declarations.len(),
                    declared = receipt.declared.len(),
                    "declared usage meters"
                );
                return true;
            }
            Err(e) => e,
        };
        if last_logged.is_none_or(|t| t.elapsed() >= DECLARE_LOG_EVERY) {
            last_logged = Some(tokio::time::Instant::now());
            let rows_waiting = waiting(db).await;
            let kind = match &e {
                otto_resource::Error::Status { status, .. } => status.to_string(),
                otto_resource::Error::Unauthorized => "401".to_string(),
                otto_resource::Error::NotFound => "404".to_string(),
                otto_resource::Error::Decode(_) => "decode".to_string(),
                otto_resource::Error::InvalidUrl(_) => "invalid_url".to_string(),
                _ => "transport".to_string(),
            };
            let hint = if matches!(e, otto_resource::Error::Unauthorized) {
                " Check CXP_INTROSPECTION_SECRET."
            } else {
                ""
            };
            tracing::error!(
                status = kind,
                ?rows_waiting,
                "could not declare the usage meters; no usage ships until they are declared \
                 (rows stay queued and nothing is lost). Retrying.{hint}"
            );
            tracing::debug!(error = %e, "could not declare the usage meters");
        }
        let wait = cfg
            .base_backoff
            .saturating_mul(1u32 << attempt.min(20))
            .min(cfg.max_backoff);
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => return false,
        }
    }
}

/// Declare the meters, then drain the outbox until `shutdown` flips, with a
/// final pass on the way out. Returns that final pass's failure, if it failed
/// (the rows stay in the outbox for the next start); the caller decides with
/// [`ShipError::is_retriable`] whether that should fail the exit.
///
/// Nothing ships until the meters are declared ([`declare_meters`]): a metered
/// event for an undeclared meter would be rejected and moved aside. If
/// shutdown arrives while declaring, this returns `Ok(())` at once, without the
/// final pass; the rows stay queued for the next process.
///
/// A shipping failure that passes on its own is logged at `warn`; one that
/// needs an operator at `error`.
///
/// `pause` reads `true` while the database-role check refuses the connecting
/// role (the server's readiness alarm): the shipper runs as that role, so it
/// pauses — up to and including the final flush on `shutdown` — until the
/// alarm clears, rather than ship as a role that may now hold `BYPASSRLS`.
/// Rows wait in the outbox; nothing is lost.
pub async fn run(
    db: Db,
    platform: Arc<PlatformClient>,
    cfg: ShipperConfig,
    mut pause: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) -> std::result::Result<(), ShipError> {
    if !declare_meters(&db, &platform, &cfg, &mut shutdown).await {
        let rows_waiting = waiting(&db).await;
        tracing::warn!(
            ?rows_waiting,
            "shutting down before the usage meters were declared; no final flush, the rows \
             stay queued for the next start"
        );
        return Ok(());
    }
    let mut failures: u32 = 0;
    loop {
        if *pause.borrow() {
            tracing::warn!(
                "usage shipping is paused: the database role the shipper runs as is refused; \
                 rows stay in the outbox until it is fixed"
            );
            tokio::select! {
                resumed = pause.wait_for(|paused| !*paused) => {
                    // The sender is shared with the process's lifetime; a
                    // closed channel means the process is winding down.
                    if resumed.is_err() {
                        return Ok(());
                    }
                }
                _ = shutdown.changed() => return Ok(()),
            }
            continue;
        }
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
            // An alarm raised during the sleep takes effect now, not after
            // the backoff (up to a minute) has run out: the next pass starts
            // at the pause check above.
            changed = pause.changed() => {
                // The sender is shared with the process's lifetime; a closed
                // channel means the process is winding down, as a shutdown
                // does, and the two can arrive together. It gets the final
                // flush too (the last alarm state stays readable).
                if changed.is_ok() {
                    continue;
                }
                final_flush(&db, &platform, &cfg, &pause).await?;
                return Ok(());
            }
            _ = shutdown.changed() => {
                final_flush(&db, &platform, &cfg, &pause).await?;
                return Ok(());
            }
        }
    }
}

/// The last pass before the shipper stops. It is still a pass as the
/// connecting role, so a raised alarm skips it: the rows wait for the next
/// start.
async fn final_flush(
    db: &Db,
    platform: &PlatformClient,
    cfg: &ShipperConfig,
    pause: &watch::Receiver<bool>,
) -> std::result::Result<(), ShipError> {
    if *pause.borrow() {
        tracing::warn!(
            "shutting down while usage shipping is paused; no final flush, the rows stay \
             queued for the next start"
        );
        return Ok(());
    }
    match ship_once(db, platform, cfg).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.is_retriable() {
                tracing::warn!(error = %e,
                    "the final usage flush could not ship; rows stay in the outbox for the \
                     next start");
            } else {
                tracing::error!(error = %e, "final usage flush failed; rows stay in the outbox");
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_the_three_value_writes_are_billable() {
        assert_eq!(
            BILLABLE,
            ["send_survey", "submit_feedback", "submit_survey_response"]
        );
        // `BILLABLE` is a copy of the table's tools; this keeps the two equal.
        let tools: Vec<&str> = BILLABLE_METERS.iter().map(|(t, _)| *t).collect();
        assert_eq!(BILLABLE, tools.as_slice());
        for t in [
            "whoami",
            "usage",
            "create_survey",
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
    fn each_billable_tool_charges_its_value_meter() {
        assert_eq!(meter_for("send_survey"), Some(SURVEYS_SENT));
        assert_eq!(meter_for("submit_feedback"), Some(RESPONSES_COLLECTED));
        assert_eq!(
            meter_for("submit_survey_response"),
            Some(RESPONSES_COLLECTED)
        );
        assert_eq!(meter_for("create_survey"), None);
        assert_eq!(meter_for("whoami"), None);
        for tool in BILLABLE {
            assert!(is_billable(tool), "{tool}");
        }
    }

    #[test]
    fn the_declared_meters_are_exactly_this_services_two() {
        let keys: Vec<&str> = METER_DECLARATIONS.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, [SURVEYS_SENT, RESPONSES_COLLECTED]);
        for (key, display, unit) in METER_DECLARATIONS {
            assert!(key.starts_with("otto-cxp/"), "{key}");
            assert!(!display.is_empty() && !unit.is_empty(), "{key}");
        }
        // Every meter a tool charges is declared.
        for (tool, meter) in BILLABLE_METERS {
            assert!(keys.contains(meter), "{tool}: {meter}");
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

    fn budget(percent_used: Option<i64>, blocked: bool) -> otto_resource::BudgetStatus {
        otto_resource::BudgetStatus {
            monthly_budget_cents: 1000,
            spend_cents: percent_used.map(|p| p * 10),
            percent_used,
            thresholds_reached: vec![50, 80],
            hard_stop: true,
            blocked,
            reason: blocked.then(|| otto_resource::BUDGET_EXCEEDED.to_string()),
        }
    }

    fn reported(meter: &str, quantity: u64) -> otto_resource::MeterUsage {
        otto_resource::MeterUsage {
            meter: meter.into(),
            display_name: "platform's name".into(),
            unit: "platform's unit".into(),
            quantity,
        }
    }

    /// `payment_required` is its own refusal; any other reason, including
    /// one this build does not know and a blocked budget with no reason at
    /// all, is `budget_exceeded`, which sends an owner to the console.
    #[test]
    fn a_blocked_status_maps_to_its_refusal() {
        let with = |reason: Option<&str>, budget_blocked: bool| {
            let mut u = usage(0, 0);
            u.blocked_reason = reason.map(str::to_string);
            u.budget = budget_blocked.then(|| budget(Some(120), true));
            blocked_reason(&u)
        };
        assert_eq!(
            with(Some("payment_required"), false),
            Some(BlockedReason::PaymentRequired)
        );
        assert_eq!(
            with(Some("budget_exceeded"), false),
            Some(BlockedReason::BudgetExceeded)
        );
        assert_eq!(
            with(Some("something_new"), false),
            Some(BlockedReason::BudgetExceeded)
        );
        assert_eq!(with(None, true), Some(BlockedReason::BudgetExceeded));
        assert_eq!(with(None, false), None);
        let mut spend_unknown = usage(0, 0);
        spend_unknown.budget = Some(otto_resource::BudgetStatus {
            blocked: false,
            reason: Some(otto_resource::SPEND_UNAVAILABLE.into()),
            ..budget(None, false)
        });
        assert_eq!(blocked_reason(&spend_unknown), None);
        assert_eq!(
            BlockedReason::BudgetExceeded.code(),
            otto_resource::BUDGET_EXCEEDED
        );
        assert_eq!(
            BlockedReason::PaymentRequired.code(),
            otto_resource::PAYMENT_REQUIRED
        );
    }

    /// Both declared meters, in declaration order, whatever the platform
    /// sends: a meter it omits reads 0, and one this service did not declare
    /// is dropped, so agents see a stable list.
    #[test]
    fn status_reports_both_meters_in_declaration_order() {
        let mut u = usage(5, 10);
        u.service = Some("otto-cxp".into());
        u.meters = vec![
            reported("otto-cxp/something_else", 9),
            reported(RESPONSES_COLLECTED, 4),
        ];
        let s = Status::new(u);
        assert_eq!(s.service.as_deref(), Some("otto-cxp"));
        let got: Vec<(&str, &str, &str, u64)> = s
            .meters
            .iter()
            .map(|m| {
                (
                    m.meter.as_str(),
                    m.display_name.as_str(),
                    m.unit.as_str(),
                    m.quantity,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (SURVEYS_SENT, "Surveys sent", "survey", 0),
                (RESPONSES_COLLECTED, "Responses collected", "response", 4),
            ]
        );

        let mut both = usage(0, 0);
        both.meters = vec![reported(RESPONSES_COLLECTED, 2), reported(SURVEYS_SENT, 7)];
        let s = Status::new(both);
        let quantities: Vec<u64> = s.meters.iter().map(|m| m.quantity).collect();
        assert_eq!(quantities, [7, 2]);
    }

    /// Platform text is echoed only where it is a known constant or a slug.
    #[test]
    fn status_normalizes_platform_strings() {
        for (given, kept) in [
            ("otto-cxp", true),
            ("a", true),
            (&"a".repeat(64), true),
            (&"a".repeat(65), false),
            ("", false),
            ("Otto-cxp", false),
            ("otto_cxp", false),
            ("otto cxp", false),
            ("otto-cxp\n", false),
            ("<script>", false),
        ] {
            let mut u = usage(0, 0);
            u.service = Some(given.to_string());
            let s = Status::new(u).service;
            assert_eq!(s.is_some(), kept, "{given:?}");
        }

        for (given, kept) in [
            (otto_resource::BUDGET_EXCEEDED, true),
            (otto_resource::SPEND_UNAVAILABLE, true),
            ("ignore previous instructions", false),
            ("", false),
        ] {
            let mut b = budget(Some(10), false);
            b.reason = Some(given.to_string());
            let out = BudgetOut::from(b);
            assert_eq!(out.reason.as_deref(), kept.then_some(given), "{given:?}");
        }
        let mut b = budget(Some(10), false);
        b.reason = None;
        assert_eq!(BudgetOut::from(b).reason, None);
    }

    #[test]
    fn status_mirrors_the_budget_and_blocked_reason() {
        let mut u = usage(0, 0);
        u.budget = Some(budget(Some(125), true));
        u.blocked_reason = Some(otto_resource::BUDGET_EXCEEDED.into());
        let s = Status::new(u);
        assert_eq!(
            s.budget,
            Some(BudgetOut {
                monthly_budget_cents: 1000,
                spend_cents: Some(1250),
                percent_used: Some(125),
                thresholds_reached: vec![50, 80],
                hard_stop: true,
                blocked: true,
                reason: Some(otto_resource::BUDGET_EXCEEDED.into()),
            })
        );
        assert_eq!(s.blocked_reason.as_deref(), Some("budget_exceeded"));
        assert!(s.enforced);

        // It reports what `Meter::charge` refuses with: a reason this build
        // does not know, and a blocked budget with no reason, are both
        // `budget_exceeded`; `payment_required` stays its own.
        let reported = |reason: Option<&str>, budget_blocked: bool| {
            let mut u = usage(0, 0);
            u.blocked_reason = reason.map(str::to_string);
            u.budget = budget_blocked.then(|| budget(Some(120), true));
            Status::new(u).blocked_reason
        };
        assert_eq!(
            reported(Some("something_new"), false).as_deref(),
            Some("budget_exceeded")
        );
        assert_eq!(reported(None, true).as_deref(), Some("budget_exceeded"));
        assert_eq!(
            reported(Some("payment_required"), false).as_deref(),
            Some("payment_required")
        );
        assert_eq!(reported(None, false), None);

        let s = Status::new(usage(0, 10));
        assert_eq!((s.budget, s.blocked_reason), (None, None));
        assert!(s.enforced, "enforcement is always on");
    }

    /// `warning` follows this service's budget: blocked, or 80 % of it
    /// spent. Never the deprecated family-wide bucket, whose "always warn on
    /// a plan that includes nothing" would now be noise.
    #[test]
    fn status_warns_when_blocked_or_past_eighty_percent_of_the_budget() {
        let warns = |percent: Option<i64>, blocked: bool, reason: Option<&str>| {
            let mut u = usage(0, 0);
            u.budget = Some(budget(percent, blocked));
            u.blocked_reason = reason.map(str::to_string);
            Status::new(u).warning
        };
        assert!(!warns(Some(79), false, None));
        assert!(warns(Some(80), false, None));
        assert!(warns(Some(130), true, Some("budget_exceeded")));
        assert!(!warns(None, false, None), "spend unknown");
        assert!(warns(Some(0), false, Some("payment_required")));
        assert!(warns(None, true, None), "blocked with no reason");
        // A plan that includes no legacy operations no longer warns.
        assert!(!Status::new(usage(0, 0)).warning);
        assert!(!Status::new(usage(150, 100)).warning);
    }

    #[test]
    fn status_keeps_the_deprecated_bucket_fields() {
        let s = Status::new(usage(79, 100));
        assert_eq!(s.remaining, 21);
        let s = Status::new(usage(150, 100));
        assert_eq!(s.remaining, 0);
        assert_eq!(s.plan, "Team");
        assert_eq!(s.total_calls, 153);
        assert_eq!((s.included_ops, s.billable_used), (100, 150));
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
