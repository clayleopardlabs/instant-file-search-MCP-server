//! macOS enumeration backend (stub).
//!
//! Phase 0 placeholder: compiles so the workspace builds for
//! `aarch64-apple-darwin`. Real implementation (Phase 2b) uses
//! `getattrlistbulk` with `FSOPT_RETURN_REALDEV` for batched statx-equivalent
//! enumeration, mount discovery via `getmntinfo`, and `(real devid, fileid)`
//! index keys. See `docs/macos-port-plan.md`.

use anyhow::{bail, Result};

use crate::types::IndexedFile;

/// Discover indexable volumes (mount points). Stub: not yet implemented.
pub fn discover_volumes() -> Vec<String> {
    Vec::new()
}

/// Scan one volume, returning its files. Stub: not yet implemented.
pub fn scan_volume(_volume: &str) -> Result<Vec<IndexedFile>> {
    bail!("macOS enumeration not yet implemented (Phase 2b)")
}

/// Mount-aware volume/root identifier for a path. Stub: not yet implemented.
pub fn volume_of(_path: &str) -> String {
    String::new()
}