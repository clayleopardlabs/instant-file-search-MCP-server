//! Portable result types and formatting helpers shared by every engine.
//!
//! These types are produced by both the native indexer (`native.rs`) and the
//! Everything fallback (`everything.rs`, Windows-only). They must stay free
//! of platform-specific imports so the MCP server compiles on Linux, where
//! the Everything engine does not exist.

use serde::{Deserialize, Serialize};

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

// ---- Time formatting (FILETIME 100 ns -> ISO 8601) -------------------------

/// Convert a 100 ns FILETIME (since 1601-01-01) to an ISO 8601 UTC string.
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
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x0100;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0800;
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
    const FILE_ATTRIBUTE_NOT_CONTENT_INDEXED: u32 = 0x2000;
    const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;

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
    if attrs & FILE_ATTRIBUTE_TEMPORARY != 0 {
        s.push('T');
    }
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        s.push('P');
    }
    if attrs & FILE_ATTRIBUTE_COMPRESSED != 0 {
        s.push('C');
    }
    if attrs & FILE_ATTRIBUTE_OFFLINE != 0 {
        s.push('O');
    }
    if attrs & FILE_ATTRIBUTE_NOT_CONTENT_INDEXED != 0 {
        s.push('I');
    }
    if attrs & FILE_ATTRIBUTE_ENCRYPTED != 0 {
        s.push('E');
    }

    if s.is_empty() {
        s.push('-');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

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
