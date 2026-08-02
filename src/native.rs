//! Named-pipe client for the native indexer engine.
//!
//! Talks to the indexer's `\\.\pipe\instant-file-search-indexer` pipe using
//! the same JSON protocol as the MCP tools: search/count/status. Responses
//! are newline-terminated JSON, so a byte-mode client reads until `\n`.
//!
//! This module is the native-first path: when the indexer pipe is reachable
//! the MCP server answers searches from the in-memory index (fast, private,
//! no Everything dependency). When it is not, everything.rs takes over.

use crate::everything::{ns100_to_iso_string, SearchResult, SearchResults};
use crate::tools::{AggregateParams, CountParams, RecentChangesParams, SearchParams};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const PIPE_NAME: &str = r"\\.\pipe\instant-file-search-indexer";
const CONNECT_TIMEOUT_MS: u32 = 2_000;

#[derive(Debug, Serialize)]
struct PipeRequest<'a> {
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct PipeResponse {
    ok: bool,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Non-blocking reachability probe: one CreateFileW attempt, no waiting.
#[allow(dead_code)]
pub fn available() -> bool {
    connect_inner(0).is_ok()
}

/// Connect, waiting up to `timeout_ms` for the server (cold start).
fn connect_inner(timeout_ms: u32) -> std::io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::{ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);

    loop {
        let handle = unsafe {
            CreateFileW(
                windows::core::PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            )
        };
        match handle {
            Ok(h) => {
                return Ok(unsafe { std::fs::File::from_raw_handle(h.0 as *mut std::ffi::c_void) });
            }
            Err(e) if e.code() == ERROR_PIPE_BUSY.into() => {
                if std::time::Instant::now() > deadline {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "pipe busy"));
                }
                let _ = unsafe { WaitNamedPipeW(windows::core::PCWSTR(name.as_ptr()), 200) };
            }
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no indexer server",
                ));
            }
        }
    }
}

