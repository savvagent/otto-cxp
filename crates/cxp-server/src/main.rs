//! otto-cxp server binary: reads the configuration, binds the port, and runs
//! [`cxp_server::run`] until SIGINT or SIGTERM. Everything else (boot checks,
//! background tasks, the bounded drain and final usage flush) lives in the
//! library, where the e2e tests run it whole.

use anyhow::{Context, Result};
use cxp_server::{run, Config, LogFormat};
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
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

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("could not bind {}", config.bind))?;
    // An error here (a refused boot, or a background task that died while
    // serving) exits non-zero, so the machine is restarted.
    run(config, listener, shutdown_signal()).await?;
    Ok(())
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
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
