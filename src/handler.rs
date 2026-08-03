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
    #[tool(description = "Find files instantly across ALL indexed drives. USE THIS instead of glob, grep, or Get-ChildItem. Query examples: '*.rs', 'content:handler', 'dm:lastweek size:>1mb', 'attrib:h'. Supports wildcards, regex, content search, date/size filters, path scoping, and sorting. For broad patterns, call count_files first to gauge size. NOTE: content: search uses a bounded 256MB store and may not cover all files -- use it for targeted searches, not exhaustive scans.")]
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

    #[tool(description = "Count files matching a query instantly across ALL indexed drives. USE THIS instead of Get-ChildItem -Recurse | Measure-Object or shell wc/find commands. Examples: '*.json path:C:/Users', '*.log'. Returns just the count, no file data. For broad patterns, always use this before find_files. NOTE: use forward slashes in paths (C:/Users) to avoid JSON escaping issues.")]
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

    #[tool(description = "Get disk usage stats for files matching a query: total count, total size, per-extension breakdown, and largest files. USE THIS instead of manually summing file sizes. Examples: '*.log', '*.tmp path:C:\\Windows'. Returns aggregate stats, not individual files.")]
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

    #[tool(description = "List recently created, modified, renamed, or deleted files across all indexed drives. USE THIS to answer 'what files changed recently?' or 'what was modified in the last hour?'. Returns change events with timestamps, paths, and reason (created/modified/renamed/deleted). SIMPLEST USAGE: pass hours=1 to get the last hour, hours=24 for the last day — the server computes the FILETIME for you. To skip delete noise (NTFS deleted-file staging area), pass reasons=created,modified. Also accepts 'since' (Windows FILETIME, 100ns since 1601, NOT .NET ticks) and 'limit' (0 = all, capped at 100k; use limit=50 for reasonable output).")]
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

    #[tool(description = "Check if the file search service is running. Returns engine status, indexed file count, and volumes. Call this before find_files/count_files if you suspect the service is down.")]
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
