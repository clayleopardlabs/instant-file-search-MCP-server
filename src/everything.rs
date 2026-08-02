// Everything IPC client wrapper using the `everything-ipc` crate's WM_COPYDATA
// communication with the Everything desktop search GUI.
//
// All functions are synchronous and blocking. The caller is responsible for
// running them on a blocking thread (e.g. via tokio::task::spawn_blocking).

use anyhow::Result;
use everything_ipc::wm::{EverythingClient, RequestFlags, SearchFlags, Sort};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Input types are defined in tools.rs (shared with rmcp tool definitions).
use crate::tools::{CountParams, SearchParams};

// ---- Public data types (output only) ---------------------------------------

/// A single search result item.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchResult {
    /// File name (always present).
    pub filename: String,

    /// Full path (excluding filename).
    pub path: Option<String>,

    /// File size in bytes.
    pub size: Option<u64>,

    /// Last modified date (ISO 8601 UTC).
    pub date_modified: Option<String>,

    /// Creation date (ISO 8601 UTC).
    pub date_created: Option<String>,

    /// Last accessed date (ISO 8601 UTC).
    pub date_accessed: Option<String>,

    /// File attributes string.
    pub attributes: Option<String>,

    /// File extension.
    pub extension: Option<String>,

    /// Number of times this file has been run (launched).
    pub run_count: Option<u32>,

    /// Last run date (ISO 8601 UTC).
    pub date_run: Option<String>,
}

/// Collection of search results.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchResults {
    /// The result items in this page.
    pub results: Vec<SearchResult>,

    /// Total number of matching items (may be larger than results.len()).
    pub total: u64,

    /// Number of results returned in this page (equals results.len()).
    pub returned: usize,

    /// The offset value used for this page (0 for the first page).
    pub offset: u32,

    /// Information about any automatic exclusions applied to the query.
    pub note: String,
}

/// Overall status of the Everything IPC connection.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EverythingStatus {
    /// Whether Everything is connected and fully usable (IPC available + DB loaded).
    pub connected: bool,

    /// Whether the Everything GUI window was found.
    pub window_found: bool,

    /// Whether the IPC message channel is functional (SendMessageW responds).
    pub ipc_available: bool,

    /// Whether Everything's database is fully loaded (indexing complete).
    pub db_loaded: bool,

    /// Everything version string, if available.
    pub version: Option<String>,

    /// Where the search engine came from.
    pub engine_source: EngineSource,

    /// Whether a bundled portable Everything ships with this MCP install.
    pub bundled_available: bool,

    /// Whether an installed Everything was detected on this machine.
    pub installed_available: bool,
}

/// How the Everything search engine was made available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineSource {
    /// A reachable Everything window already existed (user's own install).
    Existing,
    /// The MCP launched the installed Everything GUI (e.g. to bridge a service-only install).
    InstalledLaunched,
    /// The MCP launched its bundled portable Everything.
    Bundled,
    /// No engine could be made available.
    None,
}

impl Default for EngineSource {
    fn default() -> Self {
        EngineSource::None
    }
}

// ---- Engine management (self-contained bundle) -----------------------------

/// How long to wait for a freshly launched engine to load its database.
fn engine_timeout() -> Duration {
    std::env::var("EVERYTHING_ENGINE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60))
}

/// The Everything service only serves the DEFAULT instance, so both the
/// user's install and the bundled portable engine connect via
/// [`EverythingClient::new()`].
enum EngineKind {
    Default,
}

impl EngineKind {
    fn connect(&self) -> Result<EverythingClient> {
        EverythingClient::new().map_err(|e| {
            anyhow::anyhow!("Everything IPC unavailable for default instance: {e}")
        })
    }
}

impl std::fmt::Debug for EngineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "default")
    }
}

