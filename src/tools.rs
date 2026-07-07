use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Find files matching query using Everything NTFS index. Supports wildcards (*.txt), regex, path filters, and date/size sorting. Default scope is ALL indexed drives (C:, D:, etc.). WARNING: broad queries on C: will include node_modules, WinSxS, .git, and other build artifacts — pass exclude_path to remove noise.")]
pub struct SearchParams {
    pub query: String,
    pub max_results: Option<u32>,
    pub regex: Option<bool>,
    pub sort: Option<String>,
    pub path: Option<String>,
    pub fields: Option<String>,
    /// Directories to exclude from results. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`. Example: "C:\\Windows\\WinSxS;C:\\Program Files" skips both directories. Can also be a partial path like "node_modules" to exclude all folders with that name anywhere.
    pub exclude_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Fast count of files matching query using Everything NTFS index. Default scope is ALL indexed drives. WARNING: broad queries on C: will include node_modules, WinSxS, .git — ALWAYS pass exclude_path to filter noise on C: drive queries.")]
pub struct CountParams {
    pub query: String,
    pub regex: Option<bool>,
    /// Directories to exclude from count. Supports Everything ! prefix syntax. Pass one or more paths separated by `;`. Example: "C:\\Windows\\WinSxS;node_modules" skips System32 assembly cache AND all folders named node_modules.
    pub exclude_path: Option<String>,
}
