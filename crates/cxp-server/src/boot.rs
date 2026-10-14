//! The process lifecycle, as a library function so a test can run it whole:
//! connect, migrate, prove tenant isolation (or refuse to serve), check the
//! platform credential, start the background tasks, serve until told to
//! stop, then drain and flush within bounds.
//!
//! Fly gives a stopping machine `kill_timeout = 30` seconds before SIGKILL;
//! the drain ([`DRAIN_TIMEOUT`]) and the final usage flush
//! ([`FLUSH_TIMEOUT`]) together stay inside it.

use std::future::{Future, IntoFuture};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use cxp_core::usage::{ShipError, ShipperConfig};
use otto_resource::{PlatformClient, TokenClaims};
use otto_tenant::Db;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinError;
use uuid::Uuid;

use crate::{meter, platform_client, router, Config};

/// How long in-flight requests get to finish after the stop signal.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the shipper's final pass gets after the server has drained.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `platform_events` dedupe markers are kept.
pub const PLATFORM_EVENT_RETENTION_DAYS: i32 = 30;

/// What boot found, returned when the server stops cleanly.
#[derive(Debug, Clone)]
pub struct Boot {
    /// [`otto_tenant::IsolationReport::summary`], as logged.
    pub isolation: String,
    pub platform: PlatformCheck,
}

/// What the platform said about this service's credential at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformCheck {
    /// The platform authenticated this service and answered an
    /// introspection.
    Accepted,
    /// The platform refused this service's credential (401).
    Rejected,
    /// Something answered, but not as the platform does: a wrong
    /// `CXP_PLATFORM_URL` (404, an HTML page, ...).
    Misconfigured(String),
    /// No answer, or a server error: an outage, which must not stop a boot.
    Unreachable(String),
}

/// Classify the answer to a probe introspection. A random well-formed token
/// is inactive, so a platform that accepts the credential answers
/// `Ok(None)`; only the platform's introspection endpoint answers that way,
/// which is why this probes it rather than a lookup where a 404 would also be
/// "fine".
pub fn classify(probe: otto_resource::Result<Option<TokenClaims>>) -> PlatformCheck {
    use otto_resource::Error;
    match probe {
        Ok(_) => PlatformCheck::Accepted,
        Err(Error::Unauthorized) => PlatformCheck::Rejected,
        Err(e) if e.is_retriable() => PlatformCheck::Unreachable(e.to_string()),
        Err(e) => PlatformCheck::Misconfigured(e.to_string()),
    }
}

/// A token string the platform has never issued, shaped like one so the
/// client actually asks (it answers malformed strings itself).
fn probe_token() -> String {
    let random = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token = format!("otto_at_{}", &random[..43]);
    debug_assert!(otto_resource::looks_like_token(&token));
    token
}

/// Ask the platform to introspect a token nobody holds, and log what that
/// says about this service's credential. Never fatal: a platform outage must
/// not stop a restart.
pub async fn check_platform(platform: &PlatformClient, config: &Config) -> PlatformCheck {
    let check = classify(platform.introspect(&probe_token()).await);
    match &check {
        PlatformCheck::Accepted => tracing::info!(
            platform = %config.platform_url,
            "the otto platform accepted this service's credential"
        ),
        PlatformCheck::Rejected => tracing::error!(
            platform = %config.platform_url,
            resource_uri = %config.resource_uri,
            "the otto platform REJECTED this service's credential: no token can be validated and \
             no usage can be shipped. Check CXP_INTROSPECTION_SECRET and that CXP_RESOURCE_URI is \
             the URI this service was registered with"
        ),
        PlatformCheck::Misconfigured(why) => tracing::error!(
            platform = %config.platform_url,
            error = %why,
            "CXP_PLATFORM_URL is misconfigured: it did not answer as the otto platform does, so \
             no token can be validated"
        ),
        PlatformCheck::Unreachable(why) => tracing::warn!(
            platform = %config.platform_url,
            error = %why,
            "could not reach the otto platform at startup; continuing"
        ),
    }
    check
}

/// Run the server on `listener` until `shutdown` completes, then drain
/// in-flight requests (at most [`DRAIN_TIMEOUT`]) and flush usage (at most
/// [`FLUSH_TIMEOUT`]).
///
/// An error before serving is a refusal to start. An error after is a
/// background task (the server, the usage shipper, the sweeper) that ended
/// on its own: the process should exit non-zero so the machine restarts,
/// rather than serve on without shipping usage or sweeping. So is a shutdown
/// that did not finish cleanly: in-flight requests dropped at the drain
/// deadline, or a final usage flush that failed for a reason that will not
/// pass on its own, or ran out of time (its rows stay in the outbox for the
/// next start). A flush that could not reach the platform is logged at
/// `warn` and does not fail the exit: the next start ships those rows.
pub async fn run<F>(config: Config, listener: TcpListener, shutdown: F) -> Result<Boot>
where
    F: Future<Output = ()> + Send + 'static,
{
    run_with(config, listener, shutdown, ShipperConfig::default()).await
}

