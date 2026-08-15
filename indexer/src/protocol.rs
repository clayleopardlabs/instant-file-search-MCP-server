//! Portable request/response protocol for the indexer IPC.
//!
//! Transport-agnostic: the Windows named-pipe server and the Linux Unix
//! socket server both speak this protocol. One JSON request per connection,
//! one JSON response (newline-terminated), then close.
//!
//! Request:  {"method":"search","params":{...}} | {"method":"count","params":{...}}
//!           {"method":"status"} | {"method":"ping"}
//! Response: {"ok":true,"data":{...}} | {"ok":false,"error":"..."}

use serde::{Deserialize, Serialize};

use crate::query::{self, QueryOptions};
use crate::IndexerState;

#[derive(Debug, Deserialize)]
pub struct Request {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct Response<'a> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a str>,
}

pub fn handle(state: &IndexerState, req: Request) -> Response<'static> {
    match req.method.as_str() {
        "ping" => Response {
            ok: true,
            data: Some(serde_json::json!({"pong": true})),
            error: None,
        },
        "status" => Response {
            ok: true,
            data: Some(serde_json::json!({
                "version": crate::APP_VERSION,
                "commit": crate::BUILD_COMMIT,
                "indexed": state.index.len(),
                "volumes": state.volumes.iter().map(|v| v.clone()).collect::<Vec<_>>(),
                "storage_mode": state.index.storage_mode(),
                "index_path": state.index.disk_path().map(|p| p.display().to_string()),
            })),
            error: None,
        },
        "count" | "search" => {
            let mut opts: QueryOptions = match serde_json::from_value(req.params) {
                Ok(o) => o,
                Err(_) => {
                    return Response {
                        ok: false,
                        data: None,
                        error: Some("bad params"),
                    }
                }
            };
            apply_content_filter(state, &mut opts);
            let result = state.index.search(&opts);
            if req.method == "count" {
                Response {
                    ok: true,
                    data: Some(serde_json::json!({"total": result.total})),
                    error: None,
                }
            } else {
                let entries: Vec<_> = result
                    .entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "path": e.path,
                            "size": e.size,
                            "created": e.created,
                            "modified": e.modified,
                            "accessed": e.accessed,
                            "is_dir": e.is_dir,
                            "attributes": e.attributes,
                            "extension": e.extension,
                        })
                    })
                    .collect();
                Response {
                    ok: true,
                    data: Some(serde_json::json!({"total": result.total, "entries": entries})),
                    error: None,
                }
            }
        }
        "aggregate" => {
            let mut opts: query::AggregateOptions = match serde_json::from_value(req.params) {
                Ok(o) => o,
                Err(_) => {
                    return Response {
                        ok: false,
                        data: None,
                        error: Some("bad params"),
                    }
                }
            };
            let mut qopts = QueryOptions {
                query: opts.query.clone(),
                path: opts.path.clone(),
                exclude_path: opts.exclude_path.clone(),
                include_all: opts.include_all,
                ..Default::default()
            };
            apply_content_filter(state, &mut qopts);
            opts.query = qopts.query;
            opts.content_paths = qopts.content_paths;
            let result = state.index.aggregate(&opts);
            Response {
                ok: true,
                data: Some(serde_json::json!(result)),
                error: None,
            }
        }
        "recent_changes" => {
            #[derive(serde::Deserialize)]
            #[serde(default)]
            struct RecentParams {
                since: i64,
                limit: usize,
                reasons: Option<String>,
            }
            impl Default for RecentParams {
                fn default() -> Self {
                    Self {
                        since: 0,
                        limit: 100,
                        reasons: None,
                    }
                }
            }
            let p: RecentParams = match serde_json::from_value(req.params) {
                Ok(o) => o,
                Err(_) => {
                    return Response {
                        ok: false,
                        data: None,
                        error: Some("bad params"),
                    }
                }
            };
            let changes =
                state
                    .index
                    .recent_changes_filtered(p.since, p.limit, p.reasons.as_deref());
            Response {
                ok: true,
                data: Some(serde_json::json!({"changes": changes})),
                error: None,
            }
        }
        other => {
            tracing::warn!("unknown method {other}");
            Response {
                ok: false,
                data: None,
                error: Some("unknown method"),
            }
        }
    }
}

/// Extract `content:"..."` tokens from a query, resolve them against the
/// content store, strip the tokens from the query, and set `opts.content_paths`
/// to the lowercase paths whose content matches every needle.
fn apply_content_filter(state: &IndexerState, opts: &mut QueryOptions) {
    let (query, needles) = extract_content_terms(&opts.query);
    opts.query = query;
    if needles.is_empty() {
        opts.content_paths = None;
        return;
    }
    let paths = state.content.matching_paths(&needles);
    opts.content_paths = if paths.is_empty() {
        Some(Vec::new())
    } else {
        Some(paths)
    };
}

