use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Search for files matching query")]
pub struct SearchParams {
    pub query: String,
    pub max_results: Option<u32>,
    pub regex: Option<bool>,
    pub sort: Option<String>,
    pub path: Option<String>,
    pub fields: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(description = "Fast count of matching files")]
pub struct CountParams {
    pub query: String,
    pub regex: Option<bool>,
}
