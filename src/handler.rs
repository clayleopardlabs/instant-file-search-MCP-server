use rmcp::{
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerInfo,
    },
    ServerHandler, tool, tool_handler, tool_router,
};
use tracing::error;

#[cfg(windows)]
use crate::everything;
use crate::native;
use crate::tools::{
    AggregateParams, CountParams, RecentChangesParams, SearchParams,
};

/// Probe the Everything fallback engine for `search_status`. Windows-only;
/// on Linux the native indexer is the only engine.
#[cfg(windows)]
fn everything_status_probe() -> anyhow::Result<serde_json::Value> {
    everything::status()
        .map(|s| serde_json::to_value(&s).unwrap_or(serde_json::Value::Null))
}

#[cfg(not(windows))]
fn everything_status_probe() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "available": false,
        "engine": "native-only",
        "reason": "The Everything fallback engine is Windows-only"
    }))
}

#[derive(Clone, Default)]
pub struct EverythingHandler;

/// Reject filter tokens that can only ever match nothing.
///
/// Everything's semantics are "an invalid filter matches nothing", which for an
/// agent-driven server turns a typo into a confident "that file does not exist".
fn reject_invalid_filters(query: &str) -> Result<(), ErrorData> {
    let bad = crate::diagnostics::invalid_filters(query);
    if bad.is_empty() {
        return Ok(());
    }
    Err(ErrorData::invalid_params(
        format!(
            "invalid filter token(s): {}. An invalid filter matches nothing, so this \
             would return a misleading empty result. Valid forms: size:>10mb, \
             size:large, ext:rs,py, attrib:h.",
            bad.join(", ")
        ),
        None,
    ))
}

/// Echo what the server understood, and warn about likely mistakes.
fn annotate(
    mut value: serde_json::Value,
    query: &str,
    include_all: bool,
) -> serde_json::Value {
    value["parsed"] = serde_json::to_value(crate::diagnostics::parsed(query, include_all))
        .unwrap_or(serde_json::Value::Null);
    let warnings = crate::diagnostics::analyze(query, include_all);
    if !warnings.is_empty() {
        if let Ok(w) = serde_json::to_value(&warnings) {
            value["warnings"] = w;
        }
    }
    value
}

