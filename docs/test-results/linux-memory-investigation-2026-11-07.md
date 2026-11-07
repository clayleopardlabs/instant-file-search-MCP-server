# Linux indexer memory investigation — independent verification

Date: 2026-11-07 (America/New_York, UTC-05:00; journal UTC lines below are marked `Z`).
Host: `Omen`, 13th Gen Intel Core i9-13900HX, 30 GiB RAM, Linux 7.0.0-43-generic x86_64, ext4 root on nvme1n1p3, tmpfs `/tmp`.
Binary under test: `/usr/local/lib/instant-file-search/instant-file-search-indexer 1.0.20 dev` (installed 2026-11-06 13:26), built from repo HEAD `1b595b5` (Cargo version 1.0.20; the working tree at verification time had uncommitted 1.0.21 changes from other agents, which were **not** used for any measurement here).
Verifier: teammate `mem-verifier`, task `task-4`.
Post-fix cross-check: `target/release/instant-file-search-indexer 1.0.21 dev` built 2026-11-07 13:59 by the Lead (working tree, uncommitted); §3.5 reports an independent `benchmark stream` measurement from that binary, clearly separated from the deployed 1.0.20 numbers.

Constraints honoured: no Rust source edits, no commits/pushes, no `cargo` invocations, no writes to `/var/lib/instant-file-search`, service not stopped/restarted. Writes only to `scripts/` and `docs/test-results/`. All benchmark databases were created under `mktemp -d` directories.

---

## 0. Verdict summary

| Hypothesis | Verdict | Basis |
|---|---|---|
| **H1** — the ~710 MB is the old Everything fallback engine | **Refuted** | The 710 MB process is the *indexer* (a separate crate with no Everything dependency), not the MCP server. `mod everything` is `#[cfg(windows)]` (`src/main.rs:1-2`); the Linux build exposes no Everything engine and the MCP server reports `native-only`. |
| **H2** — the Windows "stream the index from the hard drive" low-RAM technique was not implemented on Linux | **Refuted as stated, but the underlying concern is confirmed with a different mechanism** | Disk-backed ("RAM saving") mode *is* implemented and active on Linux: `INSTANT_FS_INDEX_MODE=disk`, journal line `index storage mode: disk`, a live 2.688 GB `index.sqlite3`, `search_status` reporting `"storage_mode":"disk"`, and SQLite-streamed queries (`disk.rs` `query_map`, no corpus load). There is also **no** Windows-only low-RAM scan technique to port: the Windows MFT scan also accumulates a `Vec<IndexedFile>` (`mft.rs:117,154`). The real cause is that the **initial scan collects the entire corpus in RAM on every Linux start** (`scan::build_index` → `Vec<IndexedFile>` → `index.replace`), and Linux must full-rescan on every restart because `checkpoints_can_resume()` returns `false` unconditionally on Linux (`indexer/src/main.rs:447-450`). |

Measured cost per indexed entry (this machine, 1.6 M entries, see §4 for arithmetic): **≈452 bytes/entry steady**, **≈760 bytes/entry at the live high-water mark**, vs a hand-computed struct+heap floor of **≈448 bytes/entry**. The ~710 MB is not an Everything artefact and not a "disk mode not working" artefact — it is the pre-insert corpus materialisation.

The historical **"10 MiB RSS in disk mode at 500,000 records" claim is not reproducible on Linux with the current harness**: measured 132.4 MiB after the build input is dropped and 227.8 MiB at peak (§3.3), while the memory-mode half of the same claim roughly reproduces (523.9 vs ~495 MiB). It was likely a Windows working-set/measurement artefact in addition to having been taken "after build" rather than during the initial scan.

The fix was independently cross-checked on the Lead's 1.0.21 release build: the new `benchmark stream 1600000` path peaks at **10,492 kB** vs 727,928 kB for the deployed 1.0.20 disk path, with an identical SQLite database size (§3.5).

Confidence: **high** for H1; **high** for "disk mode is active and streaming on Linux"; **high** for the initial-scan root cause (fixed and re-measured); **medium** for the precise split of the 1.19 GB live peak between scan-realloc and the still-running content-index pass (both are quantified in §6).

---

## 1. Process identification — who owns the ~700 MB

Commands:

```
ps -eo pid,user,rss,vsz,pcpu,etime,args | grep -i instant | grep -v grep
grep -E '^(Name|State|Threads|VmPeak|VmSize|VmHWM|VmRSS|RssAnon|RssFile|RssShmem)' /proc/8592/status
```

Raw output (2026-11-07T14:04:11-05:00, uptime 1:19:05):

```
    PID USER      RSS    VSZ %CPU     ELAPSED COMMAND
   8592 root   710832 842648 98.5   01:19:05 /usr/local/lib/instant-file-search/instant-file-search-indexer serve
```

```
Name:	instant-file-se
State:	S (sleeping)
Threads:	3
VmPeak:	 1428916 kB
VmSize:	  842648 kB
VmHWM:	 1193540 kB      ← 1,222,184,960 B = 1.14 GiB peak
VmRSS:	  710832 kB      ← 727,891,968 B = 694 MiB "steady"
RssAnon:	  702872 kB      ← 98.9 % of RSS is anonymous heap
RssFile:	    7960 kB
RssShmem:	       0 kB
```

