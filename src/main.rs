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
    // Support triage from one command: what the server advertises, which tools
    // it serves, and whether each engine answers. Prints to stdout and exits
    // without starting a transport.
    if std::env::args().any(|arg| arg == "--self-test") {
        return self_test();
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

/// `--self-test`: report the initialize contract, the tool list, and engine
/// reachability without opening a transport.
fn self_test() -> anyhow::Result<()> {
    use rmcp::ServerHandler;

    println!("instant-file-search-mcp-server {APP_VERSION} {BUILD_COMMIT}");
    let info = handler::EverythingHandler.get_info();
    println!(
        "initialize.capabilities.tools: {}",
        info.capabilities.tools.is_some()
    );
    let names = handler::tool_names();
    println!("tools ({}): {}", names.len(), names.join(", "));
    match native::status() {
        Ok(value) => println!("native: {}", serde_json::to_string(&value)?),
        Err(error) => println!("native: unavailable ({error})"),
    }
    #[cfg(windows)]
    match everything::status() {
        Ok(value) => println!("everything: {}", serde_json::to_string(&value)?),
        Err(error) => println!("everything: unavailable ({error})"),
    }
    Ok(())
}