#[tool_router]
impl EverythingHandler {
    #[tool(description = "Find files instantly across ALL indexed drives. USE THIS instead of glob, grep, or Get-ChildItem. Query examples: '*.rs', 'content:handler', 'dm:lastweek size:>1mb', 'attrib:h'. Supports wildcards, regex, content search, date/size filters, path scoping, and sorting. For broad patterns, call count_files first to gauge size. Results are files only; add 'is:folder' to include directories, which carry aggregate sizes and are marked by is_dir. NOTE: content: search is bounded. Memory mode uses up to 256 MB of RAM; disk mode can persist content in SQLite. Use it for targeted searches, not exhaustive scans. Default excludes node_modules, .git, WinSxS, $Recycle.Bin, System Volume Information. Pass include_all=true to search everything. OR is '|' (not 'OR'). Scope with the `path` param, not path: inside the query. Multi-extension: ext:lsv,d2s.")]
    async fn find_files(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Files-by-default is applied per engine, never by rewriting the query
        // string: the native indexer takes a `files_only` option (native.rs)
        // and the Everything fallback appends a `file:` token (everything.rs).
        // Mutating the query here broke whole-query regex mode and nested regex
        // tokens inside an AND-group where they failed to compile.
        reject_invalid_filters(&params.query)?;
        let query = params.query.clone();
        let include_all = params.include_all.unwrap_or(false);
        let result = tokio::task::spawn_blocking(move || {
            match native::search(&params) {
                Ok(r) => Ok(r),
                Err(e) => {
                    error!("native search failed, falling back to Everything: {e}");
                    #[cfg(windows)]
                    {
                        everything::search(params)
                    }
                    #[cfg(not(windows))]
                    {
                        Err(e)
                    }
                }
            }
        })
        .await
        .map_err(|e| {
            error!("spawn_blocking failed: {e}");
            ErrorData::internal_error(format!("task join failed: {e}"), None)
        })?
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let value = serde_json::to_value(&result)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let value = annotate(value, &query, include_all);
        let text = serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Count files matching a query instantly across ALL indexed drives. USE THIS instead of Get-ChildItem -Recurse | Measure-Object or shell wc/find commands. Examples: '*.json path:C:/Users', '*.log'. Returns just the count, no file data. For broad patterns, always use this before find_files. Counts files only, matching find_files; add 'is:folder' to count directories. NOTE: use forward slashes in paths (C:/Users) to avoid JSON escaping issues. Default excludes node_modules, .git, WinSxS, $Recycle.Bin, System Volume Information. Pass include_all=true for a true count. OR is '|' (not 'OR'); scope with the `path` param.")]
    async fn count_files(
        &self,
        Parameters(params): Parameters<CountParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Same files-by-default contract as find_files, so "count first, then
        // list" agrees; applied per engine rather than by rewriting the query.
        reject_invalid_filters(&params.query)?;
        let query = params.query.clone();
        let include_all = params.include_all.unwrap_or(false);
        let (count, note) = tokio::task::spawn_blocking(move || {
            match native::count(&params) {
                Ok(r) => Ok(r),
                Err(e) => {
                    error!("native count failed, falling back to Everything: {e}");
                    #[cfg(windows)]
                    {
                        everything::count(params)
                    }
                    #[cfg(not(windows))]
                    {
                        Err(e)
                    }
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
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let value = annotate(value, &query, include_all);
        let text = serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Disk usage stats: total count, total_file_size (the sum of matched file sizes), per-extension breakdown, and largest files. Files-only by default like the other tools, so a bare \"*\" is safe; pass is:folder for directory rows. Pass include_all=true for a true disk total (default excludes node_modules, .git, WinSxS, $Recycle.Bin, System Volume Information). Examples: 'file:', '*.iso file: size:>1gb'. Returns aggregate stats, not individual files. USE THIS instead of manually summing file sizes.")]
    async fn aggregate_files(
        &self,
        Parameters(params): Parameters<AggregateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reject_invalid_filters(&params.query)?;
        let query = params.query.clone();
        let include_all = params.include_all.unwrap_or(false);
        // Asking for folders is the only case where folder rows in an aggregate
        // are intended, and the only case where a subtree-sized total is right.
        let result = tokio::task::spawn_blocking(move || native::aggregate(&params))
            .await
            .map_err(|e| {
                error!("spawn_blocking failed: {e}");
                ErrorData::internal_error(format!("task join failed: {e}"), None)
            })?
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let value = serde_json::to_value(&result)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let value = annotate(value, &query, include_all);
        let text = serde_json::to_string_pretty(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "List recently created, modified, renamed, or deleted files across all indexed drives. USE THIS to answer 'what files changed recently?' or 'what was modified in the last hour?'. Returns change events with timestamps, paths, and reason (created/modified/renamed/deleted), newest-first. SIMPLEST USAGE: pass hours=1 to get the last hour, hours=24 for the last day; the server computes the FILETIME for you. Results are capped at 100 by default (newest first); pass a larger limit or reasons=created,modified to skip delete noise. Also accepts 'since' (Windows FILETIME, 100ns since 1601, NOT .NET ticks).")]
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

    #[tool(description = "Check the file search service health: engine status, indexed file count, volumes. Call this before find_files/count_files if you suspect the service is down.")]
    async fn search_status(&self) -> Result<CallToolResult, ErrorData> {
        let (native_result, everything_result) = tokio::task::spawn_blocking(|| {
            (native::status(), everything_status_probe())
        })
        .await
        .map_err(|e| {
            error!("spawn_blocking failed: {e}");
            ErrorData::internal_error(format!("task join failed: {e}"), None)
        })?;

        // A client and indexer built from different revisions is a real source of
        // confusing behaviour: a bug that was fixed still present on the wire, or
        // a new option silently ignored because the indexer predates it. Surface
        // it instead of leaving the reader to diff two version strings.
        let mut warnings: Vec<serde_json::Value> = Vec::new();
        if let Ok(detail) = &native_result {
            let iv = detail.get("version").and_then(|v| v.as_str()).unwrap_or("");
            let ic = detail.get("commit").and_then(|v| v.as_str()).unwrap_or("");
            if !iv.is_empty() && iv != crate::APP_VERSION {
                warnings.push(serde_json::json!({
                    "code": "BUILD_MISMATCH",
                    "message": format!(
                        "The MCP server is {} but the running indexer is {}. The indexer may not contain fixes the client relies on. Redeploy both with scripts/install.ps1.",
                        crate::APP_VERSION, iv
                    ),
                }));
            } else if !ic.is_empty() && ic != crate::BUILD_COMMIT {
                warnings.push(serde_json::json!({
                    "code": "COMMIT_MISMATCH",
                    "message": format!(
                        "Server commit {} differs from indexer commit {}. Redeploy both from the same build.",
                        crate::BUILD_COMMIT, ic
                    ),
                }));
            }
        }

        let native_status = match native_result {
            Ok(v) => serde_json::json!({ "ok": true, "detail": v }),
            // A missing pipe usually means the service is still building its
            // index after a start or upgrade. Say so, instead of a bare
            // "no indexer server" that reads like a broken install.
            Err(e) => serde_json::json!({
                "ok": false,
                "error": e.to_string(),
                "hint": "The native indexer is not accepting connections. After a start, upgrade or crash it may still be scanning; retry in a few seconds. If it persists, run: sc query instant-file-search-indexer",
            }),
        };
        let everything_status = match everything_result {
            Ok(v) => serde_json::json!({ "ok": true, "detail": v }),
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        };

        let json = serde_json::json!({
            "server": { "version": crate::APP_VERSION, "commit": crate::BUILD_COMMIT },
            "native": native_status,
            "everything": everything_status,
            "warnings": warnings,
        });

        // NOTE: index-versus-filesystem verification (`verify_sample`) is NOT
        // wired here yet. It is implemented and unit tested in `src/verify.rs`
        // and `sample_index` below, but calling it from this handler blocked
        // indefinitely against the live index in testing, and a tool call that
        // never returns is worse than the missing check. The module is kept so
        // the follow-up starts from tested code rather than a blank page.
        // Do not re-enable without reproducing the block first.

        let text = serde_json::to_string_pretty(&json)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

/// Stat a sample of indexed paths and compare them with the index.
///
/// Uses the ordinary search path so the sample is exactly what a caller would
/// have been given, then checks each one against the filesystem.
///
/// Currently unused: see the note in `search_status`. Kept (with its tests) as
/// the starting point for the verification feature.
#[allow(dead_code)]
fn sample_index(n: usize, path: Option<&str>) -> anyhow::Result<crate::verify::VerifyReport> {
    let params = SearchParams {
        query: "file:".to_string(),
        max_results: Some(n as u32),
        offset: None,
        regex: None,
        match_case: None,
        match_whole_word: None,
        match_path: None,
        sort: None,
        path: path.map(|p| p.to_string()),
        fields: None,
        exclude_path: None,
        include_all: Some(true),
    };
    let results = native::search(&params)?;
    // `SearchResult.path` is the PARENT directory; the file name is separate.
    // Joining them is what makes the stat below compare the right object.
    let entries: Vec<(String, u64)> = results
        .results
        .iter()
        .filter_map(|r| {
            let size = r.size?;
            let full = match r.path.as_deref() {
                Some(dir) if !dir.is_empty() => std::path::Path::new(dir)
                    .join(&r.filename)
                    .to_string_lossy()
                    .into_owned(),
                _ => r.filename.clone(),
            };
            Some((full, size))
        })
        .collect();
    Ok(crate::verify::verify_entries(&entries, |p| {
        std::fs::metadata(p).ok().map(|m| m.len())
    }))
}

/// Names of the tools this server advertises, sorted. `--self-test` prints them
/// so a support report shows what the model actually receives.
pub fn tool_names() -> Vec<String> {
    let mut names: Vec<String> = EverythingHandler::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    names
}

#[tool_handler]
impl ServerHandler for EverythingHandler {
    fn get_info(&self) -> ServerInfo {
        // Advertise the `tools` capability. Without it, strictly conforming
        // clients skip `tools/list` entirely and the server appears to expose
        // no tools at all: the connection succeeds and the failure is silent.
        // Observed with the DeepSeek Harness MCP bridge (which mirrors the
        // official SDK check `capabilities.tools === undefined`); Codex,
        // OpenCode and Hermes call `tools/list` without checking, so they were
        // unaffected. Guarded by `initialize_advertises_tools_capability`.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("instant-file-search", crate::APP_VERSION)
                    .with_title("Instant File Search"),
            )
            // The rules an agent needs are shipped in the handshake rather than
            // left in docs/ that nobody is asked to read mid-task.
            .with_instructions(crate::tools::AGENT_INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::EverythingHandler;
    use rmcp::ServerHandler;

    /// Strict MCP clients (DeepSeek Harness, and anything else that follows the
    /// SDK's capability check) never call `tools/list` unless `initialize`
    /// advertises the `tools` capability: they end up with a connected server
    /// that exposes zero tools and no error anywhere. Keep this advertised.
    #[test]
    fn initialize_advertises_tools_capability() {
        let info = EverythingHandler.get_info();
        assert!(
            info.capabilities.tools.is_some(),
            "initialize must advertise capabilities.tools"
        );
    }

    /// The capability is only useful if the router really serves the five
    /// documented tools; a rename or a dropped `#[tool]` should fail loudly.
    #[test]
    fn router_registers_the_documented_tools() {
        let mut names: Vec<String> = EverythingHandler::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "aggregate_files",
                "count_files",
                "find_files",
                "recent_changes",
                "search_status",
            ]
        );
    }
}
