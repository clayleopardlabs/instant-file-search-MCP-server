//! Named pipe server: `\\.\pipe\instant-file-search-indexer`.
//!
//! Protocol: one JSON request per connection, one JSON response, then close.
//! Request:  {"method":"search","params":{...}} | {"method":"count","params":{...}}
//!           {"method":"status"} | {"method":"ping"}
//! Response: {"ok":true,"data":{...}} | {"ok":false,"error":"..."}

use anyhow::Result;
use serde::{Deserialize, Serialize};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
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

pub struct PipeServer {
    state: IndexerState,
    security: SECURITY_ATTRIBUTES,
}

impl PipeServer {
    pub fn new(state: IndexerState) -> Result<Self> {
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
        Ok(Self { state, security })
    }

    pub fn run(&self) -> Result<()> {
        tracing::info!("listening on {PIPE_NAME}");
        loop {
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

            loop {
                let response = match self.read_request(pipe) {
                    Ok(req) => self.handle(req),
                    Err(_) => break true,
                };
                let mut body = serde_json::to_vec(&response).unwrap_or_default();
                body.push(b'\n');
                unsafe {
                    let mut written = 0u32;
                    let _ = WriteFile(pipe, Some(&body), Some(&mut written), None);
                    let _ = FlushFileBuffers(pipe);
                }
            };
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
                let _ = CloseHandle(pipe);
            }
        }
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

    fn read_request(&self, pipe: HANDLE) -> Result<Request, &'static str> {
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
                break;
            }
            buf.extend_from_slice(&chunk[..read as usize]);
            if read < chunk.len() as u32 {
                break;
            }
        }
        serde_json::from_slice::<Request>(&buf).map_err(|_| "invalid JSON request")
    }

    fn handle(&self, req: Request) -> Response<'static> {
        match req.method.as_str() {
            "ping" => Response { ok: true, data: Some(serde_json::json!({"pong": true})), error: None },
            "status" => {
                Response {
                    ok: true,
                    data: Some(serde_json::json!({
                        "indexed": self.state.index.len(),
                        "volumes": self.state.volumes.iter().map(|v| v.clone()).collect::<Vec<_>>(),
                    })),
                    error: None,
                }
            }
            "count" | "search" => {
                let opts: QueryOptions = match serde_json::from_value(req.params) {
                    Ok(o) => o,
                    Err(_) => return Response { ok: false, data: None, error: Some("bad params") },
                };
                let result = self.state.index.with_entries(|entries| query::search(entries, &opts));
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
            other => {
                tracing::warn!("unknown method {other}");
                Response { ok: false, data: None, error: Some("unknown method") }
            }
        }
    }
}
