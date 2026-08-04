#[cfg(windows)]
mod everything;
mod handler;
mod native;
#[cfg(all(test, windows))]
mod parity;
mod results;
mod tools;

use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_COMMIT: &str = match option_env!("GIT_COMMIT") {
    Some(value) => value,
    None => "dev",
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("instant-file-search-mcp-server {APP_VERSION} {BUILD_COMMIT}");
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("EVERYTHING_MCP_LOG"))
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(version = APP_VERSION, commit = BUILD_COMMIT, "instant-file-search-mcp-server starting");
    let handler = handler::EverythingHandler;
    let transport = rmcp::transport::stdio();
    handler.serve(transport).await?.waiting().await?;
    Ok(())
}