/// [`run`] with the usage shipper's settings given, so a test can stretch its
/// poll interval and attribute what the platform receives to the final flush
/// rather than to a periodic pass racing the test.
pub async fn run_with<F>(
    config: Config,
    listener: TcpListener,
    shutdown: F,
    shipper_config: ShipperConfig,
) -> Result<Boot>
where
    F: Future<Output = ()> + Send + 'static,
{
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .context("could not connect to the database")?;
    let db = Db::from_pool(pool);

    if config.run_migrations {
        tracing::info!("applying migrations");
        cxp_core::migrate(&db).await.context("migrations failed")?;
    } else {
        tracing::warn!("CXP_RUN_MIGRATIONS is off; assuming the schema is already current");
    }

    // Prove tenant isolation before serving. The same migrations isolate
    // perfectly under one database role and not at all under another, and
    // only the running database can say which this is.
    let isolation = db
        .verify_tenant_isolation()
        .await
        .context("refusing to serve: tenant isolation is not enforced by this database")?
        .summary();
    tracing::info!("{isolation}");

    let platform = platform_client(&config)?;
    let check = check_platform(&platform, &config).await;
    let app = router(
        db.clone(),
        platform.clone(),
        meter(platform.clone(), &config),
        &config,
        check.clone(),
    )?;

    let (stop, stopped) = watch::channel(false);
    let mut shipper = tokio::spawn(cxp_core::usage::run(
        db.clone(),
        platform,
        shipper_config,
        stopped.clone(),
    ));
    let mut sweeper = tokio::spawn(sweep_loop(db, stopped));

    let (drain, mut draining) = watch::channel(false);
    let mut server = tokio::spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = draining.wait_for(|d| *d).await;
            })
            .into_future(),
    );
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        resource_uri = %config.resource_uri,
        platform_url = %config.platform_url,
        allowed_hosts = ?config.allowed_hosts(),
        enforce_quotas = config.enforce_quotas,
        "cxp-server listening"
    );

    // Serve until told to stop, or until something that must run for the
    // life of the process ends on its own.
    let (mut server_done, mut shipper_done) = (false, false);
    tokio::pin!(shutdown);
    let mut failure = tokio::select! {
        _ = &mut shutdown => None,
        r = &mut server => {
            server_done = true;
            Some(ended("the HTTP server", r.map(|r| r.map_err(anyhow::Error::from))))
        }
        r = &mut shipper => {
            shipper_done = true;
            Some(ended("the usage shipper", r.map(|r| r.map_err(anyhow::Error::from))))
        }
        r = &mut sweeper => Some(ended("the platform-event sweeper", r.map(Ok))),
    };

    // Drain: stop accepting, let in-flight requests finish, within bounds.
    let _ = drain.send(true);
    if !server_done {
        let drained = match tokio::time::timeout(DRAIN_TIMEOUT, &mut server).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(e))) => Err(format!("the server stopped with an error: {e}")),
            Ok(Err(e)) => Err(format!("the server task did not stop cleanly: {e}")),
            Err(_) => {
                server.abort();
                Err(format!(
                    "in-flight requests did not finish within {DRAIN_TIMEOUT:?} and were dropped"
                ))
            }
        };
        if let Err(why) = drained {
            tracing::error!(reason = %why, "the drain failed");
            failure.get_or_insert(anyhow!("{why}"));
        }
    }

    // Then the shipper's final pass, which sees every request's usage.
    tracing::info!("flushing usage");
    let _ = stop.send(true);
    if !shipper_done {
        let flushed = match tokio::time::timeout(FLUSH_TIMEOUT, &mut shipper).await {
            Ok(r) => flush_result(r),
            Err(_) => {
                shipper.abort();
                Err(format!(
                    "the final usage flush did not finish within {FLUSH_TIMEOUT:?}"
                ))
            }
        };
        if let Err(why) = flushed {
            tracing::error!(reason = %why,
                "usage was not fully flushed; unshipped rows stay in the outbox for the next start");
            failure.get_or_insert(anyhow!("{why}"));
        }
    }
    sweeper.abort();
    tracing::info!("stopped");

    match failure {
        Some(e) => Err(e),
        None => Ok(Boot {
            isolation,
            platform: check,
        }),
    }
}

/// What the shipper's final pass means for the exit. A failure that passes
/// on its own (the platform unreachable, failing, or rate limiting) is no
/// reason to exit non-zero: the rows wait in the outbox and the next start
/// ships them, and the shipper has logged it at `warn`. Anything else (the
/// credential refused, the outbox database failing, the task panicking)
/// needs an operator and fails the exit.
fn flush_result(
    r: std::result::Result<std::result::Result<(), ShipError>, JoinError>,
) -> std::result::Result<(), String> {
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) if e.is_retriable() => Ok(()),
        // Logged by the shipper itself.
        Ok(Err(e)) => Err(format!("the final usage flush failed: {e}")),
        Err(e) => Err(format!("the usage shipper did not stop cleanly: {e}")),
    }
}

