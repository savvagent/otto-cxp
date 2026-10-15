//! Liveness and readiness, which are not the same question.
//!
//! **`/healthz` never touches the database, on purpose.** A liveness probe
//! answers "is this process wedged, should it be killed?", and an orchestrator
//! kills what fails it. Point one at the database and a thirty-second database
//! blip restarts every replica simultaneously — turning a brief outage into a
//! cold start of the entire fleet, against a database that is already unwell.
//! The one thing worse than a degraded service is a degraded service that keeps
//! restarting.
//!
//! **`/readyz` does touch it**, because it answers a different question: "should
//! traffic come *here*?" The answer to that is no while the database is
//! unreachable, and the remedy is to route elsewhere and try again — not to
//! kill anything. This is the check a load balancer should be pointed at.
//!
//! **`/readyz` also reports the boot-time platform check.** While the
//! platform has refused this service's credential
//! ([`PlatformCheck::Rejected`]) or `CXP_PLATFORM_URL` answered as no platform
//! does ([`PlatformCheck::Misconfigured`]), no token can be validated, so the
//! replica is not ready: a deploy with a bad secret fails its health check
//! and rolls back rather than serving 500s. The check runs once, at boot; a
//! fix (a new secret, a corrected URL or registration) takes effect on the
//! restart that applying it causes. A platform that was merely unreachable at
//! boot does not hold readiness back: an outage must not take every replica
//! out of rotation.
//!
//! **`/readyz` also reports the hourly database-role re-check.** Boot refuses
//! a runtime role that could step around row-level security
//! (`boot::check_db_role`), but a grant made afterwards (a stray `GRANT
//! TRUNCATE`, a membership in a privileged role) would go unnoticed until the
//! next restart. The sweeper re-runs the check after each pass
//! (`boot::recheck_db_role`) and raises a [`RoleAlarm`] when it would now
//! refuse; while raised, the replica is unready with the fixed reason
//! `database role refused` (the role summary is logged, never sent: catalog
//! text stays out of responses). This fails closed, as boot does: with one
//! machine, a raised alarm takes the service out of rotation until the role
//! is fixed. The next pass after the fix clears it, as does a restart, which
//! re-runs the boot check.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use otto_tenant::Db;

use crate::PlatformCheck;

/// How long a readiness check waits on the database.
///
/// Short by design. A readiness probe that hangs for the pool's full acquire
/// timeout is indistinguishable from a hung process to whatever is polling it,
/// and the answer it eventually gives arrives after the decision was needed.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// `/readyz`'s reason while the [`RoleAlarm`] is raised: fixed, so no
/// catalog text reaches a response.
const ROLE_REFUSED: &str = "database role refused";

/// Raised while the database-role re-check would refuse the role this
/// process serves as; holds the role summary for the log. Cheap to clone:
/// clones share one flag, so boot hands one to the sweeper and one to the
/// router.
#[derive(Clone, Default)]
pub struct RoleAlarm(Arc<RwLock<Option<String>>>);

impl RoleAlarm {
    /// Raise (or keep raised) with the role `summary`.
    pub fn raise(&self, summary: String) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(summary);
    }

    /// Lower it; `true` when it was raised, so the caller logs the change
    /// once.
    pub fn clear(&self) -> bool {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some()
    }

    /// The role summary it was raised with, if raised: for the log only,
    /// never a response.
    pub fn summary(&self) -> Option<String> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn is_raised(&self) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }
}

/// What `/readyz` reports besides the database: boot's platform check and
/// the role alarm.
#[derive(Clone)]
pub struct Readiness {
    platform: PlatformCheck,
    role: RoleAlarm,
}

impl Readiness {
    /// `platform` is the result of [`crate::boot::check_platform`]; the role
    /// alarm starts lowered and unshared.
    pub fn new(platform: PlatformCheck) -> Self {
        Self {
            platform,
            role: RoleAlarm::default(),
        }
    }

    /// Report `alarm`, shared with whatever raises it.
    pub fn with_role_alarm(mut self, alarm: RoleAlarm) -> Self {
        self.role = alarm;
        self
    }
}

impl From<PlatformCheck> for Readiness {
    fn from(platform: PlatformCheck) -> Self {
        Self::new(platform)
    }
}

#[derive(Clone)]
struct Ready {
    db: Db,
    /// What boot's platform credential check found, shared into the router.
    platform: Arc<PlatformCheck>,
    role: RoleAlarm,
}

/// The health routes, with `/readyz` reporting `readiness`.
pub fn router(db: Db, readiness: impl Into<Readiness>) -> Router {
    let Readiness { platform, role } = readiness.into();
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(Ready {
            db,
            platform: Arc::new(platform),
            role,
        })
}

