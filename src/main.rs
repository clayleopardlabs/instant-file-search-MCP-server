mod everything;
mod handler;
mod tools;

use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("EVERYTHING_MCP_LOG"))
        .init();

    tracing::info!("everything-mcp-server starting");
    let handler = handler::EverythingHandler;
    let transport = rmcp::transport::stdio();
    handler.serve(transport).await?.waiting().await?;
    Ok(())
}
