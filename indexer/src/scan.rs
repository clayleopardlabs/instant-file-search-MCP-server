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
///
/// Collects every volume's entries before inserting, which keeps the index
/// swap atomic (used by the fanotify queue-overflow recovery while queries are
/// being served). The initial scan uses [`build_index_streaming`] instead so a
/// full filesystem walk does not have to materialize the corpus in RAM.
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

/// Build the index by streaming every entry straight into the disk backend.
///
/// Low-memory initial scan used by `serve`: entries are inserted as they are
/// discovered instead of being collected into a corpus-sized `Vec`, which
/// measured ~700 MB RSS for 1.6 M entries. Call this only before the pipe
/// server starts serving, because it holds the SQLite write transaction for
/// the whole walk.
///
/// The watcher's queue-overflow recovery deliberately keeps using
/// [`build_index`], which swaps the index atomically while queries are being
/// served.
#[cfg(target_os = "linux")]
pub fn build_index_streaming(volumes: &[String], index: &SharedIndex) -> Result<usize> {
    use crate::platform::scan_volume_into;

    Ok(index.replace_streaming(|sink| {
        for v in volumes {
            match scan_volume_into(v, &mut *sink) {
                Ok(count) => tracing::info!("scanned {}: {} entries", v, count),
                Err(e) => tracing::warn!("skipping {}: {e:#}", v),
            }
        }
        Ok(())
    }))
}
