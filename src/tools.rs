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
    #[allow(dead_code)]
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
#[schemars(description = "Find files instantly across ALL indexed drives. Supports wildcards (*.txt, *.rs), content search (content:text), date filters (dm:lastweek), size filters (size:>1mb), regex, path scoping, sorting, and pagination. Default excludes node_modules, .git, WinSxS, $Recycle.Bin, System Volume Information. Pass include_all=true to search everything.")]
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
    #[allow(dead_code)]
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
#[schemars(description = "Check the search service health, and optionally whether the index still agrees with the filesystem.")]
pub struct StatusParams {
    /// Stat this many indexed paths and report any that are missing or whose size changed. This is how you tell whether a total is trustworthy: a stale index gives confident wrong numbers. `path` is required with it.
    pub verify_sample: Option<u32>,
    /// Folder to sample for verification. Required when verify_sample is set, and it should be the folder whose totals you intend to quote.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Recently changed files from the NTFS change journal: created/modified/renamed/deleted with timestamps and paths. Native-only.")]
pub struct RecentChangesParams {
    /// Only return events with a timestamp (FILETIME, 100ns since 1601) strictly newer than this. 0 returns all retained events.
    pub since: Option<i64>,
    /// EASIER ALTERNATIVE to 'since': return events from the last N hours. Pass a small integer like 1 or 24. The server computes the FILETIME for you. Use this instead of 'since' to avoid 18-digit FILETIME math.
    pub hours: Option<u32>,
    /// Maximum number of events to return. Default 100 (newest first). 0 returns all retained events (ring buffer capped at 100,000).
    pub limit: Option<usize>,
    /// Only return events with these reasons. Comma-separated subset of: created, modified, renamed, deleted. Default: all. USE THIS to skip delete noise (e.g. reasons=created,modified).
    pub reasons: Option<String>,
}

/// Tool-level default type filter: `find_files` and `count_files` return files.
///
/// Directories are indexed and carry the aggregate size of everything beneath
/// them, so an unqualified `size:>1gb` ranked `C:` and `Users` above every real
/// file. Models ask a file search for files, so the default appends the `file:`
/// type token - which both the native engine and the Everything fallback parse,
/// keeping their counts in step.
///
/// A query that already chooses a type is left untouched: `is:file`,
/// `is:folder`, `file:`, `folder:`, or any `attrib:` filter (that is how a
/// caller asks for directories, e.g. `attrib:d` or `is:hidden`).
pub fn has_explicit_type(query: &str) -> bool {
    query.split_whitespace().any(|word| {
        let word = word.to_ascii_lowercase();
        word.starts_with("is:")
            || word.starts_with("file:")
            || word.starts_with("folder:")
            || word.starts_with("attrib:")
    })
}

/// Append the `file:` type token unless the query already chooses a type.
///
/// Only the Everything fallback uses this now. The native engine takes a
/// `files_only` option instead, because rewriting the query string breaks
/// whole-query regex mode (`regex: true`) and nests regex tokens inside an
/// AND-group. See 553db4d for the original change and the regex regression it
/// caused.
pub fn default_to_files(query: &str) -> String {
    if has_explicit_type(query) {
        return query.to_string();
    }
    if query.trim().is_empty() {
        return "file:".to_string();
    }
    format!("{query} file:")
}

/// Delivered to the model in the `initialize` response, so every session starts
/// with the rules that are easy to get wrong. Keep it short, it is always in
/// context.
///
/// These are the mistakes observed in real agent sessions:
/// - `aggregate_files(query="*")` double-counted folders (folders carry subtree
///   size) and inflated totals ~4x.
/// - Forgetting `include_all` silently dropped node_modules/.git/WinSxS.
/// - `OR` instead of `|` returned zero results.
/// - `path:...` inside the query returned zero results.
/// - Trusting an index hit that had been deleted a year earlier.
pub const AGENT_INSTRUCTIONS: &str = "\
Instant File Search: rules that prevent silent wrong answers.

1. aggregate_files reports total_file_size, the sum of matched file sizes. It is
   files-only like the other tools, so a bare \"*\" is safe. Pass is:folder when
   you want directory rows instead.
2. Default excludes are ON: node_modules, .git, WinSxS, $Recycle.Bin, System Volume Information. For any disk-usage
   question, or when comparing against du/Get-ChildItem, pass include_all=true; those folders ARE indexed, so a zero result without it means excluded, not absent.
3. OR is the pipe: '*.a | *.b'. Writing 'OR' is treated as a literal name term
   and returns zero results.
4. Scope with the `path` parameter (C:/Users/x). Do NOT put path:... inside the
   query: it returns zero results for paths containing spaces.
5. Multi-extension: ext:lsv,d2s or '*.a | *.b'.
6. Results are FILES by default; add is:folder for directories.
7. An invalid filter token matches NOTHING, not everything. If a query returns
   zero, re-read your tokens before concluding the file is absent.
8. The index is a lead source, not an oracle. Confirm any path you will act on
   (Test-Path / stat) before telling a user it exists.
9. When your numbers differ from Explorer or a shell, that is usually expected,
   not a bug. total_file_size sums LOGICAL file lengths: it reads lower than
   Explorer's `size on disk` because cluster slack, NTFS compression and
   alternate data streams are not counted. It can also read higher or lower than
   a shell count: the index sees ACL-protected paths a shell cannot read, and it
   omits the five folders in rule 2 unless include_all is set.
10. To check whether the index itself has drifted, call search_status with
   verify_sample=N and a path. It stats N indexed paths and reports any that are
   missing or whose size changed, which is the only reliable way to spot a stale
   index before quoting a total from it.";

#[cfg(test)]
mod tests {
    use super::default_to_files;

    #[test]
    fn adds_the_file_filter_when_no_type_is_given() {
        assert_eq!(default_to_files("*.rs"), "*.rs file:");
        assert_eq!(default_to_files("size:>1gb"), "size:>1gb file:");
        assert_eq!(default_to_files("content:\"fn main\""), "content:\"fn main\" file:");
        assert_eq!(default_to_files(""), "file:");
        assert_eq!(default_to_files("   "), "file:");
    }

    #[test]
    fn leaves_an_explicit_type_alone() {
        for query in [
            "is:folder",
            "*.rs is:file",
            "folder:src",
            "file:*.log",
            "attrib:d",
            "size:>1gb attrib:h",
        ] {
            assert_eq!(default_to_files(query), query, "query {query} must not be rewritten");
        }
    }

    #[test]
    fn a_word_that_merely_contains_a_token_is_not_a_type() {
        // "this:" and "relist:" must not be mistaken for "is:".
        assert_eq!(default_to_files("relist:today"), "relist:today file:");
    }
}