/// Why the platform check holds readiness back, if it does.
fn platform_unready(check: &PlatformCheck) -> Option<&'static str> {
    match check {
        PlatformCheck::Rejected => Some("platform credential rejected"),
        PlatformCheck::Misconfigured(_) => Some("platform misconfigured"),
        PlatformCheck::Accepted | PlatformCheck::Unreachable(_) => None,
    }
}

/// `GET /healthz` — the process is running and the runtime is scheduling.
async fn healthz() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

/// `GET /readyz` — this replica can authenticate a request and serve one
/// that needs the database.
async fn readyz(State(ready): State<Ready>) -> Response {
    if let Some(reason) = platform_unready(&ready.platform) {
        // Logged at error at boot already; a warn per probe keeps it visible.
        tracing::warn!(
            reason,
            "readiness check failed: the platform check at boot failed"
        );
        return unready(reason);
    }
    if let Some(summary) = ready.role.summary() {
        // Logged, never sent: catalog text stays out of responses.
        tracing::warn!(
            reason = ROLE_REFUSED,
            role = %summary,
            "readiness check failed: the database role re-check would refuse this role"
        );
        return unready(ROLE_REFUSED);
    }
    // `cxp_core::ping` touches no tenant table, so it needs no org: it asks
    // about the connection, not about anyone's data.
    let probe = tokio::time::timeout(READY_TIMEOUT, cxp_core::ping(&ready.db)).await;

    match probe {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ready" })),
        )
            .into_response(),

        // The body says which of the two it was. "not ready" alone sends
        // whoever is paged to look at the wrong thing half the time: a refused
        // connection is a database that is down, a timeout is usually a pool
        // with every connection checked out, and those have different fixes.
        Ok(Err(e)) => {
            tracing::warn!(error = ?e, "readiness check failed");
            unready("the database rejected a probe query")
        }
        Err(_) => {
            tracing::warn!(timeout = ?READY_TIMEOUT, "readiness check timed out");
            unready("the database did not answer a probe query in time")
        }
    }
}

fn unready(reason: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "status": "unready", "reason": reason })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_refused_or_misconfigured_platform_holds_readiness_back() {
        assert_eq!(
            platform_unready(&PlatformCheck::Rejected),
            Some("platform credential rejected")
        );
        assert_eq!(
            platform_unready(&PlatformCheck::Misconfigured("HTTP 404".into())),
            Some("platform misconfigured")
        );
        assert_eq!(platform_unready(&PlatformCheck::Accepted), None);
        assert_eq!(
            platform_unready(&PlatformCheck::Unreachable("refused".into())),
            None
        );
    }

    /// `/readyz` over a database nothing listens on, as boot built it.
    async fn probe(readiness: Readiness) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(1))
            .connect_lazy("postgres://cxp:cxp@127.0.0.1:1/none")
            .unwrap();
        let res = router(Db::from_pool(pool), readiness)
            .oneshot(
                axum::http::Request::get("/readyz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// A raised role alarm makes the replica unready with a fixed reason,
    /// never the role summary (catalog text stays out of responses); it is
    /// checked before the database, and once cleared readiness is the
    /// platform's and the database's answer again.
    #[tokio::test]
    async fn the_role_alarm_holds_readiness_back() {
        let alarm = RoleAlarm::default();
        assert!(!alarm.is_raised());
        let readiness = || Readiness::new(PlatformCheck::Accepted).with_role_alarm(alarm.clone());

        let summary = "database role \"cxp_server\" can TRUNCATE table \"customers\"";
        assert_eq!(alarm.summary(), None);
        alarm.raise(summary.into());
        assert!(alarm.is_raised());
        assert_eq!(alarm.summary().as_deref(), Some(summary));
        let (status, body) = probe(readiness()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            serde_json::json!({ "status": "unready", "reason": "database role refused" })
        );

        // The platform check still comes first.
        let (_, body) =
            probe(Readiness::new(PlatformCheck::Rejected).with_role_alarm(alarm.clone())).await;
        assert_eq!(body["reason"], "platform credential rejected");

        assert!(alarm.clear(), "it was raised");
        assert!(!alarm.clear(), "already clear");
        assert!(!alarm.is_raised());
        let (status, body) = probe(readiness()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            body["reason"]
                .as_str()
                .unwrap()
                .contains("database did not")
                || body["reason"]
                    .as_str()
                    .unwrap()
                    .contains("database rejected"),
            "{body}"
        );
        // A bare platform check converts, with no alarm.
        let (_, body) = probe(PlatformCheck::Accepted.into()).await;
        assert_ne!(body["reason"], "database role refused");
    }
}
