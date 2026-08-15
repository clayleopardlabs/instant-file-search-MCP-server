//! Repeatable, process-local measurements for the metadata backends.
//!
//! The benchmark is deliberately synthetic and read-only with respect to the
//! user's configured index. It creates a temporary database for disk mode and
//! removes it when the run finishes. Use the live MCP tools for measurements of
//! a real installed service.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::index::FileIndex;
use crate::query::QueryOptions;
use crate::types::IndexedFile;

const DEFAULT_ENTRIES: usize = 250_000;
const DEFAULT_RUNS: usize = 5;

pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    let first = args.next().unwrap_or_else(|| "memory".to_string());
    let (mode, entries, runs, json) = if first == "synthetic" {
        parse_named(args)?
    } else {
        let entries = if first.eq_ignore_ascii_case("memory") || first.eq_ignore_ascii_case("disk")
        {
            DEFAULT_ENTRIES
        } else {
            first
                .parse::<usize>()
                .context("benchmark mode must be memory, disk, or synthetic")?
        };
        let mode = if first.eq_ignore_ascii_case("memory") || first.eq_ignore_ascii_case("disk") {
            first
        } else {
            "memory".to_string()
        };
        let entries = if mode == "memory" || mode == "disk" {
            args.next()
                .as_deref()
                .map(str::parse)
                .transpose()
                .context("benchmark entry count must be a positive integer")?
                .unwrap_or(entries)
        } else {
            entries
        };
        (mode, entries, 1, true)
    };

    if entries == 0 {
        anyhow::bail!("benchmark entry count must be greater than zero");
    }
    if runs == 0 {
        anyhow::bail!("benchmark run count must be greater than zero");
    }
    if !mode.eq_ignore_ascii_case("memory") && !mode.eq_ignore_ascii_case("disk") {
        anyhow::bail!("benchmark mode must be memory or disk, got {mode:?}");
    }

    let db_path = temporary_database_path();
    let start_io = io_snapshot();
    let build_started = Instant::now();
    let index = FileIndex::for_benchmark(&mode, db_path.clone())?;
    let files = synthetic_entries(entries);
    let update_sample = files
        .get(entries / 2)
        .cloned()
        .context("synthetic benchmark corpus is empty")?;
    index.replace(files);
    let build_ms = build_started.elapsed().as_secs_f64() * 1_000.0;
    std::thread::sleep(Duration::from_millis(250));
    let rss_bytes_after_build = resident_bytes();

    let update_started = Instant::now();
    let mut updated = update_sample;
    updated.size = updated.size.saturating_add(1);
    index.upsert(updated);
    let update_ms = update_started.elapsed().as_secs_f64() * 1_000.0;

    let (reopen_ms, queries) = if mode.eq_ignore_ascii_case("disk") {
        let reopen_started = Instant::now();
        drop(index);
        let reopened = FileIndex::for_benchmark(&mode, db_path.clone())?;
        let elapsed = reopen_started.elapsed().as_secs_f64() * 1_000.0;
        (elapsed, measure_queries(&reopened, runs))
    } else {
        (0.0, measure_queries(&index, runs))
    };

    let end_io = io_snapshot();
    let database_bytes = database_bytes(&db_path);

    let result = serde_json::json!({
        "schema_version": 1,
        "mode": mode,
        "entries": entries,
        "runs": runs,
        "build_ms": build_ms,
        "reopen_ms": reopen_ms,
        "update_ms": update_ms,
        "rss_bytes_after_build": rss_bytes_after_build,
        "database_bytes": database_bytes,
        "queries_ms": queries.iter().map(|x| x.first_ms).collect::<Vec<_>>(),
        "query_results": queries,
        "io": {
            "read_bytes": end_io.read_bytes.saturating_sub(start_io.read_bytes),
            "write_bytes": end_io.write_bytes.saturating_sub(start_io.write_bytes),
            "available": start_io.available && end_io.available,
        },
        "content_cache_included": false,
        "measurement": "synthetic corpus in a separate release-build process; RSS is measured after the build input is dropped",
    });
    if json {
        println!("{result}");
    } else {
        println!("{result}");
    }

    remove_database(&db_path);
    Ok(())
}

fn parse_named(mut args: impl Iterator<Item = String>) -> Result<(String, usize, usize, bool)> {
    let mut mode = "memory".to_string();
    let mut entries = DEFAULT_ENTRIES;
    let mut runs = DEFAULT_RUNS;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => mode = args.next().context("--mode needs memory or disk")?,
            "--entries" => {
                entries = args
                    .next()
                    .context("--entries needs a number")?
                    .parse()
                    .context("--entries must be a positive integer")?;
            }
            "--runs" => {
                runs = args
                    .next()
                    .context("--runs needs a number")?
                    .parse()
                    .context("--runs must be a positive integer")?;
            }
            "--json" => json = true,
            other => anyhow::bail!("unknown benchmark option {other:?}"),
        }
    }
    Ok((mode, entries, runs, json))
}

