//! Startup scan progress.
//!
//! The pipe server only starts after the initial walk, so during startup there
//! is normally nothing to query: `search_status` just reports that the indexer
//! is not accepting connections. This module publishes progress three ways so
//! an operator or an agent can tell where the walk is:
//!
//!   * INFO log lines: `scanning <volume>` when a volume starts, a heartbeat at
//!     most every 5 s (`scanning /mnt/...: 412,000 entries (412,202 total)`),
//!     and the existing `scanned <volume>: N entries` line when it finishes;
//!   * `sd_notify(STATUS=...)` so `systemctl status` shows the same text;
//!   * a world-readable JSON file (`/run/instant-file-search/scan.json`,
//!     rewritten atomically every 2 s) that the MCP server includes in
//!     `search_status` as `scan`, and that an agent can read directly.
//!
//! It is Linux-only because only the Linux initial scan streams; the file lives
//! in the unit's `RuntimeDirectory`.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Default status file location. `INSTANT_FS_SCAN_STATUS_PATH` overrides it
/// (also honoured by the MCP reader in `src/native.rs`, which is useful for
/// tests and non-service runs).
pub const STATUS_PATH: &str = "/run/instant-file-search/scan.json";

/// How often the JSON file is rewritten while entries are arriving.
const FILE_INTERVAL: Duration = Duration::from_secs(2);
/// Minimum gap between heartbeat log lines / systemd status messages.
const LOG_INTERVAL: Duration = Duration::from_secs(5);
/// Also log after this many entries in one volume, whichever comes first.
const LOG_ENTRIES: u64 = 100_000;

#[derive(Debug, Clone, Serialize)]
pub struct ScanStatus {
    /// `scanning`, `ready`, or `failed`.
    pub state: &'static str,
    pub pid: u32,
    pub volumes: Vec<String>,
    pub volume: Option<String>,
    pub volume_index: usize,
    pub entries: u64,
    pub entries_in_volume: u64,
    pub started_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
}

pub struct ScanProgress {
    path: PathBuf,
    status: ScanStatus,
    last_file: Instant,
    last_log: Instant,
    last_log_entries: u64,
}

impl ScanProgress {
    pub fn new(volumes: &[String]) -> Self {
        let now = unix_now();
        let status = ScanStatus {
            state: "scanning",
            pid: std::process::id(),
            volumes: volumes.to_vec(),
            volume: None,
            volume_index: 0,
            entries: 0,
            entries_in_volume: 0,
            started_at: now,
            updated_at: now,
            finished_at: None,
        };
        let mut progress = Self {
            path: status_path(),
            status,
            // Force the first write/log immediately.
            last_file: Instant::now() - FILE_INTERVAL,
            last_log: Instant::now() - LOG_INTERVAL,
            last_log_entries: 0,
        };
        progress.publish("starting initial scan");
        progress
    }

    pub fn begin_volume(&mut self, volume_index: usize, volume: &str) {
        self.status.volume_index = volume_index;
        self.status.volume = Some(volume.to_string());
        self.status.entries_in_volume = 0;
        let message = format!(
            "scanning {volume} (volume {}/{})",
            volume_index + 1,
            self.status.volumes.len()
        );
        self.publish(&message);
    }

    /// Called for every walked entry. Cheap: the file and the log are throttled.
    pub fn tick(&mut self, volume: &str, entries_in_volume: u64, entries: u64) {
        self.status.entries_in_volume = entries_in_volume;
        self.status.entries = entries;
        if self.last_file.elapsed() >= FILE_INTERVAL {
            self.write_file();
        }
        if self.last_log.elapsed() >= LOG_INTERVAL
            || entries.saturating_sub(self.last_log_entries) >= LOG_ENTRIES
        {
            let message = format!(
                "scanning {volume}: {entries_in_volume} entries ({entries} total)"
            );
            self.publish(&message);
        }
    }

    pub fn finish_volume(&mut self, volume: &str, volume_entries: u64, entries: u64) {
        self.status.entries_in_volume = volume_entries;
        self.status.entries = entries;
        let message = format!("scanned {volume}: {volume_entries} entries ({entries} total)");
        self.publish(&message);
    }

    pub fn finished(&mut self, entries: u64) {
        self.status.state = "ready";
        self.status.entries = entries;
        self.status.volume = None;
        self.status.entries_in_volume = 0;
        self.status.finished_at = Some(unix_now());
        self.publish(&format!("ready: {entries} entries indexed"));
    }

    pub fn failed(&mut self, error: &str) {
        self.status.state = "failed";
        self.status.finished_at = Some(unix_now());
        self.publish(&format!("scan failed: {error}"));
    }

    fn publish(&mut self, message: &str) {
        tracing::info!("{message}");
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Status(message)]);
        self.last_log = Instant::now();
        self.last_log_entries = self.status.entries;
        self.write_file();
    }

    fn write_file(&mut self) {
        self.status.updated_at = unix_now();
        let Ok(json) = serde_json::to_vec(&self.status) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Write-then-rename so a reader never sees a half-written file. Errors
        // are ignored on purpose: progress reporting must never fail the scan.
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, &json).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
        self.last_file = Instant::now();
    }
}

fn status_path() -> PathBuf {
    if let Some(path) = std::env::var_os("INSTANT_FS_SCAN_STATUS_PATH") {
        return PathBuf::from(path);
    }
    PathBuf::from(STATUS_PATH)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_the_fields_an_agent_needs() {
        let status = ScanStatus {
            state: "scanning",
            pid: 42,
            volumes: vec!["/".to_string(), "/mnt/2tb-drive".to_string()],
            volume: Some("/mnt/2tb-drive".to_string()),
            volume_index: 1,
            entries: 500,
            entries_in_volume: 300,
            started_at: 1_700_000_000,
            updated_at: 1_700_000_042,
            finished_at: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "scanning");
        assert_eq!(json["volume"], "/mnt/2tb-drive");
        assert_eq!(json["volume_index"], 1);
        assert_eq!(json["entries"], 500);
        assert_eq!(json["entries_in_volume"], 300);
        assert_eq!(json["volumes"].as_array().map(Vec::len), Some(2));
        assert!(json["finished_at"].is_null());
    }

    #[test]
    fn progress_never_panics_when_the_status_path_is_unwritable() {
        // /proc is read-only for unprivileged test runs; the progress writer
        // must treat that as a no-op rather than failing the scan.
        let path = PathBuf::from("/proc/instant-file-search-scan-test.json");
        let mut progress = ScanProgress {
            path,
            status: ScanStatus {
                state: "scanning",
                pid: std::process::id(),
                volumes: vec!["/".to_string()],
                volume: None,
                volume_index: 0,
                entries: 0,
                entries_in_volume: 0,
                started_at: 0,
                updated_at: 0,
                finished_at: None,
            },
            last_file: Instant::now(),
            last_log: Instant::now(),
            last_log_entries: 0,
        };
        progress.begin_volume(0, "/");
        progress.tick("/", 1, 1);
        progress.finished(1);
        progress.failed("boom");
    }
}