/// Path to the bundled portable Everything.exe, if it ships with this install.
///
/// Resolution order:
/// 1. `EVERYTHING_ENGINE_EXE` environment variable (explicit override)
/// 2. `..\everything\Everything.exe` relative to this binary
/// 3. `everything\Everything.exe` next to this binary
pub fn bundled_everything_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("EVERYTHING_ENGINE_EXE") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }

    let own_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates = [
        own_dir.join("..").join("everything").join("Everything.exe"),
        own_dir.join("everything").join("Everything.exe"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

/// Locate an installed Everything.exe (the user's own install).
fn installed_everything_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // Registry App Paths (written by the Everything installer).
    for hive in [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Everything.exe",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Everything.exe",
    ] {
        let out = Command::new("reg")
            .args(["query", hive, "/ve"])
            .output()
            .ok();
        if let Some(out) = out {
            if let Ok(text) = String::from_utf8(out.stdout) {
                // Line format: "    (Default)    REG_SZ    C:\Program Files\Everything\Everything.exe"
                for line in text.lines() {
                    if let Some(idx) = line.find("REG_SZ") {
                        let p = PathBuf::from(line[idx + "REG_SZ".len()..].trim());
                        if p.is_file() {
                            candidates.push(p);
                        }
                    }
                }
            }
        }
    }

    // Common install locations.
    for base in [
        std::env::var("ProgramFiles").unwrap_or_default(),
        std::env::var("ProgramFiles(x86)").unwrap_or_default(),
        std::env::var("LOCALAPPDATA").unwrap_or_default(),
    ] {
        if !base.is_empty() {
            candidates.push(PathBuf::from(base).join("Everything").join("Everything.exe"));
        }
    }

    candidates.into_iter().find(|p| p.is_file())
}

/// Launch an Everything engine. `bundled` selects the bundled portable exe
/// (with a private config), otherwise the installed GUI.
fn launch_engine(path: &Path, bundled: bool) -> anyhow::Result<()> {
    let mut cmd = Command::new(path);
    if bundled {
        let ini = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("instant-file-search-fallback-engine-1.5.0.1418b.ini");
        // Run as the DEFAULT instance (no -instance flag): the Everything
        // service only serves the default instance, so the bundled GUI can
        // connect to a running service for indexing.
        cmd.args([
            "-config",
            ini.to_string_lossy().as_ref(),
            "-startup",
            "-first-instance",
        ]);
    } else {
        // The installed GUI connects to a running Everything service if present.
        cmd.arg("-startup");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    cmd.spawn().map_err(|e| {
        anyhow::anyhow!("failed to launch Everything at {}: {e}", path.display())
    })?;
    tracing::info!("launched Everything engine: {}", path.display());
    Ok(())
}

/// Wait until an engine of the given kind answers and its DB is loaded.
fn wait_for_engine(kind: &EngineKind, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(client) = kind.connect() {
            if client.is_ipc_available() && client.is_db_loaded() {
                return true;
            }
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Make sure a search engine is available, launching one if necessary.
///
/// Priority:
/// 1. A reachable, loaded Everything window already exists (user's install) — use it.
/// 2. An installed Everything exists but is not reachable (e.g. service-only
///    install) — launch its GUI, which bridges the running service.
/// 3. No installed Everything — launch the bundled portable Everything.
pub fn ensure_engine() -> Result<EngineSource> {
    // 1. Existing, fully loaded engine.
    if let Ok(client) = EverythingClient::new() {
        if client.is_ipc_available() && client.is_db_loaded() {
            return Ok(EngineSource::Existing);
        }
    }

    let installed = installed_everything_path();

    // 2. Launch the installed Everything GUI (bridges service-only installs).
    if let Some(i) = &installed {
        tracing::info!("launching installed Everything at {}", i.display());
        if launch_engine(i, false).is_ok()
            && wait_for_engine(&EngineKind::Default, engine_timeout())
        {
            return Ok(EngineSource::InstalledLaunched);
        }
    }

    // 3. Launch the bundled portable Everything (default instance, connects
    //    to the Everything service for indexing when one is present).
    if let Some(b) = bundled_everything_path() {
        tracing::info!("launching bundled Everything at {}", b.display());
        if launch_engine(&b, true).is_ok()
            && wait_for_engine(&EngineKind::Default, engine_timeout())
        {
            return Ok(EngineSource::Bundled);
        }
    }

    let mut msg = String::from("no Everything engine available: no reachable Everything window found");
    if installed.is_none() {
        msg.push_str(", no installed Everything, ");
    }
    if bundled_everything_path().is_none() {
        msg.push_str("and no bundled portable Everything");
    } else {
        msg.push_str("and launching available engines failed or timed out");
    }
    msg.push_str(". Run `search_status` for diagnostics.");
    Err(anyhow::anyhow!(msg))
}

/// Create an IPC client for the engine source currently in effect.
fn create_client_for(source: &EngineSource) -> Result<EverythingClient> {
    match source {
        EngineSource::Existing | EngineSource::InstalledLaunched | EngineSource::Bundled => {
            EverythingClient::new().map_err(|e| anyhow::anyhow!("Everything IPC failed: {e}"))
        }
        EngineSource::None => Err(anyhow::anyhow!("no Everything engine available")),
    }
}

// ---- Public API ------------------------------------------------------------

/// Search Everything with the given parameters.
pub fn search(params: SearchParams) -> Result<SearchResults> {
    let client = create_client()?;

    let (search_text, note) = build_search_query(
        &params.query,
        params.path.as_deref(),
        params.exclude_path.as_deref(),
        params.include_all.unwrap_or(false),
    );

    // Gather all optional parameters first (avoids type-state reassignment)
    let flags = parse_fields(params.fields.as_deref());
    let mut search_flags = SearchFlags::empty();
    if params.regex.unwrap_or(false) {
        search_flags |= SearchFlags::Regex;
    }
    if params.match_case.unwrap_or(false) {
        search_flags |= SearchFlags::MatchCase;
    }
    if params.match_whole_word.unwrap_or(false) {
        search_flags |= SearchFlags::MatchWholeWord;
    }
    if params.match_path.unwrap_or(false) {
        search_flags |= SearchFlags::MatchPath;
    }
    let sort = params
        .sort
        .as_deref()
        .and_then(parse_sort)
        .unwrap_or(Sort::NameAscending);

    let list = client
        .query_wait(&search_text)
        .request_flags(flags)
        .search_flags(search_flags)
        .sort(sort)
        .maybe_max_results(params.max_results)
        .maybe_offset(params.offset)
        .call()
        .map_err(|e| anyhow::anyhow!("Everything query failed: {e}"))?;

    // Collect results
    let results: Vec<SearchResult> = list
        .iter()
        .map(|item| {
            let filename = item
                .get_string(RequestFlags::FileName)
                .unwrap_or_default();
            let path = item.get_string(RequestFlags::Path);
            let size = item
                .get_size(RequestFlags::Size)
                .filter(|&v| v != u64::MAX);

            // FILETIME is a transitive dep type (windows-rs) that we cannot
            // name in source.  `.map(|ft| ...)`` extracts the 100-ns interval
            // count via inferred field access on the concrete FILETIME type.
            let date_modified = item
                .get_time(RequestFlags::DateModified)
                .map(|ft| ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64))
                .and_then(ns100_to_iso_string);
            let date_created = item
                .get_time(RequestFlags::DateCreated)
                .map(|ft| ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64))
                .and_then(ns100_to_iso_string);
            let date_accessed = item
                .get_time(RequestFlags::DateAccessed)
                .map(|ft| ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64))
                .and_then(ns100_to_iso_string);

            let attributes = item
                .get_u32(RequestFlags::Attributes)
                .map(format_attributes);
            let extension = item.get_string(RequestFlags::Extension);
            let run_count = item.get_u32(RequestFlags::RunCount);
            let date_run = item
                .get_time(RequestFlags::DateRun)
                .map(|ft| ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64))
                .and_then(ns100_to_iso_string);

            SearchResult {
                filename,
                path,
                size,
                date_modified,
                date_created,
                date_accessed,
                attributes,
                extension,
                run_count,
                date_run,
            }
        })
        .collect();

    let total = list.total_len() as u64;
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

/// Count matching files without retrieving result items.
///
/// Returns (total_count, exclusion_note).
pub fn count(params: CountParams) -> Result<(u64, String)> {
    let client = create_client()?;

    let mut search_flags = SearchFlags::empty();
    if params.regex.unwrap_or(false) {
        search_flags |= SearchFlags::Regex;
    }
    if params.match_case.unwrap_or(false) {
        search_flags |= SearchFlags::MatchCase;
    }
    if params.match_whole_word.unwrap_or(false) {
        search_flags |= SearchFlags::MatchWholeWord;
    }

    let (search_text, note) = build_search_query(
        &params.query,
        params.path.as_deref(),
        params.exclude_path.as_deref(),
        params.include_all.unwrap_or(false),
    );

    let list = client
        .query_wait(&search_text)
        .request_flags(RequestFlags::FileName)
        .search_flags(search_flags)
        .max_results(1)
        .call()
        .map_err(|e| anyhow::anyhow!("Everything count query failed: {e}"))?;

    Ok((list.total_len() as u64, note))
}

/// Quick check whether Everything is running, IPC is available, and the DB
/// is loaded.  This is stricter than `EverythingClient::new()` alone — it
/// also verifies the IPC message channel responds and the database is ready.
#[allow(dead_code)]
pub fn is_running() -> bool {
    let Ok(client) = EverythingClient::new() else {
        return false;
    };
    client.is_ipc_available() && client.is_db_loaded()
}

/// Return detailed status of the Everything IPC connection.
///
/// Self-healing: if no engine is available, attempts to launch one (bundled
/// portable Everything first, then the installed GUI) before reporting.
#[allow(dead_code)]
pub fn status() -> Result<EverythingStatus> {
    let bundled_available = bundled_everything_path().is_some();
    let installed_available = installed_everything_path().is_some();

    let source = ensure_engine().unwrap_or(EngineSource::None);
    let client = create_client_for(&source);

    let (ipc_available, db_loaded, window_found) = match &client {
        Ok(c) => (c.is_ipc_available(), c.is_db_loaded(), true),
        Err(_) => (false, false, false),
    };

    Ok(EverythingStatus {
        connected: source != EngineSource::None && ipc_available && db_loaded,
        window_found,
        ipc_available,
        db_loaded,
        version: None,
        engine_source: source,
        bundled_available,
        installed_available,
    })
}

// ---- Internal helpers ------------------------------------------------------

/// Try to create an Everything IPC client, launching an engine if needed.
fn create_client() -> Result<EverythingClient> {
    let source = ensure_engine()?;
    create_client_for(&source)
}

/// Default directory patterns excluded when include_all is false.
/// Must mirror the native indexer's DEFAULT_EXCLUDES (indexer/src/query.rs).
const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules",
    ".git",
    "WinSxS",
    "$Recycle.Bin",
    "System Volume Information",
];

