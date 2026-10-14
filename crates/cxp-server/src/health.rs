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

use std::sync::Arc;
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

#[derive(Clone)]
struct Ready {
    db: Db,
    /// What boot's platform credential check found, shared into the router.
    platform: Arc<PlatformCheck>,
}

/// The health routes. `platform` is the result of
/// [`crate::boot::check_platform`], which `/readyz` reports.
pub fn router(db: Db, platform: PlatformCheck) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(Ready {
            db,
            platform: Arc::new(platform),
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
}
