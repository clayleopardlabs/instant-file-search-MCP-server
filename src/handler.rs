use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ErrorData},
    ServerHandler, tool, tool_handler, tool_router,
};
use tracing::error;

use crate::everything;
use crate::tools::{CountParams, SearchParams};

#[derive(Clone, Default)]
pub struct EverythingHandler;

#[tool_router]
impl EverythingHandler {
    #[tool(description = "Instant file/directory search using Everything engine. Supports regex, sort, path filter, and selective fields.")]
    async fn everything_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = tokio::task::spawn_blocking(move || everything::search(params))
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Fast count of matching files without data transfer")]
    async fn everything_count(
        &self,
        Parameters(params): Parameters<CountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let count = tokio::task::spawn_blocking(move || everything::count(params))
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = serde_json::json!({ "total": count }).to_string();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Check if Everything is running and IPC is connected")]
    async fn everything_status(&self) -> Result<CallToolResult, ErrorData> {
        let connected = tokio::task::spawn_blocking(everything::is_running)
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?;

        let text = serde_json::json!({ "connected": connected }).to_string();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for EverythingHandler {}