/// Build the Everything search query string with smart defaults.
///
/// Concatenates: search term, optional path scope, default exclusions
/// (unless include_all), and user's explicit exclusions.
/// Returns the query string and a human-readable exclusion note.
fn build_search_query(
    query: &str,
    path: Option<&str>,
    exclude_path: Option<&str>,
    include_all: bool,
) -> (String, String) {
    let mut parts: Vec<String> = Vec::new();
    let mut excluded_dirs: Vec<&str> = Vec::new();

    parts.push(query.to_string());

    if let Some(p) = path.filter(|s| !s.is_empty()) {
        let normalised = Path::new(p)
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_string();
        // A bare path with spaces splits into multiple terms; quote it so
        // Everything treats it as one folder-scope term.
        if normalised.contains(' ') {
            parts.push(format!("\"{}\"", normalised));
        } else {
            parts.push(normalised);
        }
    }

    if !include_all {
        for excl in DEFAULT_EXCLUDES {
            parts.push(format!("!<{}\\>", excl));
            excluded_dirs.push(excl);
        }
    }

    if let Some(ep) = exclude_path.filter(|s| !s.is_empty()) {
        for part in ep.split(';') {
            let trimmed = part.trim().trim_end_matches('\\');
            if !trimmed.is_empty() {
                // `!<foo\>` excludes the folder and everything under it,
                // matching the native indexer's bare-name semantics.
                parts.push(format!("!<{}\\>", trimmed));
                excluded_dirs.push(trimmed);
            }
        }
    }

    let note = if excluded_dirs.is_empty() {
        String::new()
    } else {
        format!("Excluded directories: {}", excluded_dirs.join(", "))
    };

    (parts.join(" "), note)
}

