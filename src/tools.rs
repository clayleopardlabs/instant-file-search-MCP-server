use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Find files matching query using Everything NTFS index. Supports wildcards (*.txt), regex, path filters, and date/size sorting. Default scope is ALL indexed drives (C:, D:, etc.).")]
pub struct SearchParams {
    pub query: String,
    pub max_results: Option<u32>,
    pub regex: Option<bool>,
    pub sort: Option<String>,
    pub path: Option<String>,
    pub fields: Option<String>,
    pub exclude_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Fast count of files matching query using Everything NTFS index. Default scope is ALL indexed drives. Pass path to narrow scope.")]
pub struct CountParams {
    pub query: String,
    pub regex: Option<bool>,
    pub exclude_path: Option<String>,
}
