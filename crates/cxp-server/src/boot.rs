//! The process lifecycle, as a library function so a test can run it whole:
//! connect, check the database role (a deployment refuses a privileged one,
//! one that reaches another service's database, or one that owns anything
//! or may `TRUNCATE` a table), migrate (locally only: a deployment migrates
//! as the database owner through [`migrate`], Fly's release command), prove
//! tenant isolation (or refuse to serve), check the platform credential,
//! start the background tasks (the sweeper re-checks the role hourly,
//! [`recheck_db_role`]), serve until told to stop, then drain and flush
//! within bounds.
//!
//! Fly gives a stopping machine `kill_timeout = 30` seconds before SIGKILL;
//! the drain ([`DRAIN_TIMEOUT`]) and the final usage flush
//! ([`FLUSH_TIMEOUT`]) together stay inside it.

use std::future::{Future, IntoFuture};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use cxp_core::db_role::DbRole;
use cxp_core::usage::{ShipError, ShipperConfig};
use otto_resource::{PlatformClient, TokenClaims};
use otto_tenant::Db;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinError;
use uuid::Uuid;

use crate::health::{Readiness, RoleAlarm};
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
    /// [`DbRole::summary`] of the role this process connects as, as logged.
    pub database_role: String,
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

/// What the database-role check makes of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to refuse.
    Clean,
    /// Refused in a deployment, but this is a developer's machine: warned.
    AllowedLocally,
    /// Refused: a deployment, or an instance shared with another service.
    Refused,
}

/// Whether `role` may serve. A session whose login user or current user is,
/// or can `SET ROLE` to, a superuser-equivalent role, or can connect to
/// another service's database (`cxp_core::db_role`), escapes every unpinned
/// row-level policy, or on the shared `otto-db` reaches the other services'
/// data. One that owns anything here, or may `TRUNCATE` or add a `TRIGGER`
/// to a table, can switch row-level security off (`NO FORCE`, `DROP
/// POLICY`) or empty a table past it, with no privilege at all: the serving
/// role must own nothing, and migrations run as the owner through
/// [`migrate`].
///
/// A deployment is [`Config::is_deployment`], or an instance that also hosts
/// another otto service's database ([`DbRole::shares_the_instance`]): a
/// local run pointed at `otto-db` through `fly proxy` has loopback URLs but
/// production's data.
pub fn role_verdict(role: &DbRole, config: &Config) -> Verdict {
    verdict(
        role.is_refused_in_a_deployment() || role.can_alter_tables(),
        role,
        config,
    )
}

/// [`role_verdict`] for the migrating role, which is meant to own the
/// schema: only a privilege or another service's database refuses it.
fn migration_verdict(role: &DbRole, config: &Config) -> Verdict {
    verdict(role.is_refused_in_a_deployment(), role, config)
}

fn verdict(refusable: bool, role: &DbRole, config: &Config) -> Verdict {
    if !refusable {
        Verdict::Clean
    } else if is_deployment(role, config) {
        Verdict::Refused
    } else {
        Verdict::AllowedLocally
    }
}

/// [`Config::is_deployment`], or an instance shared with another otto
/// service, whatever the URLs say.
fn is_deployment(role: &DbRole, config: &Config) -> bool {
    config.is_deployment() || role.shares_the_instance()
}

/// What the warning says when a developer's machine is allowed a role a
/// deployment refuses.
const ALLOWED_LOCALLY: &str = "allowed only because this is not a deployment (loopback \
    CXP_PUBLIC_URL and CXP_RESOURCE_URI, no FLY_APP_NAME, and no other otto service's database \
    on the instance); a deployment refuses it";