fn exchange(method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value> {
    use std::io::{Read, Write};

    let mut file = connect_inner(CONNECT_TIMEOUT_MS)
        .map_err(|e| anyhow!("native engine unavailable: {e}"))?;
    let req = PipeRequest { method, params };
    let mut body = serde_json::to_vec(&req).map_err(|e| anyhow!(e))?;
    body.push(b'\n');

    file.write_all(&body).map_err(|e| anyhow!("write failed: {e}"))?;
    file.flush().ok();

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        let n = file.read(&mut chunk).map_err(|e| anyhow!("read failed: {e}"))?;
        if n == 0 {
            return Err(anyhow!("connection closed by server"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.ends_with(b"\n") {
            break;
        }
    }

    let resp: PipeResponse = serde_json::from_slice(&buf).map_err(|e| anyhow!("bad response: {e}"))?;
    if !resp.ok {
        return Err(anyhow!(resp.error.unwrap_or_else(|| "native engine error".to_string())));
    }
    resp.data.ok_or_else(|| anyhow!("native engine: empty response"))
}

/// Native engine ping; used for availability probing.
#[allow(dead_code)]
pub fn ping() -> Result<serde_json::Value> {
    exchange("ping", None)
}

/// Native indexer status (indexed count, volumes, engine state).
pub fn status() -> Result<serde_json::Value> {
    exchange("status", None)
}

fn options_from_search(params: &SearchParams) -> serde_json::Value {
    // max_results=0 means "no limit" to the indexer, which can produce a
    // response too large for the pipe buffer. Treat 0 as the default 100.
    let max_results = match params.max_results {
        Some(0) | None => 100,
        Some(n) => n as usize,
    };
    json!({
        "query": params.query,
        "path": params.path,
        "exclude_path": params.exclude_path,
        "include_all": params.include_all.unwrap_or(false),
        "regex": params.regex.unwrap_or(false),
        "match_case": params.match_case.unwrap_or(false),
        "match_whole_word": params.match_whole_word.unwrap_or(false),
        "match_path": params.match_path.unwrap_or(false),
        "max_results": max_results,
        "offset": params.offset.unwrap_or(0) as usize,
        "sort": params.sort,
    })
}

fn options_from_count(params: &CountParams) -> serde_json::Value {
    json!({
        "query": params.query,
        "path": params.path,
        "exclude_path": params.exclude_path,
        "include_all": params.include_all.unwrap_or(false),
        "regex": params.regex.unwrap_or(false),
        "match_case": params.match_case.unwrap_or(false),
        "match_whole_word": params.match_whole_word.unwrap_or(false),
        "max_results": 0,
        "offset": 0,
        "sort": None::<String>,
    })
}

/// Native search; converts the indexer's entry JSON to SearchResults.
pub fn search(params: &SearchParams) -> Result<SearchResults> {
    let data = exchange("search", Some(options_from_search(params)))?;
    let total = data
        .get("total")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("native search: missing total"))?;
    let note = data
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let mut results = Vec::new();
    if let Some(entries) = data.get("entries").and_then(|v| v.as_array()) {
        for e in entries {
            let full_path = e
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let (filename, path) = split_path(&full_path);
            let size = e.get("size").and_then(|v| v.as_u64());
            let ns = |key: &str| {
                e.get(key)
                    .and_then(|v| v.as_i64())
                    .and_then(|v| ns100_to_iso_string(v as u64))
            };
            let attributes = match e.get("is_dir").and_then(|v| v.as_bool()) {
                Some(true) => Some(crate::everything::format_attributes(0x10)),
                _ => None,
            };
            results.push(SearchResult {
                filename,
                path,
                size,
                date_modified: ns("modified"),
                date_created: ns("created"),
                date_accessed: ns("accessed"),
                attributes,
                extension: e
                    .get("extension")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                run_count: None,
                date_run: None,
            });
        }
    }

    let returned = results.len();
    let offset = params.offset.unwrap_or(0);
    Ok(SearchResults {
        results,
        total,
        returned,
        offset,
        note,
    })
}

/// Native count; returns (total, exclusion_note).
pub fn count(params: &CountParams) -> Result<(u64, String)> {
    let data = exchange("count", Some(options_from_count(params)))?;
    let total = data
        .get("total")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("native count: missing total"))?;
    let note = data
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((total, note))
}

/// Split a full path into (filename, parent_dir).
fn split_path(full: &str) -> (String, Option<String>) {
    match full.rfind('\\') {
        Some(i) => (full[i + 1..].to_string(), Some(full[..i].to_string())),
        None => (full.to_string(), None),
    }
}

/// Aggregation result mirrored from the indexer's `aggregate` response.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateResult {
    /// Number of matched entries (files + folders).
    pub total: u64,
    /// Matched file count.
    pub files: u64,
    /// Matched folder count.
    pub folders: u64,
    /// Sum of sizes over all matched entries.
    pub total_size: u64,
    /// The `top` largest matched entries by size.
    pub largest: Vec<AggregateLargest>,
    /// Per-extension counts and size totals over matched files.
    pub by_extension: Vec<AggregateExt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateLargest {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AggregateExt {
    pub extension: String,
    pub count: u64,
    pub size: u64,
}

fn options_from_aggregate(params: &AggregateParams) -> serde_json::Value {
    json!({
        "query": params.query,
        "path": params.path,
        "exclude_path": params.exclude_path,
        "include_all": params.include_all.unwrap_or(false),
        "regex": params.regex.unwrap_or(false),
        "match_case": params.match_case.unwrap_or(false),
        "match_whole_word": params.match_whole_word.unwrap_or(false),
        "top": params.top.unwrap_or(20) as usize,
    })
}

/// Native aggregation. This is an exceed capability with no Everything
/// equivalent; if the indexer pipe is unavailable the caller surfaces an
/// explanatory error rather than falling back.
pub fn aggregate(params: &AggregateParams) -> Result<AggregateResult> {
    let data = exchange("aggregate", Some(options_from_aggregate(params)))?;
    Ok(serde_json::from_value(data).map_err(|e| anyhow!("native aggregate: bad response: {e}"))?)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ChangeEvent {
    pub timestamp: i64,
    pub reason: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RecentChanges {
    pub changes: Vec<ChangeEvent>,
}

fn options_from_recent(since: i64, limit: usize) -> serde_json::Value {
    json!({ "since": since, "limit": limit })
}

/// Recent USN change events from the native indexer. Exceed capability with
/// no Everything equivalent; if the indexer pipe is unavailable the caller
/// surfaces an explanatory error rather than falling back.
pub fn recent_changes(params: &RecentChangesParams) -> Result<RecentChanges> {
    let since = params.since.unwrap_or(0);
    let limit = params.limit.unwrap_or(0);
    let data = exchange("recent_changes", Some(options_from_recent(since, limit)))?;
    Ok(serde_json::from_value(data).map_err(|e| anyhow!("native recent_changes: bad response: {e}"))?)
}
