use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ErrorData},
    ServerHandler, tool, tool_handler, tool_router,
};
use tracing::error;

use crate::everything;
use crate::native;
use crate::tools::{AggregateParams, CountParams, RecentChangesParams, SearchParams};

#[derive(Clone, Default)]
pub struct EverythingHandler;

#[tool_router]
impl EverythingHandler {
    #[tool(description = "INSTANT file/directory search using the local NTFS index (native engine; falls back to the Everything engine when the native indexer is not running). Fastest way to find files on this PC. Default scope: ALL indexed drives (C:, D:, etc.). Supports regex, sort, path filter, exclude_path, selective fields, match_case/match_whole_word/match_path, and pagination via offset. EXCEEDS Everything: the query supports a `content:\"text\"` token to search file contents (native bounded content index, no Windows Search dependency; Everything's content: needs the Windows Search indexer). Response includes: results, total (total matches), returned (count in this page), offset (pagination position), and note (exclusion info). Use this INSTEAD of glob/Get-ChildItem for any filesystem search. IMPORTANT: for broad patterns, call count_files FIRST to gauge result size. Use exclude_path to skip node_modules, WinSxS, etc. Pass include_all=true to search without auto-exclusions.")]
    async fn find_files(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = tokio::task::spawn_blocking(move || {
            match native::search(&params) {
                Ok(r) => Ok(r),
                Err(e) => {
                    error!("native search failed, falling back to Everything: {e}");
                    everything::search(params)
                }
            }
        })
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

    #[tool(description = "INSTANT count of matching files using the local NTFS index (native engine; falls back to the Everything engine when the native indexer is not running). Returns total count without transferring file data. Default scope: ALL indexed drives — pass path to narrow. Supports regex, match_case, match_whole_word, exclude_path, and include_all. Use this FIRST for broad patterns (e.g. *.tmp, *.json) to gauge total before calling find_files. Always prefer this over recursive shell commands to count files.")]
    async fn count_files(
        &self,
        Parameters(params): Parameters<CountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (count, note) = tokio::task::spawn_blocking(move || {
            match native::count(&params) {
                Ok(r) => Ok(r),
                Err(e) => {
                    error!("native count failed, falling back to Everything: {e}");
                    everything::count(params)
                }
            }
        })
        .await
        .map_err(|e| {
            error!("spawn_blocking failed: {e}");
            ErrorData::internal_error(format!("task join failed: {e}"), None)
        })?
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = serde_json::json!({ "total": count, "note": note }).to_string();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "AGGREGATE stats over files matching a query using the native indexer (exceed capability, native-only). Returns: total (matched entries), files, folders, total_size (sum over matches), largest (the top-N biggest entries by size), and by_extension (per-extension count + size breakdown). Pass query to filter (same syntax as find_files), path to scope to a directory, exclude_path/include_all to control exclusions, top to set how many largest entries to return. This capability has no Everything equivalent; if the native indexer is down it returns an explanatory error instead of falling back.")]
    async fn aggregate_files(
        &self,
        Parameters(params): Parameters<AggregateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = tokio::task::spawn_blocking(move || native::aggregate(&params))
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

    #[tool(description = "RECENT file-system changes from the NTFS USN Change Journal via the native indexer (exceed capability, native-only). Returns the most recent change events: created/modified/renamed/deleted files with reason, a local timestamp, and path. Pass since (FILETIME, 100ns since 1601) to only return events newer than that, and limit to cap how many events come back. The ring buffer retains the most recent 100,000 events since the indexer started. This capability has no Everything equivalent; if the native indexer is down it returns an explanatory error instead of falling back.")]
    async fn recent_changes(
        &self,
        Parameters(params): Parameters<RecentChangesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = tokio::task::spawn_blocking(move || native::recent_changes(&params))
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

    #[tool(description = "Check search engine status. Returns diagnostics for both engines: the native indexer (indexed file count, volumes, pipe availability) and the Everything engine (window/IPC/db state, engine source, bundled/installed availability). Call this BEFORE using find_files or count_files to verify an engine is available.")]
    async fn search_status(&self) -> Result<CallToolResult, ErrorData> {
        let (native_result, everything_result) = tokio::task::spawn_blocking(|| {
            (native::status(), everything::status())
        })
        .await
        .map_err(|e| {
            error!("spawn_blocking failed: {e}");
            ErrorData::internal_error(format!("task join failed: {e}"), None)
        })?;

        let native_status = match native_result {
            Ok(v) => serde_json::json!({ "ok": true, "detail": v }),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        };
        let everything_status = match everything_result {
            Ok(v) => serde_json::json!({ "ok": true, "detail": v }),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        };

        let json = serde_json::json!({
            "native": native_status,
            "everything": everything_status,
        });
        let text = serde_json::to_string_pretty(&json)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for EverythingHandler {}