/// Whether this process may serve on the database session it connected
/// with ([`role_verdict`]), and log what that session's users are. A
/// deployment refuses; a developer's machine, whose local Postgres hands out
/// a superuser that also owns the schema, is warned.
pub fn check_db_role(role: &DbRole, config: &Config) -> Result<()> {
    let summary = role.summary();
    match role_verdict(role, config) {
        Verdict::Clean => {
            tracing::info!("{summary}");
            Ok(())
        }
        Verdict::AllowedLocally => {
            tracing::warn!(public_url = %config.public_url, "{summary}: {ALLOWED_LOCALLY}");
            Ok(())
        }
        Verdict::Refused => {
            tracing::error!(public_url = %config.public_url, resource_uri = %config.resource_uri,
                fly_app_name = config.fly_app_name.as_deref().unwrap_or(""),
                "{summary}: refusing to serve. A deployment must log in as a dedicated runtime \
                 role that holds no superuser-equivalent privilege, can connect to no other \
                 service's database, and owns nothing in this database; migrations run as the \
                 database owner through `cxp-server migrate` (docs/deploy/fly.md)");
            Err(anyhow!(
                "refusing to serve: {summary}; a deployment must log in as a role that is not a \
                 superuser, holds no BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION, or \
                 cross-service predefined role, directly or through membership, can connect to \
                 no other service's database, and owns nothing in this database and may \
                 TRUNCATE or add a TRIGGER to no table: the runtime role must own nothing, and \
                 migrations run as the database owner through `cxp-server migrate` \
                 (docs/deploy/fly.md)"
            ))
        }
    }
}

/// [`check_db_role`] for the role `cxp-server migrate` connects as: the
/// owner may own (that is its job), but a deployment still refuses a
/// superuser-equivalent one or one that reaches another service's database.
pub fn check_migration_role(role: &DbRole, config: &Config) -> Result<()> {
    let summary = role.summary();
    match migration_verdict(role, config) {
        Verdict::Clean => {
            tracing::info!("{summary}");
            Ok(())
        }
        Verdict::AllowedLocally => {
            tracing::warn!(public_url = %config.public_url, "{summary}: {ALLOWED_LOCALLY}");
            Ok(())
        }
        Verdict::Refused => {
            tracing::error!(public_url = %config.public_url,
                fly_app_name = config.fly_app_name.as_deref().unwrap_or(""),
                "{summary}: refusing to migrate. A deployment migrates as the database owner, a \
                 role that holds no superuser-equivalent privilege and can connect to no other \
                 service's database (docs/deploy/fly.md)");
            Err(anyhow!(
                "refusing to migrate: {summary}; a deployment migrates as a role that is not a \
                 superuser, holds no BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION, or \
                 cross-service predefined role, directly or through membership, and can connect \
                 to no other service's database (docs/deploy/fly.md)"
            ))
        }
    }
}

/// What `CXP_MIGRATION_DATABASE_URL` missing in a deployment says.
const MIGRATION_URL_REQUIRED: &str = "CXP_MIGRATION_DATABASE_URL is required in a deployment: \
    migrations run as the database owner, never as the runtime role";

/// The URL [`migrate`] connects with: the owner's
/// ([`Config::migration_database_url`]), or, only on a developer's machine,
/// where one role both owns and serves, the runtime one.
pub fn migration_url(config: &Config) -> Result<&str> {
    match &config.migration_database_url {
        Some(url) => Ok(url),
        None if !config.is_deployment() => Ok(&config.database_url),
        None => Err(anyhow!("{MIGRATION_URL_REQUIRED}")),
    }
}

/// `cxp-server migrate`, Fly's release command: apply the migrations as the
/// database owner, on a short-lived machine, before the new release's
/// machines start, then exit. A failure aborts the deploy and leaves the old
/// release serving. Returns the migrating role's [`DbRole::summary`].
///
/// The serving process never migrates in a deployment (its role owns
/// nothing, so it cannot), and never reads the owner's credential.
pub async fn migrate(config: &Config) -> Result<String> {
    let url = migration_url(config)?;
    // Two connections: the migrator and sqlx's advisory lock, on a database
    // shared with every otto service.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
        .context("could not connect to the database to migrate")?;
    let db = Db::from_pool(pool);
    let result = migrate_on(&db, config).await;
    db.pool().close().await;
    result
}

