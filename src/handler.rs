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
    #[tool(description = "INSTANT file/directory search using Everything engine (NTFS index). Fastest way to find files on this PC. Default scope: ALL indexed drives (C:, D:, etc.). Supports regex, sort, path filter, exclude_path, selective fields, match_case/match_whole_word/match_path, and pagination via offset. Response includes: results, total (total matches), returned (count in this page), offset (pagination position), and note (exclusion info). Use this INSTEAD of glob/Get-ChildItem for any filesystem search. IMPORTANT: for broad patterns, call count_files FIRST to gauge result size. Use exclude_path to skip node_modules, WinSxS, etc. Pass include_all=true to search without auto-exclusions.")]
    async fn find_files(
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

    #[tool(description = "INSTANT count of matching files using Everything engine (NTFS index). Returns total count without transferring file data. Default scope: ALL indexed drives — pass path to narrow. Supports regex, match_case, match_whole_word, exclude_path, and include_all. Use this FIRST for broad patterns (e.g. *.tmp, *.json) to gauge total before calling find_files. Always prefer this over recursive shell commands to count files.")]
    async fn count_files(
        &self,
        Parameters(params): Parameters<CountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (count, note) = tokio::task::spawn_blocking(move || everything::count(params))
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = serde_json::json!({ "total": count, "note": note }).to_string();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Check if Everything search engine is running and IPC is connected. Returns detailed diagnostics including window status, IPC availability, and DB load state. Call this BEFORE using find_files or count_files to verify the engine is available.")]
    async fn search_status(&self) -> Result<CallToolResult, ErrorData> {
        let status = tokio::task::spawn_blocking(everything::status)
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = serde_json::to_string_pretty(&status)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for EverythingHandler {}