/// Map a sort string to the corresponding `Sort` enum value.
fn parse_sort(s: &str) -> Option<Sort> {
    match s {
        "name" => Some(Sort::NameAscending),
        "name_desc" => Some(Sort::NameDescending),
        "path" => Some(Sort::PathAscending),
        "path_desc" => Some(Sort::PathDescending),
        "size" => Some(Sort::SizeDescending),
        "size_asc" => Some(Sort::SizeAscending),
        "date_modified" => Some(Sort::DateModifiedDescending),
        "date_modified_asc" => Some(Sort::DateModifiedAscending),
        "date_created" => Some(Sort::DateCreatedDescending),
        "date_created_asc" => Some(Sort::DateCreatedAscending),
        "date_accessed" => Some(Sort::DateAccessedDescending),
        "date_accessed_asc" => Some(Sort::DateAccessedAscending),
        "extension" => Some(Sort::ExtensionAscending),
        "extension_desc" => Some(Sort::ExtensionDescending),
        "run_count" => Some(Sort::RunCountDescending),
        "run_count_asc" => Some(Sort::RunCountAscending),
        "date_run" => Some(Sort::DateRunDescending),
        "date_run_asc" => Some(Sort::DateRunAscending),
        "type_name" => Some(Sort::TypeNameAscending),
        "type_name_desc" => Some(Sort::TypeNameDescending),
        "date_recently_changed" => Some(Sort::DateRecentlyChangedDescending),
        "date_recently_changed_asc" => Some(Sort::DateRecentlyChangedAscending),
        _ => None,
    }
}

