use anyhow::Result;
use rmcp::ServiceExt;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use cxp_server::api::ApiState;
use cxp_server::config::{Config, Transport};
use cxp_server::db;
use cxp_server::tools::CxpServer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;

    tracing::info!("Connecting to database...");
    let pool = db::pool::create_pool(&config.database_url).await?;

    tracing::info!("Running migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;

    let server = CxpServer::new(pool.clone());

    match config.transport {
        Transport::Stdio => {
            tracing::info!("Starting CXP MCP server on stdio");
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();
            let service = server.serve((stdin, stdout)).await?;
            service.waiting().await?;
        }
        Transport::Http => {
            tracing::info!(
                "Starting CXP MCP server on http://{}:{}",
                config.http_host,
                config.http_port
            );
            let addr = format!("{}:{}", config.http_host, config.http_port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;

            let server_clone = server.clone();
            let streamable_http_service =
                rmcp::transport::StreamableHttpService::new(
                    move || Ok::<_, std::io::Error>(server_clone.clone()),
                    Arc::new(rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default()),
                    Default::default(),
                );

            let api_state = ApiState::new(pool);
            let api_routes = cxp_server::api::router(api_state);

            let app = axum::Router::new()
                .nest_service("/mcp", streamable_http_service)
                .merge(api_routes);

            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
