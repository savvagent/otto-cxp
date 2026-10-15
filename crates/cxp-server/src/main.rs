//! otto-cxp server binary: reads the configuration, binds the port, and runs
//! [`cxp_server::run`] until SIGINT or SIGTERM. Everything else (boot checks,
//! background tasks, the bounded drain and final usage flush) lives in the
//! library, where the e2e tests run it whole.
//!
//! `cxp-server migrate` instead applies the migrations as the database owner
//! ([`cxp_server::migrate`]) and exits, binding no port: it is Fly's release
//! command (`release_command = "migrate"`, given to the image's entrypoint).

use anyhow::{Context, Result};
use cxp_server::{migrate, run, Config, LogFormat};
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Decided before anything else, so a mistyped release command never
    // falls through to serving.
    let migrating = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("migrate") => true,
        Some(other) => anyhow::bail!("unknown argument {other:?}; usage: cxp-server [migrate]"),
    };
    // Never overrides a variable already set, so a deployment's environment
    // always wins over a stray .env.
    let dotenv = dotenvy::dotenv();
    let config = Config::from_env().context(
        "configuration is incomplete. Copy .env.example to .env for local runs, or set the \
         variables named above in the deployment",
    )?;
    init_tracing(config.log_format);
    if let Ok(path) = dotenv {
        tracing::debug!(path = %path.display(), "loaded .env");
    }

    if migrating {
        // An error exits non-zero, which aborts the deploy.
        migrate(&config).await?;
        return Ok(());
    }

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("could not bind {}", config.bind))?;
    // An error here (a refused boot, or a background task that died while
    // serving) exits non-zero, so the machine is restarted.
    run(config, listener, shutdown_signal()).await?;
    Ok(())
}

fn init_tracing(format: LogFormat) {
    // An unset RUST_LOG means "info"; a set but unreadable one is a mistake
    // worth saying so, on stderr since there is no subscriber yet.
    let filter = match std::env::var(EnvFilter::DEFAULT_ENV) {
        Err(_) => EnvFilter::new("info"),
        Ok(spec) => EnvFilter::try_new(&spec).unwrap_or_else(|e| {
            eprintln!("RUST_LOG={spec:?} is not a valid filter ({e}); logging at info instead");
            EnvFilter::new("info")
        }),
    };
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Text => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
}

/// SIGTERM is what a container runtime sends before SIGKILL; handling only
/// SIGINT would hard-kill every deploy and skip the usage flush.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("SIGINT handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!("SIGINT; shutting down"),
        _ = terminate => tracing::info!("SIGTERM; shutting down"),
    }
}