/// Parse a comma-separated fields string into the corresponding `RequestFlags`
/// bitmask.  Returns all common fields when the input is `None`.
fn parse_fields(fields: Option<&str>) -> RequestFlags {
    let all = RequestFlags::FileName
        | RequestFlags::Path
        | RequestFlags::Size
        | RequestFlags::DateModified
        | RequestFlags::DateCreated
        | RequestFlags::DateAccessed
        | RequestFlags::Attributes
        | RequestFlags::Extension;

    let fields = match fields {
        Some(s) => s,
        None => return all,
    };

    let mut flags = RequestFlags::empty();
    for part in fields.split(',') {
        let part = part.trim();
        match part {
            "filename" => flags |= RequestFlags::FileName,
            "path" => flags |= RequestFlags::Path,
            "size" => flags |= RequestFlags::Size,
            "date_modified" => flags |= RequestFlags::DateModified,
            "date_created" => flags |= RequestFlags::DateCreated,
            "date_accessed" => flags |= RequestFlags::DateAccessed,
            "attributes" => flags |= RequestFlags::Attributes,
            "extension" => flags |= RequestFlags::Extension,
            "run_count" => flags |= RequestFlags::RunCount,
            "date_run" => flags |= RequestFlags::DateRun,
            "date_recently_changed" => flags |= RequestFlags::DateRecentlyChanged,
            "file_list_filename" => flags |= RequestFlags::FileListFileName,
            _ => {}
        }
    }
    // Always include FileName so every result has at least an identifier.
    if flags.is_empty() {
        flags |= RequestFlags::FileName;
    }
    flags
}

/// Convert a 100-nanosecond-interval count (from the FILETIME epoch of
/// 1601-01-01 UTC) to an ISO 8601 UTC time string.
pub(crate) fn ns100_to_iso_string(ns100: u64) -> Option<String> {
    if ns100 == 0 {
        return None;
    }
    // FILETIME epoch (1601-01-01) -> Unix epoch (1970-01-01) offset
    const EPOCH_OFFSET: u64 = 116_444_736_000_000_000;
    if ns100 < EPOCH_OFFSET {
        return None;
    }
    let unix_secs = (ns100 - EPOCH_OFFSET) / 10_000_000;
    Some(unix_timestamp_to_rfc3339(unix_secs))
}

