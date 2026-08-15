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
mod disk;
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
#[cfg(target_os = "macos")]
mod fsevents;
#[cfg(not(windows))]
mod pipe_unix;
#[cfg(target_os = "linux")]
mod walk;
#[cfg(target_os = "macos")]
mod walk_macos;

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
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_COMMIT: &str = match option_env!("GIT_COMMIT") {
    Some(value) => value,
    None => "dev",
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "serve".to_string());
    match mode.as_str() {
        "--version" | "-V" => {
            println!("instant-file-search-indexer {APP_VERSION} {BUILD_COMMIT}");
            Ok(())
        }
        "scan" => {
            let out = scan::scan_all_volumes()?;
            print!("{out}");
            Ok(())
        }
        "benchmark" => benchmark(args),
        #[cfg(windows)]
        "service" => service_dispatcher::start(SERVICE_NAME, service_main)
            .context("failed to start service dispatcher"),
        "serve" => serve(),
        _ => {
            eprintln!("usage: instant-file-search-indexer [serve|service|scan|benchmark]");
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

    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceState, ServiceStatus, ServiceType,
    };
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
        anyhow::bail!("no indexable filesystem volumes found");
    }
    tracing::info!("volumes: {}", volumes.join(", "));

    let index = Arc::new(FileIndex::from_env().context("configure index storage")?);
    tracing::info!(
        "index storage mode: {}{}",
        index.storage_mode(),
        index
            .disk_path()
            .map(|p| format!(" ({})", p.display()))
            .unwrap_or_default()
    );

    // Capture a current tail before scanning. A disk index may instead resume
    // from a durable older tail and replay the journal entries since then.
    let live_tails = platform::journal_tails(&volumes);
    let tails = match index.checkpoints() {
        Some(saved) if checkpoints_can_resume(&volumes, &saved, &live_tails) => {
            tracing::info!(
                "resuming durable disk index from {} watcher checkpoints",
                saved.len()
            );
            saved
        }
        _ => {
            let n = scan::build_index(&volumes, &index).context("initial scan")?;
            tracing::info!("initial index built: {n} entries");
            index.replace_checkpoints(&live_tails);
            live_tails
        }
    };
    for (v, id, cursor) in &tails {
        tracing::info!("watcher checkpoint on {}: id={id} cursor={cursor}", v);
    }

    // Disk mode's contract is low, predictable RAM use. Content indexing is
    // intentionally unavailable there: building it would require enumerating
    // the full database and could reserve another 256 MiB.
    let content = Arc::new(if index.storage_mode() == "memory" && content_enabled() {
        ContentStore::new()
    } else {
        ContentStore::disabled()
    });
    let state = IndexerState {
        index: index.clone(),
        content: content.clone(),
        volumes: volumes.clone(),
    };

    // Background content-indexing pass. Snapshot eligible (path, size) pairs
    // quickly under the lock, then read files OUTSIDE the lock so queries are
    // never blocked during the content build.
    if index.storage_mode() == "memory" && content.is_enabled() {
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

/// Compare the index metadata backends without accessing the live filesystem.
/// It is intentionally a separate process per mode so allocator retention in
/// the memory run cannot inflate the disk run's RSS.
fn benchmark(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mode = args.next().unwrap_or_else(|| "memory".to_string());
    let entries: usize = args
        .next()
        .as_deref()
        .unwrap_or("250000")
        .parse()
        .context("benchmark entry count must be a positive integer")?;
    if entries == 0 {
        anyhow::bail!("benchmark entry count must be greater than zero");
    }
    let db_path = std::env::temp_dir().join(format!(
        "instant-file-search-benchmark-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let index = FileIndex::for_benchmark(&mode, db_path.clone())?;
    let started = std::time::Instant::now();
    let mut files = Vec::with_capacity(entries);
    for n in 0..entries {
        let mut entry = types::IndexedFile::new(
            format!(
                r"C:\benchmark\workspace-{:03}\src\module-{:04}\file-{:08}.rs",
                n % 1000,
                n % 10_000,
                n
            ),
            (n % 1_048_576) as u64,
            n as i64,
            n as i64,
            n as i64,
            false,
            n as u64,
        );
        entry.attributes = 0x80;
        files.push(entry);
    }
    index.replace(files);
    let build_ms = started.elapsed().as_secs_f64() * 1_000.0;
    // Let temporary scan input be dropped and report the settled resident set.
    std::thread::sleep(std::time::Duration::from_millis(250));
    let rss_bytes = resident_bytes();

    let queries = ["file-00000123", "*.rs", "module-0042"];
    let mut query_ms = Vec::new();
    let mut matched = 0usize;
    for query in queries {
        let opts = query::QueryOptions {
            query: query.to_string(),
            max_results: 100,
            ..Default::default()
        };
        let q_started = std::time::Instant::now();
        let result = index.search(&opts);
        query_ms.push(q_started.elapsed().as_secs_f64() * 1_000.0);
        matched = matched.saturating_add(result.total);
    }
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(db_path.with_extension("sqlite3-shm"));
    println!(
        "{}",
        serde_json::json!({
            "mode": mode,
            "entries": entries,
            "build_ms": build_ms,
            "rss_bytes_after_build": rss_bytes,
            "queries_ms": query_ms,
            "matched_total": matched,
            "content_cache_included": false,
        })
    );
    Ok(())
}

#[cfg(windows)]
fn resident_bytes() -> u64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    if unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            counters.cb,
        )
    }
    .is_ok()
    {
        counters.WorkingSetSize as u64
    } else {
        0
    }
}

