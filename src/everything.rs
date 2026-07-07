// Everything IPC client wrapper using the `everything-ipc` crate's WM_COPYDATA
// communication with the Everything desktop search GUI.
//
// All functions are synchronous and blocking. The caller is responsible for
// running them on a blocking thread (e.g. via tokio::task::spawn_blocking).

use anyhow::Result;
use everything_ipc::wm::{EverythingClient, RequestFlags, SearchFlags, Sort};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
}

/// Collection of search results.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchResults {
    /// The result items in this page.
    pub results: Vec<SearchResult>,

    /// Total number of matching items (may be larger than results.len()).
    pub total: u64,
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
}

// ---- Public API ------------------------------------------------------------

/// Search Everything with the given parameters.
pub fn search(params: SearchParams) -> Result<SearchResults> {
    let client = create_client()?;

    // Build search text: optionally scope to path and exclude path
    let search_text = build_search_text(&params.query, params.path.as_deref(), params.exclude_path.as_deref());

    // Gather all optional parameters first (avoids type-state reassignment)
    let flags = parse_fields(params.fields.as_deref());
    let search_flags = if params.regex.unwrap_or(false) {
        SearchFlags::Regex
    } else {
        SearchFlags::empty()
    };
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

            SearchResult {
                filename,
                path,
                size,
                date_modified,
                date_created,
                date_accessed,
                attributes,
                extension,
            }
        })
        .collect();

    let total = list.total_len() as u64;
    Ok(SearchResults { results, total })
}

/// Count matching files without retrieving result items.
pub fn count(params: CountParams) -> Result<u64> {
    let client = create_client()?;

    let search_flags = if params.regex.unwrap_or(false) {
        SearchFlags::Regex
    } else {
        SearchFlags::empty()
    };

    let search_text = build_search_text(&params.query, None, params.exclude_path.as_deref());

    let list = client
        .query_wait(&search_text)
        .request_flags(RequestFlags::FileName)
        .search_flags(search_flags)
        .max_results(1)
        .call()
        .map_err(|e| anyhow::anyhow!("Everything count query failed: {e}"))?;

    Ok(list.total_len() as u64)
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
#[allow(dead_code)]
pub fn status() -> Result<EverythingStatus> {
    let client = match EverythingClient::new() {
        Ok(c) => c,
        Err(e) => {
            return Ok(EverythingStatus {
                connected: false,
                window_found: matches!(e, everything_ipc::wm::IpcError::NoIpcWindow),
                ipc_available: false,
                db_loaded: false,
                version: None,
            })
        }
    };

    let ipc_available = client.is_ipc_available();
    let db_loaded = client.is_db_loaded();

    Ok(EverythingStatus {
        connected: ipc_available && db_loaded,
        window_found: true,
        ipc_available,
        db_loaded,
        version: None,
    })
}

// ---- Internal helpers ------------------------------------------------------

/// Try to create an Everything IPC client, with a user-friendly bail! on
/// failure.
fn create_client() -> Result<EverythingClient> {
    EverythingClient::new().map_err(|e| {
        anyhow::anyhow!(
            "Everything is not running. Please start Everything \
             (C:\\Program Files\\Everything\\Everything.exe). \
             The GUI window must be visible for IPC. (underlying error: {e})"
        )
    })
}

/// Build the search text string: concat path scope + query + exclusion pattern.
///
/// Supports Everything's `!` exclusion syntax: prepends `!<exclude_path> `
/// to exclude a folder/prefix from results.
fn build_search_text(query: &str, path: Option<&str>, exclude_path: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);

    // Path scope: prepend as a prefix to the query
    if let Some(p) = path {
        if !p.is_empty() {
            let normalised = Path::new(p)
                .to_string_lossy()
                .trim_end_matches('\\')
                .to_string();
            parts.push(normalised);
        }
    }

    parts.push(query.to_string());

    // Exclusion: Everything uses ! prefix for NOT
    if let Some(ep) = exclude_path {
        if !ep.is_empty() {
            let normalised = Path::new(ep)
                .to_string_lossy()
                .trim_end_matches('\\')
                .to_string();
            parts.push(format!("!{}", normalised));
        }
    }

    parts.join(" ")
}

/// Map a sort string to the corresponding `Sort` enum value.
fn parse_sort(s: &str) -> Option<Sort> {
    match s {
        "name" => Some(Sort::NameAscending),
        "path" => Some(Sort::PathAscending),
        "size" => Some(Sort::SizeDescending),
        "date_modified" => Some(Sort::DateModifiedDescending),
        "date_created" => Some(Sort::DateCreatedDescending),
        "date_accessed" => Some(Sort::DateAccessedDescending),
        "extension" => Some(Sort::ExtensionAscending),
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
fn ns100_to_iso_string(ns100: u64) -> Option<String> {
    if ns100 == 0 {
        return None;
    }
    // FILETIME epoch (1601-01-01) -> Unix epoch (1970-01-01) offset
    const EPOCH_OFFSET: u64 = 1_164_447_360_000_000_000;
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
fn format_attributes(attrs: u32) -> String {
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
        let r = ns100_to_iso_string(11_644_473_600_000_000_00);
        assert_eq!(r.as_deref(), Some("1970-01-01T00:00:00Z"));
    }
}
