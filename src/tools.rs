use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Find files matching query using Everything NTFS index. Supports wildcards (*.txt), regex, path filters, date/size sorting, and pagination. Default scope is ALL indexed drives (C:, D:, etc.). By default automatically excludes node_modules, .git, and WinSxS directories. Pass include_all=true to search everything.")]
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
    pub sort: Option<String>,
    /// Restrict search to files under this path. Example: "C:\\Users" or "B:\\Projects".
    pub path: Option<String>,
    /// Comma-separated list of fields to return: filename, path, size, date_modified, date_created, date_accessed, attributes, extension, run_count, date_run, date_recently_changed.
    pub fields: Option<String>,
    /// Directories to exclude from results. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`. Example: "C:\\Windows\\WinSxS;C:\\Program Files" skips both directories. Can also be a partial path like "node_modules" to exclude all folders with that name anywhere.
    pub exclude_path: Option<String>,
    /// Set to true to search ALL files including node_modules, .git, and WinSxS. Default: false (these dirs are excluded automatically).
    pub include_all: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Fast count of files matching query using Everything NTFS index. Default scope is ALL indexed drives. By default automatically excludes node_modules, .git, and WinSxS directories. Pass include_all=true to count everything.")]
pub struct CountParams {
    pub query: String,
    /// Narrow search to a specific directory. Everything automatically normalises path separators.
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