Service identity: systemd unit `instant-file-search-indexer.service`, cgroup `/system.slice/instant-file-search-indexer.service`, `ActiveEnterTimestamp=2026-11-07 12:46:22 EST`, `ExecMainStartTimestamp=12:45:05 EST`, `NRestarts=1`. Process has been up 79 min at snapshot time; RSS was stable across the investigation (710,592 kB at 13:49 → 710,832 kB at 14:04, +240 kB/15 min, i.e. no active leak).

**MCP server process check.** The MCP server binary `/usr/local/lib/instant-file-search/instant-file-search-mcp-server` is installed (1.0.20) but **not resident**: `pgrep -af instant-file-search-mcp-server` returns nothing. Do not confuse the two ~710 MB processes on this box — `ps -eo pid,rss,args --sort=-rss` shows:

```
 33196 omen   721964 firefox
  8592 root   710832 /usr/local/lib/instant-file-search/instant-file-search-indexer serve
 33201 omen   709816 node /home/omen/.local/bin/dsh web ...
```

The 709 MB `node` process is the DeepSeek Harness web server, unrelated to Instant File Search. Only PID 8592 is the indexer.

Thread table (`/proc/8592/task/*/stat`, `CLK_TCK=100`), 13:55:04:

| TID | state | utime_s | stime_s | total CPU | share of lifetime |
|---|---|---|---|---|---|
| 8592 (main) | S | 14.8 | 26.4 | 41.2 s | 1 % |
| 8929 | S/R | 987.8 | 2688.7 | 3676.5 s | 89 % |
| 8930 | S/R | 41.7 | 95.5 | 137.2 s (13:55) → 329.8 s (14:01) | 3 % → rising |