/// A task that should have run until shutdown ended: log it at `error` and
/// turn it into the process's exit error.
fn ended(what: &str, result: std::result::Result<Result<()>, JoinError>) -> anyhow::Error {
    let why = match result {
        Ok(Ok(())) => "it returned".to_string(),
        Ok(Err(e)) => format!("it failed: {e:#}"),
        Err(e) if e.is_panic() => "it panicked".to_string(),
        Err(e) => format!("it was cancelled: {e}"),
    };
    tracing::error!(task = what, reason = %why, "a background task ended while serving; exiting");
    anyhow!("{what} ended while serving: {why}")
}

/// Hourly retention ([`cxp_core::platform_events::sweep`]): old
/// platform-event markers, expired removed-member tombstones, and rejected
/// usage past `REJECTED_USAGE_KEEP_DAYS`.
async fn sweep_loop(db: Db, mut shutdown: watch::Receiver<bool>) {
    loop {
        match cxp_core::platform_events::sweep(&db, PLATFORM_EVENT_RETENTION_DAYS).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(rows = n, "swept old platform-event records"),
            Err(e) => tracing::warn!(error = %e.log_detail(),
                source = ?std::error::Error::source(&e), "retention sweep failed"),
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => {}
            _ = shutdown.changed() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use otto_resource::{ClientConfig, IntrospectionResponse};

    /// A platform whose introspection endpoint answers `status` (with an
    /// inactive-token body when 200), or one that cannot be reached.
    async fn platform_answering(status: Option<StatusCode>) -> PlatformClient {
        let base = match status {
            Some(status) => {
                let app = Router::new().route(
                    "/oauth/introspect",
                    post(move || async move {
                        if status == StatusCode::OK {
                            Json(IntrospectionResponse::inactive()).into_response()
                        } else {
                            (status, "no").into_response()
                        }
                    }),
                );
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
                format!("http://{addr}")
            }
            // Bound and dropped: nothing listens there.
            None => {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                drop(listener);
                format!("http://{addr}")
            }
        };
        PlatformClient::new(ClientConfig::new(
            base,
            "https://cxp.example.com/mcp",
            "otto_rs_test",
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn the_credential_check_tells_accepted_from_rejected_from_misconfigured() {
        let config = Config::for_test();
        let config = &config;
        let check =
            |status| async move { check_platform(&platform_answering(status).await, config).await };
        assert_eq!(check(Some(StatusCode::OK)).await, PlatformCheck::Accepted);
        assert_eq!(
            check(Some(StatusCode::UNAUTHORIZED)).await,
            PlatformCheck::Rejected
        );
        // A wrong host that answers 404 is not "accepted".
        assert!(matches!(
            check(Some(StatusCode::NOT_FOUND)).await,
            PlatformCheck::Misconfigured(_)
        ));
        assert!(matches!(
            check(Some(StatusCode::INTERNAL_SERVER_ERROR)).await,
            PlatformCheck::Unreachable(_)
        ));
        assert!(matches!(check(None).await, PlatformCheck::Unreachable(_)));
    }

    #[test]
    fn the_probe_is_a_well_formed_token_nobody_holds() {
        let (a, b) = (probe_token(), probe_token());
        assert!(otto_resource::looks_like_token(&a), "{a}");
        assert_ne!(a, b);
    }

    /// A final flush the platform was unreachable for leaves its rows for the
    /// next start and exits cleanly; one that needs an operator fails the
    /// exit.
    #[test]
    fn only_a_flush_failure_that_will_not_pass_fails_the_exit() {
        let status = |status| {
            Ok(Err(ShipError::Platform(otto_resource::Error::Status {
                status,
                body: String::new(),
            })))
        };
        assert_eq!(flush_result(Ok(Ok(()))), Ok(()));
        for passes in [503, 502, 429, 408] {
            assert_eq!(flush_result(status(passes)), Ok(()), "{passes}");
        }
        let err = flush_result(Ok(Err(ShipError::Platform(
            otto_resource::Error::Unauthorized,
        ))))
        .unwrap_err();
        assert!(err.contains("final usage flush failed"), "{err}");
        assert!(flush_result(Ok(Err(ShipError::Db(sqlx::Error::PoolClosed)))).is_err());
        assert!(flush_result(Ok(Err(ShipError::Receipt("short".into())))).is_err());
    }

    #[test]
    fn the_drain_and_the_flush_fit_inside_flys_kill_timeout() {
        assert!(DRAIN_TIMEOUT + FLUSH_TIMEOUT < Duration::from_secs(30));
    }
}
