//! instant-file-search-indexer
//!
//! Native NTFS indexer for the instant-file-search MCP server.
//!
//! Modes:
//!   serve   — build the index, then serve queries + USN updates over
//!             \\.\pipe\instant-file-search-indexer (default)
//!   scan    — one-shot diagnostic scan, print stats, exit
//!   help    — usage

mod index;
mod mft;
mod pipe;
mod query;
mod scan;
mod sector_reader;
mod usn;

use std::sync::Arc;

use anyhow::{Context, Result};

use index::FileIndex;

/// Shared state handed to the pipe server.
#[derive(Clone)]
pub struct IndexerState {
    pub index: Arc<FileIndex>,
    pub volumes: Vec<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());
    match mode.as_str() {
        "scan" => {
            let out = scan::scan_all_volumes()?;
            print!("{out}");
            Ok(())
        }
        "serve" => serve(),
        _ => {
            eprintln!("usage: instant-file-search-indexer [serve|scan]");
            Ok(())
        }
    }
}

fn serve() -> Result<()> {
    let volumes = mft::discover_ntfs_volumes();
    if volumes.is_empty() {
        anyhow::bail!("no NTFS fixed volumes found");
    }
    tracing::info!("volumes: {}", volumes.join(", "));

    let index = Arc::new(FileIndex::new());

    // Initial scan.
    let n = scan::build_index(&volumes, &index).context("initial scan")?;
    tracing::info!("initial index built: {n} entries");

    let state = IndexerState { index: index.clone(), volumes: volumes.clone() };

    // USN watcher thread.
    let watch_index = index.clone();
    let watch_volumes = volumes.clone();
    std::thread::spawn(move || {
        if let Err(e) = usn::watch_all(&watch_volumes, &watch_index) {
            tracing::error!("USN watcher exited: {e:#}");
        }
    });

    let server = pipe::PipeServer::new(state)?;
    server.run()
}
