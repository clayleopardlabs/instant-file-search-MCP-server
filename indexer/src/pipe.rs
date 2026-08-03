//! Named pipe server: `\\.\pipe\instant-file-search-indexer`.
//!
//! Protocol: one JSON request per connection, one JSON response, then close.
//! Request:  {"method":"search","params":{...}} | {"method":"count","params":{...}}
//!           {"method":"status"} | {"method":"ping"}
//! Response: {"ok":true,"data":{...}} | {"ok":false,"error":"..."}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile, CreateFileW, OPEN_EXISTING,
    FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::query::{self, QueryOptions};
use crate::IndexerState;

pub const PIPE_NAME: &str = r"\\.\pipe\instant-file-search-indexer";
const MAX_PIPE_BYTES: u32 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct Response<'a> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn last_error() -> windows::core::Error {
    windows::core::Error::from_thread()
}

/// A HANDLE is a raw pointer in the windows crate (not Send), but kernel
/// handles are thread-agnostic: the value may be moved between threads freely.
struct SendHandle(HANDLE);
unsafe impl Send for SendHandle {}

/// Serve one connected pipe client until it disconnects. Runs on a
/// dedicated thread so concurrent clients never block each other.
fn serve_connection(state: IndexerState, stop: Arc<AtomicBool>, pipe: SendHandle) {
    let pipe = pipe.0;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let response = match read_request(pipe) {
            Ok(req) => handle(&state, req),
            Err(_) => break,
        };
        let mut body = serde_json::to_vec(&response).unwrap_or_default();
        body.push(b'\n');
        unsafe {
            let mut written = 0u32;
            let _ = WriteFile(pipe, Some(&body), Some(&mut written), None);
            let _ = FlushFileBuffers(pipe);
        }
    }
    unsafe {
        let _ = DisconnectNamedPipe(pipe);
        let _ = CloseHandle(pipe);
    }
}

/// Connect a dummy client to the pipe to unblock a pending ConnectNamedPipe
/// so the serve loop can observe the stop flag.
pub fn poke_stop() {
    let mut name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateFileW(
            PCWSTR(name.as_mut_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        );
        if let Ok(h) = h {
            let _ = CloseHandle(h);
        }
    }
}

pub struct PipeServer {
    state: IndexerState,
    security: SECURITY_ATTRIBUTES,
    stop: Arc<AtomicBool>,
}

impl PipeServer {
    #[allow(dead_code)]
    pub fn new(state: IndexerState) -> Result<Self> {
        Self::with_stop(state, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_stop(state: IndexerState, stop: Arc<AtomicBool>) -> Result<Self> {
        // DACL granting Everyone read/write, so non-elevated clients
        // (the MCP server runs as the user) can connect to the pipe.
        let sddl = "D:(A;;GRGW;;;WD)";
        let mut sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl_wide.as_mut_ptr()),
                1,
                &mut sd,
                None,
            )?
        };
        if sd.0.is_null() {
            anyhow::bail!("ConvertStringSecurityDescriptorToSecurityDescriptorW returned null");
        }
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        Ok(Self { state, security, stop })
    }

    pub fn run(&self) -> Result<()> {
        tracing::info!("listening on {PIPE_NAME}");
        while !self.stop.load(Ordering::SeqCst) {
            let pipe = self.create_pipe()?;
            let connected = unsafe { ConnectNamedPipe(pipe, None) };
            if connected.is_err() {
                let e = last_error();
                // ERROR_PIPE_CONNECTED means a client connected between
                // CreateNamedPipeW and ConnectNamedPipe — treat as connected.
                if e.code() != HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) {
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe);
                        let _ = CloseHandle(pipe);
                    }
                    continue;
                }
            }

            // Serve each connection on its own thread so concurrent clients
            // (the MCP plugin spawns a fresh server per tool call and may
            // issue parallel calls) never wait on each other.
            let state = self.state.clone();
            let stop = self.stop.clone();
            let pipe = SendHandle(pipe);
            std::thread::spawn(move || serve_connection(state, stop, pipe));
        }
        Ok(())
    }

    fn create_pipe(&self) -> Result<HANDLE> {
        let mut name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_mut_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                MAX_PIPE_BYTES,
                MAX_PIPE_BYTES,
                0,
                Some(&self.security),
            )
        };
        if pipe.is_invalid() {
            anyhow::bail!("CreateNamedPipeW failed: {}", last_error());
        }
        Ok(pipe)
    }
}

fn read_request(pipe: HANDLE) -> Result<Request, &'static str> {
        let mut buf = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        loop {
            let mut read = 0u32;
            let ok = unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut read), None) };
            if let Err(e) = ok {
                if e.code() == HRESULT::from_win32(windows::Win32::Foundation::ERROR_MORE_DATA.0) {
                    buf.extend_from_slice(&chunk[..read as usize]);
                    continue;
                }
                return Err("read failed");
            }
            if read == 0 {
                return Err("client disconnected");
            }
            buf.extend_from_slice(&chunk[..read as usize]);
            // In message mode ReadFile returns a complete message (or
            // ERROR_MORE_DATA for a longer one), so once we have a full
            // message the JSON must parse; otherwise it is a protocol error.
            if let Ok(req) = serde_json::from_slice::<Request>(&buf) {
                return Ok(req);
            }
            return Err("invalid JSON request");
        }
    }

    fn handle(state: &IndexerState, req: Request) -> Response<'static> {
        match req.method.as_str() {
            "ping" => Response { ok: true, data: Some(serde_json::json!({"pong": true})), error: None },
            "status" => {
                Response {
                    ok: true,
                    data: Some(serde_json::json!({
                        "indexed": state.index.len(),
                        "volumes": state.volumes.iter().map(|v| v.clone()).collect::<Vec<_>>(),
                    })),
                    error: None,
                }
            }
            "count" | "search" => {
                let mut opts: QueryOptions = match serde_json::from_value(req.params) {
                    Ok(o) => o,
                    Err(_) => return Response { ok: false, data: None, error: Some("bad params") },
                };
                apply_content_filter(state, &mut opts);
                let result = state.index.with_entries(|entries| query::search(entries, &opts));
                if req.method == "count" {
                    Response { ok: true, data: Some(serde_json::json!({"total": result.total})), error: None }
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
                    Err(_) => return Response { ok: false, data: None, error: Some("bad params") },
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
                let result = state
                    .index
                    .with_entries(|entries| query::aggregate(entries, &opts));
                Response { ok: true, data: Some(serde_json::json!(result)), error: None }
            }
            "recent_changes" => {
                #[derive(serde::Deserialize)]
                #[serde(default)]
                struct RecentParams {
                    since: i64,
                    limit: usize,
                }
                impl Default for RecentParams {
                    fn default() -> Self {
                        Self { since: 0, limit: 100 }
                    }
                }
                let p: RecentParams = match serde_json::from_value(req.params) {
                    Ok(o) => o,
                    Err(_) => return Response { ok: false, data: None, error: Some("bad params") },
                };
                let changes = state.index.recent_changes(p.since, p.limit);
                Response { ok: true, data: Some(serde_json::json!({"changes": changes})), error: None }
            }
            other => {
                tracing::warn!("unknown method {other}");
                Response { ok: false, data: None, error: Some("unknown method") }
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
fn extract_content_terms(query: &str) -> (String, Vec<String>) {
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
}
