use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sort order for search results. Mirrors the sort tokens accepted by both
/// the native indexer and the Everything engine. Serializes to the wire token
/// (e.g. `date_modified_asc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// Sort by file name, ascending (default).
    Name,
    /// Sort by file name, descending.
    NameDesc,
    /// Sort by full path, ascending.
    Path,
    /// Sort by full path, descending.
    PathDesc,
    /// Sort by size, largest first.
    Size,
    /// Sort by size, smallest first.
    SizeAsc,
    /// Sort by last-modified time, newest first.
    DateModified,
    /// Sort by last-modified time, oldest first.
    DateModifiedAsc,
    /// Sort by creation time, newest first.
    DateCreated,
    /// Sort by creation time, oldest first.
    DateCreatedAsc,
    /// Sort by last-accessed time, newest first.
    DateAccessed,
    /// Sort by last-accessed time, oldest first.
    DateAccessedAsc,
    /// Sort by file extension, then name.
    Extension,
    /// Sort by file extension, descending, then name.
    ExtensionDesc,
}

impl SortOrder {
    /// The wire token for this sort order (matches the indexer's sort keys).
    pub fn as_str(&self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::NameDesc => "name_desc",
            SortOrder::Path => "path",
            SortOrder::PathDesc => "path_desc",
            SortOrder::Size => "size",
            SortOrder::SizeAsc => "size_asc",
            SortOrder::DateModified => "date_modified",
            SortOrder::DateModifiedAsc => "date_modified_asc",
            SortOrder::DateCreated => "date_created",
            SortOrder::DateCreatedAsc => "date_created_asc",
            SortOrder::DateAccessed => "date_accessed",
            SortOrder::DateAccessedAsc => "date_accessed_asc",
            SortOrder::Extension => "extension",
            SortOrder::ExtensionDesc => "extension_desc",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Find files instantly across ALL indexed drives. Supports wildcards (*.txt, *.rs), content search (content:text), date filters (dm:lastweek), size filters (size:>1mb), regex, path scoping, sorting, and pagination. Default excludes node_modules, .git, WinSxS. Pass include_all=true to search everything.")]
pub struct SearchParams {
    pub query: String,
    pub max_results: Option<u32>,
    /// Number of results to skip (for pagination). Use with max_results to implement offset-based paging.
    pub offset: Option<u32>,
    pub regex: Option<bool>,
    /// Enable case-sensitive matching (default: case-insensitive).
    pub match_case: Option<bool>,
    /// Match whole words only, not substrings.
    pub match_whole_word: Option<bool>,
    /// Also search the full file path, not just the file name.
    pub match_path: Option<bool>,
    /// Sort order for results. Valid values: name, name_desc, path, path_desc,
    /// size (largest first), size_asc, date_modified (newest first),
    /// date_modified_asc, date_created, date_created_asc, date_accessed,
    /// date_accessed_asc, extension, extension_desc. Default: name.
    pub sort: Option<SortOrder>,
    /// Restrict search to files under this path. USE FORWARD SLASHES to avoid JSON escaping: "C:/Users" or "B:/Projects". Backslashes also work but must be escaped in JSON.
    pub path: Option<String>,
    /// Comma-separated list of fields to return: filename, path, size, date_modified, date_created, date_accessed, attributes, extension, run_count, date_run, date_recently_changed.
    pub fields: Option<String>,
    /// Directories to exclude from results. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`. Example: "C:\\Windows\\WinSxS;C:\\Program Files" skips both directories. Can also be a partial path like "node_modules" to exclude all folders with that name anywhere.
    pub exclude_path: Option<String>,
    /// Set to true to search ALL files including node_modules, .git, and WinSxS. Default: false (these dirs are excluded automatically).
    pub include_all: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Count files matching a query instantly. Returns count without transferring file data. USE THIS before find_files for broad patterns.")]
pub struct CountParams {
    pub query: String,
    /// Narrow search to a specific directory. USE FORWARD SLASHES to avoid JSON escaping: "C:/Users". Everything normalises path separators automatically.
    pub path: Option<String>,
    pub regex: Option<bool>,
    /// Enable case-sensitive matching (default: case-insensitive).
    pub match_case: Option<bool>,
    /// Match whole words only, not substrings.
    pub match_whole_word: Option<bool>,
    /// Directories to exclude from count. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`. Example: "C:\\Windows\\WinSxS;node_modules" skips System32 assembly cache AND all folders named node_modules.
    pub exclude_path: Option<String>,
    /// Set to true to count ALL files including node_modules, .git, and WinSxS. Default: false (these dirs are excluded automatically).
    pub include_all: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Disk usage stats for files matching a query: total count, total size, per-extension breakdown, and largest files. Native-only.")]
pub struct AggregateParams {
    pub query: String,
    /// Narrow aggregation to files under this path (a directory scope). USE FORWARD SLASHES to avoid JSON escaping: "C:/Users" or "B:/Projects".
    pub path: Option<String>,
    pub regex: Option<bool>,
    /// Enable case-sensitive matching (default: case-insensitive).
    pub match_case: Option<bool>,
    /// Match whole words only, not substrings.
    pub match_whole_word: Option<bool>,
    /// Directories to exclude. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`.
    pub exclude_path: Option<String>,
    /// Set to true to include node_modules, .git, and WinSxS. Default: false.
    pub include_all: Option<bool>,
    /// How many of the largest entries to return (default 20).
    pub top: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Recently changed files from the NTFS change journal: created/modified/renamed/deleted with timestamps and paths. Native-only.")]
pub struct RecentChangesParams {
    /// Only return events with a timestamp (FILETIME, 100ns since 1601) strictly newer than this. 0 returns all retained events.
    pub since: Option<i64>,
    /// EASIER ALTERNATIVE to 'since': return events from the last N hours. Pass a small integer like 1 or 24. The server computes the FILETIME for you. Use this instead of 'since' to avoid 18-digit FILETIME math.
    pub hours: Option<u32>,
    /// Maximum number of events to return. 0 (default) returns all retained events (ring buffer capped at 100,000).
    pub limit: Option<usize>,
}
