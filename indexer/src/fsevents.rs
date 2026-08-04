//! macOS change tracking via FSEvents (stub).
//!
//! Phase 0b: compiles on the macOS target. Real implementation (Phase 4) uses
//! per-volume FSEventStreams with `kFSEventStreamCreateFlagFileEvents` +
//! `kFSEventStreamCreateFlagUseExtendedData`, `since=` replay via
//! `FSEventStreamEventId`, and a user-space ring buffer + append-only journal
//! for timestamps and restart persistence. See `docs/macos-port-plan.md`.

use anyhow::{bail, Result};
use std::sync::Arc;

use crate::content::ContentStore;
use crate::index::FileIndex;

/// macOS has no persistent per-file journal; return an empty tail list.
pub fn journal_tails(_volumes: &[String]) -> Vec<(String, u64, i64)> {
    Vec::new()
}

/// Watch all volumes for changes via FSEvents, applying events to the index
/// and content store until the process exits. Stub: not yet implemented.
pub fn watch_all(
    _volumes: &[String],
    _index: &Arc<FileIndex>,
    _content: &Arc<ContentStore>,
    _tails: &[(String, u64, i64)],
) -> Result<()> {
    bail!("macOS FSEvents watcher not yet implemented (Phase 4)")
}