(share of the watcher's 4,122 s lifetime at 13:55; by 14:02 thread 8929 had reached 3,912 s and both threads were running ~50/50.)

There are **two** busy threads, not one, and the **whole process is pinned at exactly 100.0 % of one core** (measured repeatedly: 100.0 % over 30 s, 100.0 % over 10 s, 100.0 % over 5 s — see §5). `ps` reports 98.2–98.5 % lifetime average for the same reason.

---

## 2. H1 — the Everything fallback engine

### 2.1 Source gating (repo HEAD `1b595b5`)

`src/main.rs`:

```
1: #[cfg(windows)]
2: mod everything;
3: mod diagnostics;
4: mod handler;
5: mod native;
6: #[cfg(all(test, windows))]
7: mod parity;
```

`src/main.rs:62` additionally gates the `everything::status()` probe inside `--self-test` behind `#[cfg(windows)]`.

`src/handler.rs` routes native-first and only compiles the fallback on Windows:

```
10: #[cfg(windows)]
11: use crate::everything;
...
19: #[cfg(windows)]
20: fn everything_status_probe() -> anyhow::Result<serde_json::Value> {
21:     everything::status()
...
25: #[cfg(not(windows))]
26: fn everything_status_probe() -> anyhow::Result<serde_json::Value> {
27:     Ok(serde_json::json!({
28:         "available": false,
29:         "engine": "native-only",
30:         "reason": "The Everything fallback engine is Windows-only"
31:     }))
32: }
```

and the actual fallback call sites in `find_files` / `count_files` are `#[cfg(windows)]`-gated, with the non-Windows arm returning the native error (`src/handler.rs:183-190`, `219-227`).

### 2.2 The indexer has no Everything dependency

`indexer/Cargo.toml` describes the crate as `"Native file indexer service for instant file search (Phase 2: no Everything dependency)"`. `grep -rni everything indexer --include='*.rs'` returns only *comments* (query-semantics compatibility notes in `query.rs`, `mft.rs`, `content.rs`, `platform.rs`); there is no dependency, module, or FFI symbol. The 710 MB process is `instant-file-search-indexer`, so even a Windows-style fallback in the MCP server could not account for its RSS.

### 2.3 Empirical check

Running the installed MCP server in triage mode proves the engine set on this host:

```
$ /usr/local/lib/instant-file-search/instant-file-search-mcp-server --self-test
instant-file-search-mcp-server 1.0.20 dev
initialize.capabilities.tools: true
tools (5): aggregate_files, count_files, find_files, recent_changes, search_status
native: {"...","index_path":"/var/lib/instant-file-search/index.sqlite3","indexed":1621007,"storage_mode":"disk","version":"1.0.20","volumes":["/","/boot/efi","/mnt/2tb-drive"]}
```

No `everything:` line is printed at all on Linux (the print is `#[cfg(windows)]`), and status has no fallback engine entry. **H1 is refuted.**

---

## 3. H2 — disk mode / "RAM saving mode" on Linux

### 3.1 It is configured and active

```
$ cat /etc/instant-file-search/indexer.env
INSTANT_FS_INDEX_MODE=disk
INSTANT_FS_CONTENT_INDEX=auto

$ ls -l /var/lib/instant-file-search/
-rw-r--r-- 1 root root   62552431 Nov  7 14:04 changes.jsonl
-rw-r--r-- 1 root root 2688032768 Nov  7 14:04 index.sqlite3
-rw-r--r-- 1 root root    2293760 Nov  7 14:03 index.sqlite3-shm
-rw-r--r-- 1 root root    4350752 Nov  7 14:04 index.sqlite3-wal
```

Journal (UTC `Z` timestamps inside the line, local offset in the prefix):

```
2026-11-07T12:45:07-05:00 ... 2026-11-07T17:45:07.188752Z  INFO index storage mode: disk (/var/lib/instant-file-search/index.sqlite3)
2026-11-07T12:45:08-05:00 ... 2026-11-07T17:45:08.596711Z  INFO scanned /: 665470 entries
2026-11-07T12:45:08-05:00 ... 2026-11-07T17:45:08.644124Z  INFO scanned /boot/efi: 202 entries
2026-11-07T12:46:08-05:00 ... 2026-11-07T17:46:08.685361Z  INFO scanned /mnt/2tb-drive: 943262 entries
2026-11-07T12:46:21-05:00 ... 2026-11-07T17:46:21.824376Z  INFO initial index built: 1608934 entries
2026-11-07T12:46:22-05:00 ... 2026-11-07T17:46:22.125195Z  INFO listening on /tmp/instant-file-search-indexer.sock
2026-11-07T12:46:22-05:00 ... 2026-11-07T17:46:22.125529Z  INFO fanotify: watching / (filesystem-wide)
2026-11-07T12:46:22-05:00 ... 2026-11-07T17:46:22.125569Z  WARN fanotify: mark /mnt/2tb-drive failed: No such device (os error 19)
2026-11-07T12:46:22-05:00 ... 2026-11-07T17:46:22.125578Z  INFO fanotify: change watcher started on 2 volume(s)
```

Volume counts reconcile exactly: 665,470 + 202 + 943,262 = **1,608,934** indexed entries.

`indexer/src/index.rs:84-85` shows disk is the **default** mode on every platform, not a Windows-only path:

```rust
84:     pub fn from_env() -> anyhow::Result<Self> {
85:         let mode = std::env::var("INSTANT_FS_INDEX_MODE").unwrap_or_else(|_| "disk".into());
```

Queries from the disk backend stream rows through `rusqlite` cursors and never load the corpus: `indexer/src/disk.rs:479` `pub fn search(&self, opts: &QueryOptions)`, `487`/`508` `stmt.query_map(...)` behind `indexer/src/index.rs:557-560` (`if let Some(disk) = &self.disk { return disk.search(opts) }`).

### 3.2 Disk mode does *not* prevent the RAM cost — measured

Benchmarks were run with `scripts/measure-indexer-memory.sh` (created by this investigation, §7), which routes `TMPDIR` into a throwaway `mktemp -d` so the SQLite file never touches `/var/lib`. Raw JSON is reproduced from `/tmp/ifs-raw-evidence/summary.txt`; the 1.6 M rows were also measured twice via the wrapper script with identical results (±0.2 %).

| mode | entries | peak RSS (VmHWM) | peak bytes/entry | `rss_bytes_after_build` | post-build bytes/entry | DB bytes | wall s | build ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| disk | 1,600,000 | 727,928 kB = **745,398,272 B (710.9 MiB)** | 465.9 | 424,951,808 B (405.3 MiB) | 265.6 | 854,122,496 B (814.6 MiB) | 31.4 | 23,656 |
| memory | 1,600,000 | 1,743,020 kB = **1,784,852,480 B (1702.2 MiB)** | 1115.5 | 1,392,185,344 B (1327.7 MiB) | 870.1 | 0 | 6.2 | 2,308 |
| disk | 500,000 | 233,256 kB = 238,854,144 B (227.8 MiB) | 477.7 | 138,850,304 B (132.4 MiB) | 277.7 | 265,043,968 B (252.8 MiB) | 7.9 | 5,174 |
| disk | 250,000 | 120,964 kB = 123,867,136 B (118.1 MiB) | 495.5 | 73,863,168 B (70.4 MiB) | 295.5 | 132,014,080 B (125.9 MiB) | 3.7 | 2,148 |

Exact raw JSON for the 1.6 M disk run (queries elided):

```json
{"build_ms":23656.079673,"content_cache_included":false,"database_bytes":854122496,
 "entries":1600000,"io":{"available":true,"read_bytes":0,"write_bytes":0},
 "measurement":"synthetic corpus in a separate release-build process; RSS is measured after the build input is dropped",
 "mode":"disk","reopen_ms":751.13913,"rss_bytes_after_build":424951808,"runs":1,
 "schema_version":1,"update_ms":42.382177999999996}
```

This independently reproduces the Lead's figures: their documented "728 MB peak / 425,164,800 B after build" vs my "727,928 kB (728.0 MB decimal) / 424,951,808 B" — a 0.05 % difference. **The Lead's benchmark numbers are accurate.**

### 3.3 Could the "10 MiB in disk mode at 500k" claim ever have been true?

No, not with this code base and measurement method. At 500,000 entries the measured values are:

* peak RSS **227.8 MiB** (22.8× the claim),
* RSS after the build input was dropped **132.4 MiB** (13.2× the claim),
* and the benchmark reports `rss_bytes_after_build` **after** a 250 ms settle, i.e. exactly the "after indexing" window the README table describes.

The claim is not a Linux-vs-Windows artefact of the *harness*: `benchmark.rs` is platform-independent, and every mode does `let files = synthetic_entries(entries); ... index.replace(files)` before measuring (`indexer/src/benchmark.rs:65-72`), so the disk benchmark materialises the same 500k-entry corpus on Windows. `resident_bytes()` differs per platform (Windows `WorkingSetSize`, Linux `statm`, macOS `ru_maxrss` — `benchmark.rs:294-345`), and a Windows working set can be *trimmed* after the corpus is freed, which would report a much smaller "after build" number than Linux RSS without the scan ever having used less RAM. So the 10 MiB figure looks like (a) a working-set-trimming artefact, or (b) a number measured on a reopened DB without the corpus build — not evidence of a low-RAM Windows scan path. See §6 for the full contradiction list.

Note the asymmetry that makes this argument strong: the **memory-mode half of the same claim reproduces** (docs: ~495 MiB at 500k; measured here 523.9 MiB post-build / 721.9 MiB peak), while the disk-mode half is off by 13×. A real cross-platform gain would not be asymmetric in that way.

**Plain statement for the record:** the historical "10 MiB RSS in disk mode (500,000 records)" figure is **not reproducible with the current harness on Linux** — it measures 132.4 MiB after the build input is dropped and 227.8 MiB at peak. It is additionally "after build", not the initial scan. The most likely origins are a Windows working-set/measurement artefact (Windows `WorkingSetSize` can be trimmed after the corpus is freed, while Linux counts retained pages as RSS) and/or a number taken on a reopened database rather than a build. It should not be used as a baseline for the Linux scan path.

### 3.4 Linux full-rescan on every restart

`indexer/src/main.rs:419-450` (`checkpoints_can_resume`):

```rust
419: fn checkpoints_can_resume(
...
        #[cfg(windows)]
        { saved_id == live_id && saved_cursor <= live_cursor }
        #[cfg(target_os = "macos")]
        { saved_id <= live_id }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = (saved_id, saved_cursor, live_id, live_cursor);
            false
        }
```

and `indexer/src/main.rs:186-198`: the saved-checkpoint branch is only taken when that function returns true; otherwise `build_initial_index(...)` runs a full scan and logs `initial index built: {n} entries`. On Linux this is unconditional, so the corpus-sized scan cost recurs on every service start (this service restarted once already at 12:45:05, and the earlier attempt was killed by systemd at 12:45:03 — see §6j).

### 3.5 Independent cross-check of the fix (1.0.21 working-tree build)

While this investigation was running, the Lead produced a release build of the uncommitted fix (`target/release/instant-file-search-indexer 1.0.21 dev`, built 2026-11-07 13:59) which adds a `benchmark stream` mode: same synthetic corpus, fed one entry at a time through `FileIndex::replace_streaming` (the path the Linux initial scan now uses) instead of a corpus-sized `Vec`. I re-ran it myself with the same `/proc` sampling method:

| binary | mode | entries | peak RSS (VmHWM) | `rss_bytes_after_build` | DB bytes | wall s |
|---|---|---:|---:|---:|---:|---:|
| 1.0.20 installed | disk (collect-then-insert) | 1,600,000 | 727,928 kB (745,398,272 B) | 424,951,808 B | 854,122,496 B | 31.4 |
| 1.0.21 target/release | **stream** | 1,600,000 | **10,492 kB (10,743,808 B)** | **10,301,440 B (9.8 MiB)** | 854,122,496 B | 31.8 |
| 1.0.21 target/release | **stream** | 500,000 | **9,024 kB (9,240,576 B)** | **9,240,576 B (8.8 MiB)** | 265,043,968 B | 7.7 |

The streaming path removes ~99 % of the peak RSS at identical database size and near-identical wall time (30.9 s vs 31.8 s for 1.6 M entries), so the fix does address the mechanism identified in §4. My 1.6 M stream peak (10,492 kB) agrees with the Lead's independent 10,136 kB peak to within 3.5 %. The Lead also reports a real serve-as-user disk-mode walk of 1,627,303 entries peaking at 11,736 kB; that measurement is the Lead's, not repeated here (it needs the indexer's fanotify capabilities and a non-root volume walk).

Remaining caveat: these are synthetic-walk and walk-only numbers. The **live service** still runs the content-index pass in §6d and the fanotify loop in §5, so a live `serve` measurement after the fix is the number that matters for the user-facing claim.

---


## 4. Where the RAM goes — source-level accounting

Deployed 1.0.20 path (`HEAD:indexer/src/scan.rs:26-40`, unchanged by the working-tree streaming patch):

```rust
26: pub fn build_index(volumes: &[String], index: &SharedIndex) -> Result<usize> {
27:     let mut all = Vec::new();
28:     for v in volumes {
29:         match scan_volume(v) {
30:             Ok(entries) => {
31:                 tracing::info!("scanned {}: {} entries", v, entries.len());
32:                 all.extend(entries);        // ← whole volume materialised, then moved into `all`
...
39:     let n = index.replace(all);              // ← only now does anything reach SQLite
```

* `walk.rs:112` `pub fn scan_volume(volume: &str) -> Result<Vec<IndexedFile>>` returns a `Vec`; every walked entry is pushed at `walk.rs:210` (`entries.push(file)`).
* `index.rs:188` `pub fn replace(&self, entries: Vec<IndexedFile>)` takes the whole `Vec` and the disk backend inserts it row-by-row (`disk.rs:304-320`, one transaction). In disk mode the SQLite *index* is on disk, but the scan corpus is fully resident until `replace` returns.
* `IndexedFile` (`indexer/src/types.rs:12-50`) is not a thin record: `path`, `own_name`, `name`, `lower_name`, `lower_path`, `extension` are six `String`/`Option<String>` fields, five of which are populated on Linux (`own_name` is Windows-only). `IndexedFile::new` (`types.rs:52-110`) calls `refresh()` which lowercases the path into a fresh allocation (`lower_path`), copies the basename into `name`, lowercases it into `lower_name`, and extracts `extension`. Every entry therefore owns 5 heap allocations before it ever reaches SQLite.
* `indexer/src/mft.rs:85,117,154` shows the Windows scan has the *same* shape (`Vec::with_capacity(record_count)` then `entries.push(e)`), which is why H2's premise fails: Windows reads `$MFT` as a device stream (a speed technique) but still materialises the corpus. What Windows has instead is resumable USN checkpoints, so it usually does not run that scan on restart.

### Measured bytes per entry

| quantity | bytes | per entry (÷ 1,608,934 live / ÷1,600,000 bench) |
|---|---:|---:|
| live steady VmRSS 710,832 kB | 727,891,968 | **452.4 B** |
| live peak VmHWM 1,193,540 kB | 1,222,184,960 | **759.6 B** |
| bench disk 1.6 M peak | 745,398,272 | 465.9 B |
| bench disk 1.6 M post-build | 424,951,808 | 265.6 B |
| bench disk 500k peak | 238,854,144 | 477.7 B |

Hand estimate for one Linux entry: struct = **≈208 B** (five `String`s at 24 B = 120 B, six 8-byte scalars = 48 B, `u32`+2×`bool` padded = 16 B, `Option<String>` = 24 B) + heap = path ≈60 B, `lower_path` ≈60 B, `name` ≈20 B, `lower_name` ≈20 B, `extension` ≈3 B, plus ≈16 B malloc overhead per allocation (5 allocations) → **≈448 B/entry**. Measured 452.4 B/entry steady. The accounting matches the measurement to ~1 %.

Why the live *peak* is 1.19 GB while the benchmark peaks at 0.75 GB: the benchmark builds its corpus with `(0..n).map(..).collect()` (`benchmark.rs:210-259`), which preallocates exactly, while `build_index` receives one `Vec` per volume and then `all.extend(entries)` grows by doubling. At 1.6 M live entries the last extend (943,262 entries from `/mnt/2tb-drive` into an `all` of 665,672) forces a reallocation to a 2,097,152-slot buffer while the 1,048,576-slot old buffer and the 943k-entry source `Vec` are both still alive: ≈0.21 GB old buffer + ≈0.44 GB new buffer + ≈0.20 GB source buffer + ≈0.39 GB of heap strings ≈ **1.1–1.2 GB**. That is the observed VmHWM. The streaming fix removes all three transient copies; a benchmark-only verification would not have seen them.

---

## 5. Separate finding — fanotify self-write feedback loop

### 5.1 The loop exists and is self-sustaining

`HEAD:indexer/src/fanotify.rs` marks whole filesystems:

```
152:    let mark_mask = FAN_CREATE
153:        | FAN_DELETE
154:        | FAN_MOVED_FROM
155:        | FAN_MOVED_TO
156:        | FAN_MODIFY
157:        | FAN_ATTRIB
158:        | FAN_CLOSE_WRITE;
...
165:            libc::fanotify_mark(fd, FAN_MARK_ADD | FAN_MARK_FILESYSTEM, ...)
```

The indexer's own data directory is on `/`, so its own writes are watched. `HEAD:indexer/src/index.rs:312-333` (`record_change`) appends each event to `changes.jsonl` and **flushes per event**; `apply_event` (`fanotify.rs:379+`) upserts into the same SQLite database. Each write therefore produces a new event, which produces another write — a closed feedback loop. The deployed 1.0.20 binary has no internal-path filter (`is_internal_index_path` does not exist in `HEAD:indexer/src/index.rs`; it is added only in the uncommitted working tree).

### 5.2 Raw measurements

`changes.jsonl` size sampled over 10 s (2026-11-07T13:54:36 → 13:54:46 -05:00):

```
size0=61,254,761  size1=61,264,192  growth=9,431 B over 10.00 s = 943 B/s (0.9 KiB/s)
```

Over a 30 s window at ~1 s resolution (14:0x): mean **14,699 B/s**, sd 8,012 B/s, min 1,092 B/s, max 27,510 B/s. Over a 401 s window (13:51:13 → 13:57:54, `ls` sizes): **1,759 B/s**. So the growth is bursty; the Lead's "≈12 KB/s" is a plausible average but not a constant (see §6g).

Per-thread CPU delta over the same 10 s (13:54:36 → 13:54:46):

```
TID 8592: +0.01 s =  0.1 % of a core
TID 8929: +5.87 s = 58.7 % of a core
TID 8930: +4.10 s = 41.0 % of a core
process total: 10 CPU-seconds / 10.0 s = 100.0 % of one core
```

Repeated over 30 s: 8929 49.5 %, 8930 50.3 %, **total 30.00 CPU-s / 30.0 s = 100.0 %**. Correlating 30×1 s samples: `corr(8929,8930) = −0.971`, `corr(change-log B/s, 8929) = +0.22`, `corr(change-log B/s, 8930) = −0.24`. The two threads are mirror images of each other — a shared lock (the single `rusqlite::Connection` behind a `Mutex`, plus the index `RwLock`) serialises them, and the process saturates at exactly one core.

Controlled injection test (write .bin/.txt files on `/` for 8 s each, then repeat):

```
baseline                                         8.0s  8929: 32.2%  8930: 67.2%  total=100%
sustained .bin (fanotify heavy, no content)      8.0s  8929: 42.5%  8930: 57.3%  total=100%
baseline                                         8.0s  8929: 46.9%  8930: 53.1%  total=100%
sustained .txt 66KB (content-index heavy)        8.0s  8929: 44.7%  8930: 54.9%  total=100%
baseline                                         8.0s  8929: 47.7%  8930: 52.1%  total=100%
```

The total never exceeds 100 % of a core no matter how much external work is added, and the split drifts around 50/50 — evidence for a single-core lock bottleneck rather than a single hot loop.

### 5.3 The events are the indexer's own files

Last 2,000,000 bytes of `changes.jsonl` at 13:58 (15,208 parsed events):

| path prefix | events | share |
|---|---:|---:|
| `/var/lib/instant-file-search` (self) | 11,996 | 78.9 % |
| `/home/omen/.cargo` (concurrent agent cargo build) | 2,475 | 16.3 % |
| `/var/log` | 386 | 2.5 % |
| other | 351 | 2.3 % |

Internal-path breakdown, same tail: `index.sqlite3-wal` 5,910 WRITE, `changes.jsonl` 5,909 WRITE, `index.sqlite3` 101 WRITE. Whole-file totals: **369,177 of 461,589 lines (80 %)** mention `/var/lib/instant-file-search`. Representative consecutive self-events, 3 ms apart — the exact `append → flush → WAL commit → watch → append` cycle:

```json
{"timestamp":134385513992103448,"reason":"WRITE","path":"/var/lib/instant-file-search/changes.jsonl","is_dir":false}
{"timestamp":134385513992105474,"reason":"WRITE","path":"/var/lib/instant-file-search/index.sqlite3-wal","is_dir":false}
{"timestamp":134385513992105851,"reason":"WRITE","path":"/var/lib/instant-file-search/index.sqlite3","is_dir":false}
```

Arithmetic check: 5,910 WAL writes + 5,909 `changes.jsonl` writes over the 435.8 s spanned by that 2 MB tail is ≈27 self-cycles/s, and at ~137 B/event line + a SQLite commit each that is consistent with one saturated core. This confirms the loop: ~80 % of the change journal's volume is the indexer recording its own writes, and each record causes another write.

### 5.4 Caveats on this finding

* The tail also contains 2,475 events from a concurrent `cargo build` (the Lead's own build), so a naive `tail` shows cargo paths, not self-paths. The self-event share only becomes visible when you count prefixes over a window (analysis above).
* `changes.jsonl` is cumulative: its first line is `2026-11-07T03:33:24-05:00`, 10.4 h before the snapshot, and the file survives restarts (61–62 MB now). "61 MB file" therefore is not all from this run.
* `fanotify: mark /mnt/2tb-drive failed: No such device (os error 19)` means the volume holding 943,262 of the 1,608,934 entries (59 %) is **not** watched; changes there are invisible until a restart rescan.
* The hot thread's role (fanotify loop vs the content-index pass) could not be resolved with certainty without root (`/proc/8592/io`, `smaps`, `syscall`, `stack`, `wchan` symbol are all root-only here). See §6c.

---

## 6. Contradictions / uncertainty (do not soften)

Legend: **[C]** = does not fit the Lead's story; **[U]** = unresolved due to privileges or ambiguity.

a. **[C] The live high-water mark is 1.64× the benchmark peak the Lead cites.** Live `VmHWM` is 1,193,540 kB (1.22 GB, 759.6 B/entry) while `benchmark disk 1600000` peaks at 727,928 kB (745 MB, 465.9 B/entry). The explanation is *not* "the service does extra things" hand-waving: `build_index` materialises one `Vec` per volume and then `all.extend()` doubles capacity while the old buffer and the source volume's `Vec` are alive (§4), which `benchmark.rs` avoids by `collect()`-ing with an exact size hint. Implication: **any fix verified only with the benchmark will under-report the real peak by ~0.4–0.5 GB**, and the Lead's "728 MB peak" cannot be the whole peak story.

b. **[C] Live steady RSS is 1.7× the benchmark's post-build RSS.** Live retained RSS is 727,891,968 B (452 B/entry), but the benchmark's `rss_bytes_after_build` for the same 1.6 M corpus is 424,951,808 B (266 B/entry). So the benchmark's "RSS after build input dropped" does *not* reproduce what the live service retains. Candidate explanation: glibc arena retention/fragmentation after the larger live peak (1.22 GB) plus concurrent content-index allocations. **Update:** the fixed 1.0.21 `benchmark stream` path does drop to 10.3 MB after build (§3.5), which confirms the mechanism; the residual gap is between *benchmark* and *live service*, and only a live post-fix `serve` measurement can settle it.

c. **[C][U] The "hot thread pinned in the fanotify loop" claim is incomplete.** There are two hot threads at ~50/50 (`8929`, `8930`), the process is pinned at exactly 100.0 % of one core in *every* window (10 s, 30 s, 5 s) even when I inject extra events, and the two threads' CPU shares are anti-correlated (−0.971) — i.e. they serialise on a shared mutex, not on fanotify event supply. Thread role attribution is unresolved: source order says the content-index thread is spawned first (`indexer/src/main.rs:249`) and the fanotify watcher second (`:302`), which would make 8929 the **content-index pass**, yet 8929 is the thread with 3,676 s of lifetime CPU and 8930 only 137–330 s. A fanotify watcher that was busy from 12:46 should have accumulated CPU all along, which favours 8929 = watcher; but 8930's recent jump to ~50 % coincides with the cargo build creating thousands of new files, which favours 8930 = content thread. No root privileges (`/proc/<pid>/io`, `smaps_rollup`, `syscall`, `stack` all denied) prevented a decisive read. **Either way, half of the CPU is not the fanotify loop**, so "the fanotify loop pins the core" is at best half the story.

d. **[C] A content-indexing pass is still running 80 minutes after startup and was not part of the Lead's diagnosis.** Disk storage + `INSTANT_FS_CONTENT_INDEX=auto` enables `ContentStore::disk` and a background fill pass (`indexer/src/main.rs:211-296`) that walks every eligible text entry and writes bodies into the same `index.sqlite3`. At 14:05 the live API reports `content: {"enabled":true,"files":72640,"bytes":565975205,"storage_mode":"disk","budget_bytes":2147483648}` — 540 MiB of content written, still growing, and the journal **never** contains the `content index built: ...` completion line. That pass shares the single SQLite connection with the fanotify loop. Consequences: (i) part of the 1.19 GB VmHWM and of the steady RSS may belong to the content pass, not the scan; (ii) the proposed scan-streaming fix does not touch it; (iii) post-fix "tens of MB RSS" may be missed if the content pass is active during the measurement. This is a live performance bug in its own right (unbounded-in-time pass), not a contradiction of the scan diagnosis — but it does contradict a single-cause narrative.

e. **[C] `index.sqlite3` is 2.688 GB while a fresh 1.6 M-entry database is 0.854 GB and live content is 0.566 GB.** 2.688 GB − (0.815 GB metadata + 0.566 GB content) leaves **≈1.3 GB of free pages/bloat**, consistent with `DELETE FROM files` on every start plus `clear_content` without `VACUUM` (`indexer/src/main.rs:205-213` explicitly says "No VACUUM: a multi-GB index would delay service start"). That bloat is a plausible contributor to the 36 s gap between the earlier run's `volumes:` log and `index storage mode: disk` log (12:43:33 → 12:44:09), i.e. `DiskIndex::open`'s `PRAGMA quick_check(1)` over 2.688 GB (`disk.rs:97`). Any memory story that uses "the index is only 1.6 M rows" should account for this.

f. **[C] systemd/cgroup memory numbers are not process RSS and will mislead.** The cgroup `memory.peak` for the current run is 2,650,951,680 B (2.47 GiB) versus process VmHWM 1,222,184,960 B (1.14 GiB); an earlier killed run logged `Consumed 20.817s CPU ... 5.1G memory peak`. These include page cache charged to the cgroup. Do not cite cgroup `MemoryPeak` as the indexer's RSS.

g. **[C] "changes.jsonl grown to 53 MB and growing ~12 KB/s" is directionally right but imprecise.** The file is now 62.6 MB and cumulative over ~10.4 h (first event 03:33) across service runs; current-run average is not separable from the file alone. Measured rates: 943 B/s (10 s window), 1,759 B/s (401 s), 14,699 ± 8,012 B/s (30 s, range 1.1–27.5 KB/s). The rate is bursty because 16 % of recent traffic is the Lead's own `cargo build` on `/` (`~/.cargo/registry/src/...`), as well as the self-loop. The self-loop share is still overwhelming over a wider window (79 % of the last 2 MB, 80 % of the whole file), but "12 KB/s" should not be treated as a constant.

h. **[U] Thread count evidence conflicts with a single-hot-loop model.** `Threads: 3`; the main thread does the accept loop and 41 s of startup work, and there is no per-thread name (`comm` is the process name for all three), and `wchan`/`syscall` are permission-denied. Any claim that a *specific* TID is the fanotify thread is inference, not direct observation. I have labelled it as such.

i. **[C] The indexing work is unbounded by the change log size.** `record_change` keeps a ring buffer of 100,000 events (`HEAD:indexer/src/index.rs:51` `MAX_CHANGES`) and flushes every line, so memory does not grow with the log, but the fsync/flush per event guarantees the loop never idles. This is a design consequence, worth stating: even after filtering internal paths, any external `/` writer (e.g. `cargo build`, browser caches) generates change-log traffic and SQLite commits at this cost. The proposed self-path filter is necessary, not sufficient, for the CPU symptom.

j. **[C] The service barely fits its systemd start timeout, and the scan is on the critical path.** The 12:43:33 run was killed at 12:45:03 with `start operation timed out` after `scanned /` and before `/mnt/2tb-drive` finished; the successful run took 77 s (12:45:05 → 12:46:22) against the default 90 s `TimeoutStartSec`. A 2.688 GB DB `quick_check` (≈36 s) plus a 1.6 M-entry scan (≈76 s total observed) is already over budget on a slower disk. Memory measurements taken after a restart are therefore also timing measurements.

k. **[U] MCP-server/resident-process ambiguity.** There is a second ~710 MB process on this machine (`node .../dsh web`, PID 33201, 709,816 kB) that can be mistaken for the file-search MCP server. The actual `instant-file-search-mcp-server` was not running at all. Any report that says "the MCP server uses 700 MB" is wrong on this host.

l. **[U] Benchmark I/O counters are unusable here.** Both benchmark runs report `"io":{"available":true,"read_bytes":0,"write_bytes":0}` although the disk run wrote 854 MB — `/proc/self/io` block-layer counters are not a reliable signal on this kernel/filesystem, so do not use the benchmark's I/O fields for the "8.3 GiB moved" style claims.

---

## 7. Deliverable: `scripts/measure-indexer-memory.sh`

Location: `scripts/measure-indexer-memory.sh` (executable, `set -euo pipefail`). It runs the indexer's read-only `benchmark` subcommand, redirects `TMPDIR` into a fresh `mktemp -d` (so the throwaway SQLite database can never land in `/var/lib/instant-file-search`), samples `VmRSS`/`VmHWM` from `/proc/<pid>/status` every 0.2 s, stops on process exit or zombie, and prints one compact table.

The script is **non-destructive**: it never deletes anything — its temp directory is left in place and its path is printed for manual cleanup (the Lead adjusted the original `mktemp` cleanup to this non-destructive form before commit, and added `stream` to the documented `MODES` values).

```
Usage: scripts/measure-indexer-memory.sh [ENTRY_COUNT]   # default 1600000
Env:   BIN, MODES ("disk memory"), EXTRA_COUNTS ("250000 500000"),
       SAMPLE_INTERVAL
       MODES="disk stream" compares the collecting path with the 1.0.21+ streaming path
```

Smoke test (50,000 entries):

```
$ MODES="disk" ./scripts/measure-indexer-memory.sh 50000
| mode   |   entries |   peak_MiB | steady_MiB | rss_after_MiB |   wall_s | database_MiB |   build_ms |
|--------|-----------|------------|------------|------------|----------|--------------|------------|
| disk   |     50000 |       30.2 |       20.6 |       20.6 |      0.9 |         24.7 |        313 |
```

The full runs used for §3.2:

```
$ ./scripts/measure-indexer-memory.sh 1600000
| mode   |   entries |   peak_MiB | steady_MiB | rss_after_MiB |   wall_s | database_MiB |   build_ms |
|--------|-----------|------------|------------|------------|----------|--------------|------------|
| disk   |   1600000 |      711.1 |      403.9 |      405.5 |     30.9 |        814.6 |      23436 |
| memory |   1600000 |     1702.5 |      893.4 |     1327.7 |      6.1 |          0.0 |       2236 |

$ EXTRA_COUNTS="250000 500000" ./scripts/measure-indexer-memory.sh 1600000
| disk   |    250000 |      118.0 |       68.3 |       70.4 |      3.6 |        125.9 |       2032 |
| memory |    250000 |      362.5 |      266.4 |      264.1 |      1.3 |          0.0 |        398 |
| disk   |    500000 |      227.8 |      131.6 |      132.5 |      7.8 |        252.8 |       5057 |
| memory |    500000 |      721.9 |      303.8 |      523.9 |      3.0 |          0.0 |        842 |
```

Caveat printed by the script and confirmed here: the benchmark materialises `synthetic_entries()` in RAM in **both** modes, so the disk row is a lower bound on live behaviour (it omits `build_index`'s per-volume `Vec` and reallocation peak).

The `scripts/` directory has no README/conventions file, so no note was added there.

---

## 8. Reproduction commands (compact)

```bash
# process identity
ps -eo pid,user,rss,vsz,pcpu,etime,args | grep -i instant | grep -v grep
grep -E '^(Name|State|Threads|VmPeak|VmSize|VmHWM|VmRSS|RssAnon|RssFile)' /proc/8592/status

# thread CPU (per tid)
python3 - <<'EOF'
import os
for tid in sorted(os.listdir('/proc/8592/task')):
    s=open(f'/proc/8592/task/{tid}/stat').read(); r=s[s.rindex(')')+2:].split()
    u,st=int(r[11]),int(r[12]); print(tid, 'cpu_s=%.2f'%((u+st)/os.sysconf('SC_CLK_TCK')))
EOF

# mode / config / journal
cat /etc/instant-file-search/indexer.env
journalctl -u instant-file-search-indexer --no-pager -o short-iso | grep -E 'index storage mode|initial index built|scanned /|fanotify'

# MCP server engine triage (read-only)
/usr/local/lib/instant-file-search/instant-file-search-mcp-server --self-test

# RAM measurements
./scripts/measure-indexer-memory.sh 1600000
EXTRA_COUNTS="250000 500000" ./scripts/measure-indexer-memory.sh 1600000

# fanotify self-write loop
stat -c '%s %y' /var/lib/instant-file-search/changes.jsonl; sleep 10; stat -c '%s %y' /var/lib/instant-file-search/changes.jsonl
python3 - <<'EOF'   # prefix census over the last 2 MB
import json,collections
b=open('/var/lib/instant-file-search/changes.jsonl','rb').read()
lines=b.split(b'\n')[:-1][-15000:]
c=collections.Counter(json.loads(l)['path'].split('/')[1] for l in lines if l)
print(c)
EOF
```

Evidence directory for raw benchmark JSON: `/tmp/ifs-raw-evidence/` (`summary.txt`, per-run `out.json`), preserved at time of writing; it is outside the repo and not a deliverable.

---

## 9. What I would tell the Lead to change in the narrative

1. Keep the H1 refutation — it is airtight at process identity, source gating, crate dependency, and `--self-test` level.
2. Rephrase the H2 conclusion from "Linux is missing the Windows streaming technique" to: *"the Linux initial scan materialises the corpus in RAM and Linux re-runs it on every restart; disk mode stores the index on disk but does not stream the scan. Windows has the same collect-then-insert scan, but usually skips it via durable USN checkpoints."* That survives source review; the current wording does not.
3. Quote the live peak (1.19 GB / 760 B per entry) separately from the benchmark peak (728 MB / 466 B per entry) and explain the `Vec::extend` doubling; do not present the benchmark peak as the service peak.
4. Do not state that the fanotify loop is *the* pinned thread without qualification. State: two threads, serialised on one SQLite connection, process pinned at 100 % of one core; the self-write loop is confirmed as ~80 % of change-log volume; the content-index pass is a second live consumer that the fix does not address.
5. Measure the live service after the fix (RSS *and* CPU), not just `benchmark disk`; and measure it after the content pass has either completed or been disabled, otherwise the result is not attributable to the scan fix. The `benchmark stream` result (§3.5: 10.5 MB peak at 1.6 M entries) and the Lead's serve-as-user walk (11.7 MB peak) are strong evidence the scan fix works; the still-open question is the *live* steady RSS/CPU with the content pass and watcher active.