/// Pull every `content:"..."` (or `content:word`) term out of a query string,
/// returning the remaining query and the collected needles. Handles quotes
/// (spaces inside quotes stay in the needle).
pub fn extract_content_terms(query: &str) -> (String, Vec<String>) {
    let mut needles = Vec::new();
    let mut rest = String::with_capacity(query.len());
    let bytes = query.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let token_start = i;
        let mut consumed = false;
        if bytes[i] == b'"' {
            i += 1;
            let inner = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let content = &query[inner..i];
            if i < bytes.len() {
                i += 1; // closing quote
            }
            if let Some(value) = content.strip_prefix("content:") {
                if !value.is_empty() {
                    needles.push(value.to_string());
                    consumed = true;
                }
            }
            if !consumed {
                rest.push_str(&query[start..inner - 1]);
                rest.push_str(content);
                if i <= bytes.len() {
                    rest.push('"');
                }
            }
        } else {
            while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
                i += 1;
            }
            let token = &query[token_start..i];
            if let Some(value) = token.strip_prefix("content:") {
                if value.starts_with('"') {
                    // content:"..." with spaces inside quotes: scan ahead to
                    // the closing quote so the whole phrase stays one needle.
                    let mut q = i;
                    while q < bytes.len() && bytes[q] != b'"' {
                        q += 1;
                    }
                    let inner = &query[token_start + "content:".len() + 1..q];
                    if !inner.is_empty() {
                        needles.push(inner.to_string());
                        consumed = true;
                        i = if q < bytes.len() { q + 1 } else { q };
                    }
                } else if !value.is_empty() {
                    needles.push(value.to_string());
                    consumed = true;
                }
            }
            if !consumed {
                rest.push_str(&query[start..token_start]);
                rest.push_str(token);
            }
        }
        if !consumed && i < bytes.len() && (bytes[i] as char).is_whitespace() {
            rest.push(' ');
        }
    }
    (rest.trim().to_string(), needles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::content::ContentStore;
    use crate::index::FileIndex;
    use crate::types::IndexedFile;

    fn extract(q: &str) -> (String, Vec<String>) {
        extract_content_terms(q)
    }

    fn rest_words(rest: &str) -> Vec<&str> {
        rest.split_whitespace().collect()
    }

    #[test]
    fn plain_query_passthrough() {
        let (rest, needles) = extract("foo bar");
        assert_eq!(rest_words(&rest), vec!["foo", "bar"]);
        assert!(needles.is_empty());
    }

    #[test]
    fn bare_content_token() {
        let (rest, needles) = extract("foo content:needle bar");
        assert_eq!(rest_words(&rest), vec!["foo", "bar"]);
        assert_eq!(needles, vec!["needle"]);
    }

    #[test]
    fn quoted_content_token() {
        let (rest, needles) = extract(r#"content:"fn main""#);
        assert_eq!(rest, "");
        assert_eq!(needles, vec!["fn main"]);
    }

    #[test]
    fn quoted_content_mixed_with_query() {
        let (rest, needles) = extract(r#"src content:"pub struct Foo" baz"#);
        assert_eq!(rest_words(&rest), vec!["src", "baz"]);
        assert_eq!(needles, vec!["pub struct Foo"]);
    }

    #[test]
    fn fully_quoted_content_token() {
        let (rest, needles) = extract(r#""content:fn main""#);
        assert_eq!(rest, "");
        assert_eq!(needles, vec!["fn main"]);
    }

    #[test]
    fn multiple_content_tokens() {
        let (rest, needles) = extract(r#"content:"a b" content:c"#);
        assert_eq!(rest, "");
        assert_eq!(needles, vec!["a b", "c"]);
    }

    #[test]
    fn unquoted_content_with_following_words() {
        let (rest, needles) = extract("content:needle rest here");
        assert_eq!(rest_words(&rest), vec!["rest", "here"]);
        assert_eq!(needles, vec!["needle"]);
    }

    #[test]
    fn disk_backend_serves_every_protocol_method() {
        let path = std::env::temp_dir().join(format!(
            "instant-file-search-protocol-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let index = Arc::new(FileIndex::for_benchmark("disk", path.clone()).unwrap());
        index.replace(vec![IndexedFile::new(
            r"C:\work\alpha.rs".into(),
            42,
            1,
            2,
            3,
            false,
            1,
        )]);
        index.record_change(10, "CREATE", r"C:\work\alpha.rs", false);
        let state = crate::IndexerState {
            index: index.clone(),
            content: Arc::new(ContentStore::disabled()),
            volumes: vec!["C:\\".into()],
        };
        for request in [
            Request {
                method: "ping".into(),
                params: serde_json::Value::Null,
            },
            Request {
                method: "status".into(),
                params: serde_json::Value::Null,
            },
            Request {
                method: "search".into(),
                params: serde_json::json!({"query":"alpha","max_results":100}),
            },
            Request {
                method: "count".into(),
                params: serde_json::json!({"query":"alpha"}),
            },
            Request {
                method: "aggregate".into(),
                params: serde_json::json!({"query":"*.rs"}),
            },
            Request {
                method: "recent_changes".into(),
                params: serde_json::json!({"since":0,"limit":10}),
            },
        ] {
            let response = handle(&state, request);
            assert!(response.ok, "protocol method failed");
        }
        let status = handle(
            &state,
            Request {
                method: "status".into(),
                params: serde_json::Value::Null,
            },
        );
        assert_eq!(status.data.unwrap()["storage_mode"], "disk");
        drop(state);
        drop(index);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }
}