#[derive(serde::Serialize)]
struct QueryMeasurement {
    query: String,
    first_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    matched: usize,
}

fn measure_queries(index: &FileIndex, runs: usize) -> Vec<QueryMeasurement> {
    benchmark_queries()
        .iter()
        .map(|query| {
            let opts = QueryOptions {
                query: query.to_string(),
                max_results: 100,
                ..Default::default()
            };
            let mut times = Vec::with_capacity(runs);
            let mut matched = 0;
            for _ in 0..runs {
                let started = Instant::now();
                let result = index.search(&opts);
                times.push(started.elapsed().as_secs_f64() * 1_000.0);
                matched = result.total;
            }
            let first_ms = times[0];
            let p50_ms = percentile(&mut times.clone(), 0.50);
            let p95_ms = percentile(&mut times, 0.95);
            QueryMeasurement {
                query: (*query).to_string(),
                first_ms,
                p50_ms,
                p95_ms,
                matched,
            }
        })
        .collect()
}

fn percentile(values: &mut [f64], percentile: f64) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    let index = ((values.len().saturating_sub(1)) as f64 * percentile).round() as usize;
    values[index.min(values.len().saturating_sub(1))]
}

fn benchmark_queries() -> [&'static str; 6] {
    [
        "file-00000123",
        "*.rs",
        "module-0042",
        "size:>512kb",
        "file:*.json",
        "folder:*",
    ]
}

fn synthetic_entries(entries: usize) -> Vec<IndexedFile> {
    let extensions = ["rs", "txt", "json", "log", "md"];
    (0..entries)
        .map(|n| {
            let is_dir = n % 97 == 0;
            let extension = extensions[n % extensions.len()];
            let root = if n % 17 == 0 { "node_modules" } else { "src" };
            let name = if n % 101 == 0 {
                format!("café-{:08}.{}", n, extension)
            } else {
                format!("file-{:08}.{}", n, extension)
            };
            let path = if is_dir {
                format!(
                    r"C:\benchmark\workspace-{:03}\{}\module-{:04}",
                    n % 50,
                    root,
                    n % 10_000
                )
            } else {
                format!(
                    r"C:\benchmark\workspace-{:03}\{}\module-{:04}\{}",
                    n % 50,
                    root,
                    n % 10_000,
                    name
                )
            };
            let mut entry = IndexedFile::new(
                path,
                (n as u64 * 131) % 1_048_576,
                n as i64,
                n as i64,
                n as i64,
                is_dir,
                n as u64,
            );
            entry.attributes = if n % 19 == 0 { 0x2 } else { 0x80 };
            entry
        })
        .collect()
}

fn temporary_database_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "instant-file-search-benchmark-{}-{}.sqlite3",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

fn database_bytes(path: &PathBuf) -> u64 {
    [
        path.clone(),
        path.with_extension("sqlite3-wal"),
        path.with_extension("sqlite3-shm"),
    ]
    .iter()
    .filter_map(|p| std::fs::metadata(p).ok())
    .map(|m| m.len())
    .sum()
}

fn remove_database(path: &PathBuf) {
    for candidate in [
        path.clone(),
        path.with_extension("sqlite3-wal"),
        path.with_extension("sqlite3-shm"),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}

#[derive(Default, Clone, Copy)]
struct IoSnapshot {
    read_bytes: u64,
    write_bytes: u64,
    available: bool,
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

#[cfg(target_os = "linux")]
fn resident_bytes() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    pages.saturating_mul(unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 })
}

#[cfg(target_os = "macos")]
fn resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } == 0 {
        let usage = unsafe { usage.assume_init() };
        usage.ru_maxrss as u64
    } else {
        0
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn resident_bytes() -> u64 {
    0
}

#[cfg(windows)]
fn io_snapshot() -> IoSnapshot {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessIoCounters, IO_COUNTERS};
    let mut counters = IO_COUNTERS::default();
    if unsafe { GetProcessIoCounters(GetCurrentProcess(), &mut counters) }.is_ok() {
        IoSnapshot {
            read_bytes: counters.ReadTransferCount,
            write_bytes: counters.WriteTransferCount,
            available: true,
        }
    } else {
        IoSnapshot::default()
    }
}

#[cfg(target_os = "linux")]
fn io_snapshot() -> IoSnapshot {
    let Ok(text) = std::fs::read_to_string("/proc/self/io") else {
        return IoSnapshot::default();
    };
    let mut out = IoSnapshot {
        available: true,
        ..Default::default()
    };
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("read_bytes:") => {
                out.read_bytes = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0)
            }
            Some("write_bytes:") => {
                out.write_bytes = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0)
            }
            _ => {}
        }
    }
    out
}

#[cfg(not(any(windows, target_os = "linux")))]
fn io_snapshot() -> IoSnapshot {
    IoSnapshot::default()
}