#[cfg(not(windows))]
fn resident_bytes() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::checkpoints_can_resume;

    #[cfg(windows)]
    #[test]
    fn windows_resume_requires_the_same_usn_journal() {
        let volumes = vec!["C:\\".to_string()];
        assert!(checkpoints_can_resume(
            &volumes,
            &[("C:\\".into(), 7, 100)],
            &[("C:\\".into(), 7, 200)],
        ));
        assert!(!checkpoints_can_resume(
            &volumes,
            &[("C:\\".into(), 7, 100)],
            &[("C:\\".into(), 8, 200)],
        ));
        assert!(!checkpoints_can_resume(
            &volumes,
            &[("C:\\".into(), 7, 300)],
            &[("C:\\".into(), 7, 200)],
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_resume_accepts_an_older_fsevents_id() {
        let volumes = vec!["/".to_string()];
        assert!(checkpoints_can_resume(
            &volumes,
            &[("/".into(), 100, 0)],
            &[("/".into(), 200, 0)],
        ));
    }
}

/// The optional 256 MiB content cache can be disabled in memory mode.
fn content_enabled() -> bool {
    match std::env::var("INSTANT_FS_CONTENT_INDEX") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "on"),
        Err(_) => true,
    }
}

/// Return true only when every indexed volume has a checkpoint the platform
/// can replay. Windows validates the USN journal ID. macOS event IDs are
/// monotonic; FSEvents itself reports a wrapped or unavailable history and its
/// watcher then performs the authoritative full re-scan.
fn checkpoints_can_resume(
    volumes: &[String],
    saved: &[(String, u64, i64)],
    live: &[(String, u64, i64)],
) -> bool {
    if saved.len() != volumes.len() || live.len() != volumes.len() {
        return false;
    }
    volumes.iter().all(|volume| {
        let Some((_, saved_id, saved_cursor)) = saved.iter().find(|(v, _, _)| v == volume) else {
            return false;
        };
        let Some((_, live_id, live_cursor)) = live.iter().find(|(v, _, _)| v == volume) else {
            return false;
        };
        #[cfg(windows)]
        {
            saved_id == live_id && saved_cursor <= live_cursor
        }
        #[cfg(target_os = "macos")]
        {
            saved_id <= live_id
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (saved_id, saved_cursor, live_id, live_cursor);
            false
        }
    })
}
