//! Scan orchestration: build the initial index from all discovered volumes.

use anyhow::Result;

use crate::index::SharedIndex;
use crate::platform::{discover_volumes, scan_volume};

/// One-shot scan of all volumes, printing stats (used by `scan` mode).
pub fn scan_all_volumes() -> Result<String> {
    let volumes = discover_volumes();
    let mut out = String::new();
    for v in &volumes {
        match scan_volume(v) {
            Ok(entries) => {
                out.push_str(&format!("{}: {} entries\n", v, entries.len()));
            }
            Err(e) => {
                out.push_str(&format!("{}: ERROR: {e:#}\n", v));
            }
        }
    }
    Ok(out)
}

/// Build the index into `index` from all discovered volumes.
pub fn build_index(volumes: &[String], index: &SharedIndex) -> Result<usize> {
    let mut all = Vec::new();
    for v in volumes {
        match scan_volume(v) {
            Ok(entries) => {
                tracing::info!("scanned {}: {} entries", v, entries.len());
                all.extend(entries);
            }
            Err(e) => {
                tracing::warn!("skipping {}: {e:#}", v);
            }
        }
    }
    let n = index.replace(all);
    Ok(n)
}
