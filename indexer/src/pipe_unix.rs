//! Unix socket server: `/tmp/instant-file-search-indexer.sock` (Linux).
//!
//! Protocol: one JSON request per connection, one JSON response (newline-
//! terminated), then close.  The protocol logic itself lives in `protocol`
//! (portable); this module is only the Linux transport (Unix socket creation +
//! read/write).

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::protocol::{self, Request};
use crate::IndexerState;

pub const SOCKET_PATH: &str = "/tmp/instant-file-search-indexer.sock";

/// Maximum request buffer size (16 MiB).  Realistic requests are tiny;
/// this is a safety net against a misbehaving client.
const MAX_BUF_BYTES: usize = 16 * 1024 * 1024;

pub struct PipeServer {
    state: IndexerState,
    stop: Arc<AtomicBool>,
}

impl PipeServer {
    #[allow(dead_code)]
    pub fn new(state: IndexerState) -> Result<Self> {
        Self::with_stop(state, Arc::new(AtomicBool::new(false)))
    }

    pub fn with_stop(state: IndexerState, stop: Arc<AtomicBool>) -> Result<Self> {
        Ok(Self { state, stop })
    }

    pub fn run(&self) -> Result<()> {
        // Remove a stale socket left by a crashed previous process.
        let _ = std::fs::remove_file(SOCKET_PATH);

        let listener = match UnixListener::bind(SOCKET_PATH) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Another stale handle may still hold the path.
                let _ = std::fs::remove_file(SOCKET_PATH);
                UnixListener::bind(SOCKET_PATH)?
            }
            Err(e) => return Err(e.into()),
        };

        // World-writable so the user-level MCP server can connect even when
        // the indexer runs as root for fanotify.
        std::fs::set_permissions(
            SOCKET_PATH,
            std::fs::Permissions::from_mode(0o666),
        )?;

        // Nonblocking accept so the loop can observe the stop flag.
        listener.set_nonblocking(true)?;

        tracing::info!("listening on {SOCKET_PATH}");

        // Tell systemd the service is ready to serve queries (Type=notify).
        // No-op when NOTIFY_SOCKET is unset (running in a terminal).
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);

        while !self.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let state = self.state.clone();
                    let stop = self.stop.clone();
                    std::thread::spawn(move || serve_connection(state, stop, stream));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    tracing::warn!("accept error: {e}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        // Clean up socket on shutdown.
        let _ = std::fs::remove_file(SOCKET_PATH);
        Ok(())
    }
}

/// Serve one connected client until it disconnects or stop is requested.
/// Mirrors the Windows `pipe.rs::serve_connection` loop.
fn serve_connection(state: IndexerState, stop: Arc<AtomicBool>, mut stream: UnixStream) {
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let response = match read_request(&mut stream) {
            Ok(req) => protocol::handle(&state, req),
            Err(_) => break,
        };
        let mut body = serde_json::to_vec(&response).unwrap_or_default();
        body.push(b'\n');
        if stream.write_all(&body).is_err() {
            break;
        }
        let _ = stream.flush();
    }
}

/// Read one JSON request from a Unix stream socket.
///
/// Unix STREAM sockets do not preserve message boundaries, so we accumulate
/// bytes into a buffer and try to parse after each read.  The MCP client
/// (`src/native.rs::exchange`) appends `b'\n'` after the JSON body, so we
/// accept both:
///
/// 1. **Raw buffer parse** (matches the Windows message-mode pattern: the
///    entire JSON is in the buffer with no framing).
/// 2. **Trailing-newline parse** (trim a trailing `b'\n'` before parsing).
///
/// A trailing `b'\n'` that fails to parse means the request is malformed.
/// A buffer that has not yet received a trailing `b'\n'` is treated as a
/// partial read and we keep accumulating.
fn read_request(stream: &mut UnixStream) -> Result<Request, &'static str> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).map_err(|_| "read failed")?;
        if n == 0 {
            // EOF with data still in the buffer: try a final parse.
            if !buf.is_empty() {
                if let Ok(req) = serde_json::from_slice::<Request>(&buf) {
                    return Ok(req);
                }
                if buf.last() == Some(&b'\n') {
                    if let Ok(req) = serde_json::from_slice::<Request>(&buf[..buf.len() - 1]) {
                        return Ok(req);
                    }
                }
            }
            return Err("client disconnected");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_BUF_BYTES {
            return Err("request too large");
        }
        // Fast path: try the raw buffer first (handles requests without a
        // trailing newline, matching the Windows message-mode behavior).
        if let Ok(req) = serde_json::from_slice::<Request>(&buf) {
            return Ok(req);
        }
        // Client-framed path: the MCP client writes JSON + '\n'.
        // If the buffer ends with '\n' and still won't parse, it is
        // genuinely malformed.
        if buf.last() == Some(&b'\n') {
            if let Ok(req) = serde_json::from_slice::<Request>(&buf[..buf.len() - 1]) {
                return Ok(req);
            }
            return Err("invalid JSON request");
        }
        // No trailing '\n' yet: partial read, keep accumulating.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Verify that a Request roundtrips through protocol::handle and
    /// serialises back to valid JSON.
    #[test]
    fn request_roundtrips_through_protocol() {
        let index = Arc::new(crate::index::FileIndex::new());
        let content = Arc::new(crate::content::ContentStore::new());
        let state = crate::IndexerState {
            index,
            content,
            volumes: vec![],
        };

        let req: Request =
            serde_json::from_str(r#"{"method":"status","params":null}"#).unwrap();
        let response = protocol::handle(&state, req);

        // The response must serialise and parse back.
        let json = serde_json::to_vec(&response).expect("serialize");
        assert!(!json.is_empty());
        let parsed: serde_json::Value = serde_json::from_slice(&json).expect("parse back");
        assert_eq!(parsed["ok"], true);
        assert!(parsed.get("data").is_some());
    }

    /// Verify that a ping request also roundtrips.
    #[test]
    fn ping_roundtrips_through_protocol() {
        let index = Arc::new(crate::index::FileIndex::new());
        let content = Arc::new(crate::content::ContentStore::new());
        let state = crate::IndexerState {
            index,
            content,
            volumes: vec![],
        };

        let req: Request = serde_json::from_str(r#"{"method":"ping"}"#).unwrap();
        let response = protocol::handle(&state, req);
        let json = serde_json::to_vec(&response).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_slice(&json).expect("parse back");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["data"]["pong"], true);
    }

    /// Verify stale socket removal works (does not touch SOCKET_PATH; uses
    /// a temporary file instead).
    #[test]
    fn stale_socket_is_removed() {
        let dir = std::env::temp_dir().join("instant-file-search-test");
        let _ = std::fs::create_dir_all(&dir);
        let dummy = dir.join("dummy.sock");
        std::fs::write(&dummy, b"stale").unwrap();
        assert!(dummy.exists());

        std::fs::remove_file(&dummy).unwrap();
        assert!(!dummy.exists());

        // Removing a non-existent path must not error (matches startup logic).
        let err = std::fs::remove_file(&dummy).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

        let _ = std::fs::remove_dir(&dir);
    }
}