async fn migrate_on(db: &Db, config: &Config) -> Result<String> {
    let role = cxp_core::db_role::inspect(db)
        .await
        .context("could not read the database role's privileges")?;
    // On an instance shared with another service, a loopback configuration
    // (a `fly proxy` run) is a deployment here too: it needs the owner's own
    // credential.
    if config.migration_database_url.is_none() && role.shares_the_instance() {
        let summary = role.summary();
        tracing::error!("{summary}: {MIGRATION_URL_REQUIRED}");
        return Err(anyhow!(
            "refusing to migrate: {summary}; {MIGRATION_URL_REQUIRED}"
        ));
    }
    check_migration_role(&role, config)?;
    tracing::info!("applying migrations");
    cxp_core::migrate(db).await.context("migrations failed")?;
    tracing::info!("migrations applied");
    Ok(role.summary())
}

/// Re-run the serving check ([`role_verdict`]) on the live database and
/// raise or lower `alarm`, which holds `/readyz` back while raised
/// ([`crate::health`]). Called by the sweeper after each pass, so a grant
/// made after boot is noticed within the hour. A failed read leaves the
/// alarm as it was: `/readyz` reports an unreachable database itself.
pub async fn recheck_db_role(db: &Db, config: &Config, alarm: &RoleAlarm) {
    let role = match cxp_core::db_role::inspect(db).await {
        Ok(role) => role,
        Err(e) => {
            tracing::warn!(error = %e.log_detail(), source = ?std::error::Error::source(&e),
                "could not re-check the database role; the readiness alarm is unchanged");
            return;
        }
    };
    match role_verdict(&role, config) {
        Verdict::Refused => {
            let summary = role.summary();
            tracing::error!(
                "the database role changed after boot: {summary}; /readyz answers 503 until it \
                 is fixed"
            );
            alarm.raise(summary);
        }
        Verdict::Clean | Verdict::AllowedLocally => {
            if alarm.clear() {
                tracing::info!(
                    "the database role re-check passes again; /readyz no longer holds back"
                );
            }
        }
    }
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
    // Serving never uses the migration owner's credential: drop any copy so
    // nothing below can reach it (main has already removed it from the
    // environment, `secret_env`).
    let config = Config {
        migration_database_url: None,
        ..config
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(cxp_core::POOL_ACQUIRE_TIMEOUT)
        .connect(&config.database_url)
        .await
        .context("could not connect to the database")?;
    let db = Db::from_pool(pool);

    // Before anything runs as this role, migrations included.
    let role = cxp_core::db_role::inspect(&db)
        .await
        .context("could not read the database role's privileges")?;
    check_db_role(&role, &config)?;

    // The runtime role owns nothing, so in a deployment it cannot migrate:
    // the release command does, as the owner.
    let deployment = is_deployment(&role, &config);
    if config.run_migrations && deployment {
        let why = "refusing to serve: CXP_RUN_MIGRATIONS is on in a deployment; the runtime role \
                   cannot migrate: set CXP_RUN_MIGRATIONS=false and migrate with cxp-server \
                   migrate (the release command)";
        tracing::error!("{why}");
        return Err(anyhow!("{why}"));
    }
    if config.run_migrations {
        tracing::info!("applying migrations");
        cxp_core::migrate(&db).await.context("migrations failed")?;
    } else if deployment {
        tracing::info!(
            "not migrating: migrations run as the database owner through cxp-server migrate"
        );
    } else {
        tracing::warn!("CXP_RUN_MIGRATIONS is off; assuming the schema is already current");
    }

    // org.deleted writes its tombstone as this role, outside any tenant
    // transaction: without the grant, every such webhook would fail.
    let missing = cxp_core::db_role::missing_runtime_grants(&db)
        .await
        .context("could not read the database role's grants")?;
    if !missing.is_empty() {
        let why = format!(
            "the runtime role cannot write the org.deleted tombstone ({}): GRANT cxp_runtime TO {}",
            missing.join(", "),
            sql_identifier(&role.current_user)
        );
        if deployment {
            tracing::error!("{why}: refusing to serve");
            return Err(anyhow!("refusing to serve: {why}"));
        }
        tracing::warn!("{why}: allowed only because this is not a deployment");
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
    let alarm = RoleAlarm::default();
    let app = router(
        db.clone(),
        platform.clone(),
        meter(platform.clone(), &config),
        &config,
        Readiness::new(check.clone()).with_role_alarm(alarm.clone()),
    )?;

    let (stop, stopped) = watch::channel(false);
    let mut shipper = tokio::spawn(cxp_core::usage::run(
        db.clone(),
        platform,
        shipper_config,
        stopped.clone(),
    ));
    let mut sweeper = tokio::spawn(sweep_loop(db, config.clone(), alarm, stopped));

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
        r = &mut sweeper => Some(ended(SWEEPER, r.map(Ok))),
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
            database_role: role.summary(),
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

/// What the log and the exit error call [`sweep_loop`], which does more
/// than sweep platform events.
const SWEEPER: &str = "the sweeper (retention and database-role re-check)";

/// `name` as a quoted SQL identifier, ready to paste into a statement:
/// always double-quoted, with every embedded double quote doubled, so a name
/// Postgres would fold (`Cxp`), misparse (`cxp-runtime`), or read as a
/// keyword (`user`) pastes as written.
fn sql_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
/// usage past `REJECTED_USAGE_KEEP_DAYS`; then the database-role re-check
/// ([`recheck_db_role`]). The first pass runs at boot, right after the boot
/// check, and so finds nothing new.
async fn sweep_loop(db: Db, config: Config, alarm: RoleAlarm, mut shutdown: watch::Receiver<bool>) {
    loop {
        match cxp_core::platform_events::sweep(&db, PLATFORM_EVENT_RETENTION_DAYS).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(rows = n, "swept old platform-event records"),
            Err(e) => tracing::warn!(error = %e.log_detail(),
                source = ?std::error::Error::source(&e), "retention sweep failed"),
        }
        recheck_db_role(&db, &config, &alarm).await;
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

    /// A deployment refuses every superuser-equivalent privilege, held by
    /// the login (session) user or the current user, directly or through
    /// membership; a developer's machine only warns.
    #[test]
    fn a_deployment_refuses_a_privileged_database_role() {
        use cxp_core::db_role::Privilege::*;
        let deployed = Config::for_test();
        assert!(deployed.is_deployment());
        let mut local = Config::for_test();
        local.public_url = "http://localhost:8080".into();
        local.resource_uri = "http://localhost:8080/mcp".into();
        assert!(!local.is_deployment());

        let least = DbRole::for_test("cxp_server", "cxp_server", &[]);
        assert!(check_db_role(&least, &deployed).is_ok());
        assert!(check_db_role(&least, &local).is_ok());

        for r in [
            DbRole::for_test("s", "s", &[("s", "s", &[Superuser])]),
            DbRole::for_test("s", "s", &[("s", "s", &[BypassRls])]),
            DbRole::for_test("s", "s", &[("s", "s", &[CreateRole])]),
            DbRole::for_test("s", "s", &[("s", "s", &[Replication])]),
            DbRole::for_test("s", "s", &[("s", "postgres", &[Superuser, BypassRls])]),
            DbRole::for_test("s", "s", &[("s", "s", &[CreateDb])]),
            DbRole::for_test("s", "s", &[("s", "pg_read_all_data", &[PredefinedRole])]),
            DbRole::for_test("s", "s", &[("s", "pg_signal_backend", &[PredefinedRole])]),
            // A superuser login that SET ROLEs to a plain role on connect.
            DbRole::for_test("otto_cxp", "s", &[("otto_cxp", "otto_cxp", &[Superuser])]),
        ] {
            let err = format!("{:#}", check_db_role(&r, &deployed).unwrap_err());
            assert!(err.contains("refusing to serve"), "{err}");
            assert!(err.contains(&r.summary()), "{err}");
            assert!(check_db_role(&r, &local).is_ok(), "{}", r.summary());
        }

        // Loopback URLs on a Fly machine are still a deployment.
        let mut on_fly = local.clone();
        on_fly.fly_app_name = Some("otto-cxp".into());
        let su = DbRole::for_test("s", "s", &[("s", "s", &[Superuser])]);
        assert!(check_db_role(&su, &on_fly).is_err());
        // So is a loopback public URL serving a real audience.
        let mut real_audience = local.clone();
        real_audience.resource_uri = "https://otto-cxp.savvagent.com/mcp".into();
        assert!(check_db_role(&su, &real_audience).is_err());

        // Another service's database within reach is refused like a
        // privilege, and only warned about locally.
        let mut reach = DbRole::for_test("s", "s", &[]);
        reach.other_databases = vec![cxp_core::db_role::DatabaseAccess {
            user: "s".into(),
            database: "otto_platform".into(),
            through: "otto_app".into(),
        }];
        let err = format!("{:#}", check_db_role(&reach, &deployed).unwrap_err());
        assert!(
            err.contains(r#"can connect to database "otto_platform""#),
            "{err}"
        );
        assert!(check_db_role(&reach, &local).is_ok());

        // An instance shared with another otto service is a deployment,
        // whatever the URLs say (a local run through `fly proxy`)...
        let mut proxied = su.clone();
        proxied.shared_with = vec!["otto_platform".into()];
        let err = format!("{:#}", check_db_role(&proxied, &local).unwrap_err());
        assert!(
            err.contains(r#"the instance also hosts "otto_platform""#),
            "{err}"
        );
        // ...where a least-privilege role still serves.
        let mut clean = least.clone();
        clean.shared_with = vec!["otto_platform".into()];
        assert!(check_db_role(&clean, &local).is_ok());
        assert!(check_db_role(&clean, &deployed).is_ok());
    }

    /// [`Config::for_test`] with loopback URLs: not a deployment.
    fn local_config() -> Config {
        let mut local = Config::for_test();
        local.public_url = "http://localhost:8080".into();
        local.resource_uri = "http://localhost:8080/mcp".into();
        assert!(!local.is_deployment());
        local
    }

    /// The database owner (what #16's runbook left production connecting
    /// as).
    fn owner() -> DbRole {
        let mut r = DbRole::for_test("cxp_server", "cxp_server", &[]);
        r.owned = vec![cxp_core::db_role::OwnedObject {
            user: "cxp_server".into(),
            kind: "table",
            name: "customers".into(),
            owner: "cxp_server".into(),
        }];
        r.owned_total = 40;
        r
    }

    /// A role that owns nothing but was granted `TRUNCATE` on a table.
    fn truncate_holder() -> DbRole {
        let mut r = DbRole::for_test("cxp_server", "cxp_server", &[]);
        r.table_privileges = vec![cxp_core::db_role::TablePrivilege {
            user: "cxp_server".into(),
            table: "customers".into(),
            privileges: vec!["TRUNCATE"],
        }];
        r
    }

    /// Every serving verdict: a clean role serves anywhere; a privileged
    /// one, one reaching another database, an owner, and a `TRUNCATE`
    /// holder are refused in a deployment and on a shared instance, and
    /// only allowed (with a warning) on a developer's machine.
    #[test]
    fn the_serving_verdict_refuses_whatever_can_step_around_rls() {
        use cxp_core::db_role::Privilege::*;
        let deployed = Config::for_test();
        let local = local_config();
        let shared = |mut r: DbRole| {
            r.shared_with = vec!["otto_platform".into()];
            r
        };
        let mut reach = DbRole::for_test("s", "s", &[]);
        reach.other_databases = vec![cxp_core::db_role::DatabaseAccess {
            user: "s".into(),
            database: "otto_flags".into(),
            through: "PUBLIC".into(),
        }];
        let clean = DbRole::for_test("cxp_server", "cxp_server", &[]);
        assert_eq!(role_verdict(&clean, &deployed), Verdict::Clean);
        assert_eq!(role_verdict(&clean, &local), Verdict::Clean);
        assert_eq!(role_verdict(&shared(clean), &local), Verdict::Clean);
        for (what, r) in [
            (
                "privileged",
                DbRole::for_test("s", "s", &[("s", "s", &[Superuser])]),
            ),
            ("other databases", reach),
            ("owner", owner()),
            ("TRUNCATE holder", truncate_holder()),
        ] {
            assert_eq!(role_verdict(&r, &deployed), Verdict::Refused, "{what}");
            assert_eq!(role_verdict(&r, &local), Verdict::AllowedLocally, "{what}");
            assert_eq!(role_verdict(&shared(r), &local), Verdict::Refused, "{what}");
        }
    }

    /// A deployment refuses an owning runtime role, saying migrations run
    /// as the owner through `cxp-server migrate`; a developer's machine,
    /// where one role does both, only warns.
    #[test]
    fn a_deployment_refuses_an_owning_runtime_role() {
        for r in [owner(), truncate_holder()] {
            let err = format!("{:#}", check_db_role(&r, &Config::for_test()).unwrap_err());
            assert!(err.contains("refusing to serve"), "{err}");
            assert!(err.contains(&r.summary()), "{err}");
            assert!(err.contains("cxp-server migrate"), "{err}");
            assert!(err.contains("owns nothing"), "{err}");
            assert!(check_db_role(&r, &local_config()).is_ok());
        }
    }

    /// The migrating role is meant to own the schema; only a privilege or
    /// another service's database refuses it, in a deployment or on a
    /// shared instance.
    #[test]
    fn the_migration_role_may_own_but_not_be_privileged() {
        use cxp_core::db_role::Privilege::*;
        let deployed = Config::for_test();
        assert!(check_migration_role(&owner(), &deployed).is_ok());
        assert!(check_migration_role(&truncate_holder(), &deployed).is_ok());
        let su = DbRole::for_test("s", "s", &[("s", "s", &[Superuser])]);
        let err = format!("{:#}", check_migration_role(&su, &deployed).unwrap_err());
        assert!(err.contains("refusing to migrate"), "{err}");
        assert!(err.contains(&su.summary()), "{err}");
        assert!(check_migration_role(&su, &local_config()).is_ok());
        let mut proxied = su.clone();
        proxied.shared_with = vec!["otto_platform".into()];
        assert!(check_migration_role(&proxied, &local_config()).is_err());
    }

    /// `migrate` uses the owner's URL when set; without it, a developer's
    /// machine migrates as its one role and a deployment refuses.
    #[test]
    fn the_migration_url_is_the_owners_and_required_in_a_deployment() {
        let owner_url = "postgres://cxp_owner:x@db/otto_cxp";
        let mut deployed = Config::for_test();
        let err = format!("{:#}", migration_url(&deployed).unwrap_err());
        assert!(
            err.contains("CXP_MIGRATION_DATABASE_URL is required"),
            "{err}"
        );
        assert!(err.contains("never as the runtime role"), "{err}");
        deployed.migration_database_url = Some(owner_url.into());
        assert_eq!(migration_url(&deployed).unwrap(), owner_url);

        let mut local = local_config();
        assert_eq!(migration_url(&local).unwrap(), local.database_url);
        local.migration_database_url = Some(owner_url.into());
        assert_eq!(migration_url(&local).unwrap(), owner_url);
    }

    /// The suggested `GRANT` pastes as is: the name is always quoted, so a
    /// keyword, a mixed-case or punctuated name all survive, embedded quotes
    /// doubled.
    #[test]
    fn a_role_name_is_always_quoted() {
        assert_eq!(sql_identifier("cxp_server"), r#""cxp_server""#);
        assert_eq!(sql_identifier("user"), r#""user""#);
        assert_eq!(sql_identifier("Cxp"), r#""Cxp""#);
        assert_eq!(sql_identifier("cxp-runtime"), r#""cxp-runtime""#);
        assert_eq!(sql_identifier(r#"a"b"#), r#""a""b""#);
    }

    #[test]
    fn a_background_task_that_ends_is_named_in_the_exit_error() {
        let err = ended(SWEEPER, Ok(Ok(()))).to_string();
        assert_eq!(
            err,
            "the sweeper (retention and database-role re-check) ended while serving: it returned"
        );
    }

    #[test]
    fn the_drain_and_the_flush_fit_inside_flys_kill_timeout() {
        assert!(DRAIN_TIMEOUT + FLUSH_TIMEOUT < Duration::from_secs(30));
    }
}
