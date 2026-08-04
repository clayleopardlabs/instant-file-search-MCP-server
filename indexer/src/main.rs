//! instant-file-search-indexer
//!
//! Native NTFS indexer for the instant-file-search MCP server.
//!
//! Modes:
//!   serve    — console mode: build the index, then serve queries + USN
//!              updates over \\.\pipe\instant-file-search-indexer (default)
//!   service  — Windows service mode (registers with SCM via `sc create`)
//!   scan     — one-shot diagnostic scan, print stats, exit
//!   help     — usage

mod content;
mod index;
mod platform;
mod protocol;
mod query;
mod scan;
mod types;

#[cfg(windows)]
mod mft;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod sector_reader;
#[cfg(windows)]
mod usn;

#[cfg(target_os = "linux")]
mod fanotify;
#[cfg(target_os = "linux")]
mod pipe_unix;
#[cfg(target_os = "linux")]
mod walk;

use std::sync::Arc;

use anyhow::{Context, Result};
#[cfg(windows)]
use windows_service::service_dispatcher;

use content::ContentStore;
use index::FileIndex;

/// Shared state handed to the pipe server.
#[derive(Clone)]
pub struct IndexerState {
    pub index: Arc<FileIndex>,
    pub content: Arc<ContentStore>,
    pub volumes: Vec<String>,
}

const SERVICE_NAME: &str = "instant-file-search-indexer";

fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());
    match mode.as_str() {
        "scan" => {
            let out = scan::scan_all_volumes()?;
            print!("{out}");
            Ok(())
        }
        #[cfg(windows)]
        "service" => {
            service_dispatcher::start(SERVICE_NAME, service_main)
                .context("failed to start service dispatcher")
        }
        "serve" => serve(),
        _ => {
            eprintln!("usage: instant-file-search-indexer [serve|service|scan]");
            Ok(())
        }
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
}

/// SCM entry point for `service` mode. Runs serve() on a worker thread so
/// the service thread can process SCM control requests.
#[cfg(windows)]
extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_service::service::{ServiceControl, ServiceControlAccept, ServiceState, ServiceStatus, ServiceType};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let stop_requested = Arc::new(AtomicBool::new(false));
    let stop_flag = stop_requested.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop_flag.store(true, Ordering::SeqCst);
                pipe::poke_stop();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = match service_control_handler::register(SERVICE_NAME, event_handler) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("service control handler registration failed: {e}");
            return;
        }
    };

    let running_status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: windows_service::service::ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::from_secs(5),
        process_id: None,
    };
    let _ = status_handle.set_service_status(running_status.clone());

    let worker_stop = stop_requested.clone();
    let worker = std::thread::spawn(move || serve_with_stop(worker_stop));

    // The indexer has no graceful stop hook yet; report stopped as soon as
    // the worker exits so SCM doesn't wait on the service timeout.
    if let Err(e) = worker.join() {
        eprintln!("service worker panicked: {e:?}");
    }
    let _ = status_handle.set_service_status(ServiceStatus {
        current_state: ServiceState::Stopped,
        ..running_status
    });
    let _ = stop_requested;
}

fn serve() -> Result<()> {
    serve_with_stop(Arc::new(std::sync::atomic::AtomicBool::new(false)))
}

fn serve_with_stop(stop: Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
    init_tracing();

    let volumes = platform::discover_volumes();
    if volumes.is_empty() {
        anyhow::bail!("no NTFS fixed volumes found");
    }
    tracing::info!("volumes: {}", volumes.join(", "));

    let index = Arc::new(FileIndex::new());

    // Capture journal tails BEFORE the scan: the watcher starts from here,
    // covering changes made during the scan window without replaying the
    // entire journal history (the scan already snapshots current state).
    let tails = platform::journal_tails(&volumes);
    for (v, id, usn) in &tails {
        tracing::info!("USN tail on {}: id={id} next={usn}", v);
    }

    // Initial scan.
    let n = scan::build_index(&volumes, &index).context("initial scan")?;
    tracing::info!("initial index built: {n} entries");

    let content = Arc::new(ContentStore::new());
    let state = IndexerState { index: index.clone(), content: content.clone(), volumes: volumes.clone() };

    // Background content-indexing pass. Snapshot eligible (path, size) pairs
    // quickly under the lock, then read files OUTSIDE the lock so queries are
    // never blocked during the content build.
    {
        let fill_index = index.clone();
        let fill_content = content.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut candidates = Vec::new();
            fill_index.with_entries(|entries| {
                for (path, entry) in entries {
                    if entry.is_dir {
                        continue;
                    }
                    if ContentStore::should_index(path, entry.size) {
                        candidates.push((path.clone(), entry.size));
                    }
                }
            });
            let mut count = 0usize;
            for (path, _size) in candidates {
                if fill_content.total_bytes() >= content::TOTAL_BUDGET {
                    break;
                }
                match std::fs::read(&path) {
                    Ok(data) => {
                        let keep = data.len().min(content::MAX_FILE_BYTES as usize);
                        fill_content.insert(&path, &data[..keep]);
                        count += 1;
                    }
                    Err(_) => {}
                }
            }
            tracing::info!(
                "content index built: {count} files, {} bytes, {}ms",
                fill_content.total_bytes(),
                started.elapsed().as_millis()
            );
        });
    }

    // USN watcher thread.
    let watch_index = index.clone();
    let watch_content = content.clone();
    let watch_volumes = volumes.clone();
    std::thread::spawn(move || {
        if let Err(e) = platform::watch_all(&watch_volumes, &watch_index, &watch_content, &tails) {
            tracing::error!("USN watcher exited: {e:#}");
        }
    });

    let server = platform::PipeServer::with_stop(state, stop)?;
    let result = server.run();

    // Tell systemd the service is shutting down (Type=notify). No-op when
    // NOTIFY_SOCKET is unset.
    #[cfg(target_os = "linux")]
    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);

    result
}