/// Format a Unix timestamp (whole seconds) as an RFC 3339 / ISO 8601 UTC time
/// string (`2024-01-15T10:30:00Z`).
fn unix_timestamp_to_rfc3339(secs: u64) -> String {
    let sec_of_day = secs % 86400;
    let mut days = secs / 86400;

    let hour = sec_of_day / 3600;
    let min = (sec_of_day % 3600) / 60;
    let sec = sec_of_day % 60;

    // Civil date from days since 1970-01-01
    let mut year: i64 = 1970;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: &[u64] = if leap {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month: u64 = 1;
    let mut day = days;
    for &md in month_days {
        if day < md {
            break;
        }
        day -= md;
        month += 1;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day + 1,
        hour,
        min,
        sec
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Format file attributes as a human-friendly string (e.g. "A", "H", "R", "S",
/// "D").
pub(crate) fn format_attributes(attrs: u32) -> String {
    let mut s = String::with_capacity(8);

    const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;
    const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0800;

    if attrs & FILE_ATTRIBUTE_ARCHIVE != 0 {
        s.push('A');
    }
    if attrs & FILE_ATTRIBUTE_READONLY != 0 {
        s.push('R');
    }
    if attrs & FILE_ATTRIBUTE_HIDDEN != 0 {
        s.push('H');
    }
    if attrs & FILE_ATTRIBUTE_SYSTEM != 0 {
        s.push('S');
    }
    if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        s.push('D');
    }
    if attrs & FILE_ATTRIBUTE_COMPRESSED != 0 {
        s.push('C');
    }

    if s.is_empty() {
        s.push('-');
    }
    s
}

// ---- Tests (manual / integration only — require live Everything GUI) --------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sort() {
        assert_eq!(parse_sort("name"), Some(Sort::NameAscending));
        assert_eq!(parse_sort("size"), Some(Sort::SizeDescending));
        assert_eq!(
            parse_sort("date_modified"),
            Some(Sort::DateModifiedDescending)
        );
        assert_eq!(parse_sort("bogus"), None);
    }

    #[test]
    fn test_parse_fields() {
        let flags = parse_fields(Some("filename,size,path"));
        assert!(flags.contains(RequestFlags::FileName));
        assert!(flags.contains(RequestFlags::Size));
        assert!(flags.contains(RequestFlags::Path));
        assert!(!flags.contains(RequestFlags::DateModified));

        // Default returns all common fields
        let all = parse_fields(None);
        assert!(all.contains(RequestFlags::FileName));
        assert!(all.contains(RequestFlags::Extension));

        // Empty input falls back to FileName
        let empty = parse_fields(Some(""));
        assert_eq!(empty, RequestFlags::FileName);
    }

    #[test]
    fn test_format_attributes() {
        assert_eq!(format_attributes(0x0020), "A");
        assert_eq!(format_attributes(0x0001), "R");
        assert_eq!(format_attributes(0x0021), "AR");
        assert_eq!(format_attributes(0x0000), "-");
    }

    #[test]
    fn test_unix_timestamp_to_rfc3339() {
        assert_eq!(unix_timestamp_to_rfc3339(0), "1970-01-01T00:00:00Z");
        let s = unix_timestamp_to_rfc3339(1_705_305_600);
        assert!(s.starts_with("2024-01-15"));
        assert!(s.ends_with("Z"));
    }

    #[test]
    fn test_ns100_to_iso_string() {
        assert!(ns100_to_iso_string(0).is_none());
        assert!(ns100_to_iso_string(1_000).is_none());
        let r = ns100_to_iso_string(116_444_736_000_000_000);
        assert_eq!(r.as_deref(), Some("1970-01-01T00:00:00Z"));
    }
}
