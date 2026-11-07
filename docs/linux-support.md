# Linux Support Assessment

> **Status (2026-11-07): implemented.** This document started as a port
> assessment; the native Linux backend now ships and runs as
> `instant-file-search-indexer.service`. Sections 1-3 are the original
> 2026-08-03 research that drove the port and are kept as history; section 4
> records measured current behavior. Read "would" and "planned" below as
> "was the plan", not "still missing".

What it would take to port the native indexer to Linux. Research conducted 2026-08-03
via three parallel lanes (change tracking, fast enumeration, inode semantics). Sources
are cited inline; all findings are from kernel docs, man pages, LWN, and production
indexer precedent (FSearch, Tracker, Baloo, Recoll, kerything).

## Executive summary

The Windows engine's two pillars are (1) raw NTFS `$MFT` reads for instant full
enumeration and (2) the USN Change Journal for incremental updates. **Neither has a
direct Linux equivalent.** The port is feasible but requires a different architecture:

- **Enumeration:** a `getdents64` + batched `statx` walk (parallel by subtree) replaces
  the `$MFT` read. Warm: ~2-6 s for 2.7M files; cold SSD: ~15-40 s; cold HDD: minutes.
  Raw ext4 inode-table reads (kerything's approach) can match Everything-class latency
  (~2-3 s even cold) but need root and are ext4-only.
- **Change tracking:** fanotify (`FAN_CLASS_NOTIF | FAN_REPORT_DFID_NAME |
  FAN_REPORT_TARGET_FID`, `FAN_MARK_FILESYSTEM`) replaces the USN journal. There is
  **no persistent kernel journal on Linux** (the change-journal proposal is unmerged as
  of 2026). "recent_changes since X" must be a user-space ring buffer the daemon
  persists itself; any downtime or queue overflow forces a rescan.
- **Index key:** `(st_dev, st_ino)` replaces the NTFS file reference number. Sound for a
  live index, but inode numbers have **no sequence-number reuse guard** (unlike NTFS
  FRN), are only unique per filesystem, and `st_dev` is not stable across reboots.

## 1. Change tracking (USN journal replacement)

**Recommended: fanotify** in `FAN_CLASS_NOTIF` mode with `FAN_REPORT_DFID_NAME` +
`FAN_REPORT_TARGET_FID`, one `FAN_MARK_FILESYSTEM` mark per volume. This is what
GNOME Tracker (since 3.3) and FSearch (since 0.3) converged on.

- **Why not inotify:** per-directory watches, no hierarchy, hard per-user limits
  (`max_user_watches` default ~1% of RAM, clamped 8192-1M; `max_queued_events` 16384).
  FSearch's author: unusable beyond ~1M files.
- **Permissions:** `FAN_MARK_FILESYSTEM` requires root/CAP_SYS_ADMIN. The current
  implementation runs this path through systemd with `AmbientCapabilities=CAP_SYS_ADMIN`.
  It does not yet provide an inotify fallback, so unprivileged execution and filesystems
  where filesystem-wide fanotify marks fail are reported as unsupported rather than
  silently providing incomplete change tracking. A `fuseblk` volume (for example an
  `ntfs-3g` mount such as `/mnt/2tb-drive` on this machine) is one of those: the mark
  fails with `ENODEV`, the journal logs a warning, and changes on that mount are not
  tracked until the index is rebuilt on the next start.
- **Event identity:** FID events carry `(fsid, file handle)` + parent handle + name, 
  survives renames, resolvable via `open_by_handle_at`. `FAN_RENAME` (5.17+) is a single
  atomic event with old+new parent+name (the USN rename analogue).
- **Scalability:** one event queue per group, system-wide; default cap 16384 events
  (`FAN_Q_OVERFLOW` = rescan signal). FSearch handles "tens of thousands of changes/sec".
- **Known blind spots:** no events for mmap/msync write-back, no remote NFS changes,
  btrfs subvolumes may reject filesystem marks with `EXDEV`; this is currently a
  documented limitation because the inotify fallback is not implemented.

**There is no USN journal on Linux.** ext4's JBD2 is a metadata journal (not an API);
XFS exposes an LSN (no userspace API); btrfs has an internal change log (no stable API).
The kernel change-journal proposal (Amir Goldstein, LSFMM 2018-2025) is design-stage,
unmerged. eBPF kprobes on `vfs_*` are viable (Datadog/Wazuh scale) but fragile across
kernel versions, need root, and still have no persistence, wrong tool for an indexer.
auditd can stream file events but loses events under load and is security-shaped.

**Consequence for `recent_changes`:** the daemon persists its bounded event history as
newline-delimited JSON at `/var/lib/instant-file-search/changes.jsonl` on Linux
(override with `INSTANT_FS_CHANGE_LOG`). Events already written survive normal daemon
restarts. Events that occur while the daemon is down remain unrecoverable, and a future
hardening pass should add explicit downtime-gap detection and rescan signaling.

## 2. Fast enumeration ($MFT replacement)

**Recommended: `getdents64` + batched `statx`, parallel by subtree.** No kernel API
exposes a bulk inode dump (debugfs is explicitly not a stable ABI; `FS_IOC_GETFSMAP`
maps space, not files).

- Use a large `getdents64` buffer (1-4 MiB; glibc's 32 KB default is the trap for huge
  dirs). Use `d_type` to skip `stat` for type decisions. Batch `statx`
  (`STATX_BASIC_STATS | STATX_BTIME`) for size/times. Parallelize by directory subtree
  (the `fd`/`srch`/`ferret` pattern). io_uring statx is optional and usually slower than
  a thread pool.
- **Expected:** warm dentry cache ~2-6 s for 2.7M files; cold SSD ~15-40 s; cold HDD
  minutes.
- **Raw ext4 reads** (parse group descriptors → linear inode table, like `$MFT`) reach
  ~1.2M+ files/sec (kerything), ~2-3 s cold. But: needs root/disk-group, unsafe while
  mounted read-write (stale data vs kernel cache), and only ext4 has a stable documented
  linear inode table. XFS is a large btree-walk effort; btrfs's format is not a stable
  ABI. **Not recommended as the default**: a walk is the sane engineering choice.

**Timestamps:** `stx_btime` ↔ date_created (ext4/XFS/btrfs yes; ext3/tmpfs/NFS no),
`stx_mtime` ↔ date_modified, `stx_atime` ↔ date_accessed (unreliable on Linux defaults, 
`relatime`/`noatime`). `stx_ctime` is NOT a file-change time (it's inode status change).

## 3. Index key (inode vs NTFS FRN)

**Key by `(st_dev, st_ino)`, never `st_ino` alone** (POSIX identity pair; inode numbers
repeat across filesystems). Rules:

- **Drop entries on delete**: inode numbers are reused immediately and undetectably
  (no sequence-number guard, unlike NTFS FRN's 16-bit sequence).
- **Do not persist `(st_dev, st_ino)` across reboots**, `st_dev` is not stable across
  reboots (btrfs/ZFS change it; Baloo bug 402154). Persist `f_fsid`/file handles or
  rescan.
- **Handle multi-path-per-inode** (hard links, bind mounts), the "one path per file"
  model already needs this on NTFS.
- **btrfs subvolumes** report a per-subvolume anonymous `st_dev` (not in mountinfo, not
  stable); use `stx_subvol` (6.10+) or `(fsid, subvol, ino)`.
- **`frn:` filter maps to `st_ino`**, scoped by device/mount, documented as best-effort
  (no reuse guard).

**Parent resolution:** build `(dev, ino) → path` during the scan (as the Windows engine
builds FRN→path). For incremental updates, fanotify FID events give the parent handle +
child name (`FAN_REPORT_DIR_FID|FAN_REPORT_NAME`) or the child's own handle
(`FAN_REPORT_TARGET_FID`), resolve via `open_by_handle_at` + `fstatat`. `FAN_RENAME`
updates the path old→new atomically.

## 4. Memory behavior on Linux (measured 2026-11-07)

The large process on Linux is the native indexer
(`/usr/local/lib/instant-file-search/instant-file-search-indexer serve`, systemd
unit `instant-file-search-indexer.service`, running as root), not the MCP server
process and not the Everything fallback engine. `mod everything;` is
`#[cfg(windows)]` in `src/main.rs`, so the fallback is not compiled at all on
Linux; `src/handler.rs` answers its non-Windows status probe with
`{"engine":"native-only","reason":"The Everything fallback engine is Windows-only"}`.

Disk mode is active, not missing. `/etc/instant-file-search/indexer.env` sets
`INSTANT_FS_INDEX_MODE=disk`, the journal logs `index storage mode: disk
(/var/lib/instant-file-search/index.sqlite3)`, the database was 2.7 GB on this
machine, and disk queries stream rows from SQLite. "Disk mode is not implemented
on Linux" does not describe this machine.

The RAM cost was the initial scan, not query time. `scan::build_index()` called
`platform::scan_volume()` for each volume (`/`, `/boot/efi`, `/mnt/2tb-drive`),
collected every entry into one `Vec<IndexedFile>` with `all.extend(entries)`, and
only afterwards called `index.replace(all)`. Each `IndexedFile` carries six
precomputed heap strings (`path`, `name`, `lower_name`, `lower_path`, `own_name`,
`extension`), roughly 450 to 650 bytes per entry, so the 1,608,934 entries
(665,470 + 202 + 943,262) held hundreds of megabytes before any row reached
SQLite.

Measured on host "Omen", Ubuntu-like Linux, kernel 7.0, x86_64, 2026-11-07.
Memory figures are VmRSS/VmHWM read from `/proc`:

| Measurement | Result |
|---|---|
| Deployed 1.0.20 service, steady | VmRSS 710,092 kB |
| Deployed 1.0.20 service, high-water mark | VmHWM 1,193,540 kB |
| Installed 1.0.20 `benchmark disk 1600000`, peak | 728 MB RSS |
| Same benchmark, `rss_bytes_after_build` after the input was dropped | 425,164,800 bytes (425 MB) |

The last row is the telling one: even disk mode kept a large RSS after the build
input was released, which is what a collect-then-insert scan looks like. The
older "10 MiB RSS in disk mode" figure in `low-memory-roadmap.md` measured a
synthetic benchmark after its build, not the initial scan of a real corpus, and
it does not reproduce on Linux with the current harness: at 500,000 entries the
same disk benchmark reported 132 MiB after the build and 228 MiB at peak, while
memory mode roughly reproduced its own 495 MiB figure. The 10 MiB was probably a
Windows working-set/measurement artefact in addition to being a post-build
number.

There was also no Windows-only low-RAM scan technique to port. The Windows MFT
scanner materialises its corpus the same way (`mft.rs` builds a
`Vec::with_capacity(record_count)`), so the Linux scan was not missing a Windows
trick. What differs is restart behavior: Windows USN checkpoints let a restart
skip the scan, while Linux must re-walk every time.

The live service's 1.19 GB high-water mark also exceeds the benchmark's 728 MB
peak. `build_index()` holds one `Vec` per volume and then extends those into a
single corpus `Vec`, so both buffers are briefly resident; the synthetic
benchmark preallocates exactly one vector and misses that doubling.

**Fix in this change.** The Linux disk-mode initial scan now streams entries into
one SQLite transaction as they are discovered (`FileIndex::replace_streaming`,
`DiskIndex::replace_streaming`, `walk::scan_volume_into`) instead of
materializing a corpus-sized `Vec`. Only that initial `serve` scan streams: the
fanotify queue-overflow recovery still uses the collect-then-insert
`build_index()` so it can swap the index atomically while queries are served.
Measured on the same host on 2026-11-07 with a release build of this change
(VmRSS/VmHWM from `/proc`):

| Measurement | Before (collect-then-insert) | After (streaming) |
|---|---|---|
| Synthetic 1.6M-entry corpus, peak | 727,588 kB (710 MiB) | 10,136 kB (9.9 MiB) |
| Same benchmark, after build | 413,536 kB (404 MiB) | 8,364 kB (8.2 MiB) |
| Real walk, 1,627,303 entries, peak during scan | 710,092 kB steady / 1,193,540 kB VmHWM | 11,736 kB (11.5 MiB) |

The before column's synthetic rows reproduce the earlier measurement, so the
delta comes from the scan path rather than a different build. Build wall time was
unchanged (25.4 s vs 25.7 s for the synthetic corpus) and the query set returned
identical results and timings (for example `*.rs` at 1,024 ms both, and
`file:*.json` at 2,876 ms vs 2,857 ms), so the change costs no build time and no
query performance. The real-walk run could not bind the production indexer socket
because the live root service owns it and ended with "Address already in use",
so that early run did not observe post-scan steady state; the synthetic streaming
row (8.2 MiB) is the scan-time figure, and the shipped service's measured steady
state is given next.

A later restart of the shipped 1.0.22 service confirmed the steady state in
production: 2,687,923 entries indexed (the machine had gained a fourth volume, an
overlay tree being rebuilt by an image build) in about three minutes, then
VmRSS = VmHWM = 31,056 kB. That figure includes the by-design 100,000-event
change ring buffer and the content cache, so it is above the scan's own 10-12 MiB.

Reproduce with `instant-file-search-indexer benchmark disk 1600000` (old
collect path) and `instant-file-search-indexer benchmark stream 1600000` (the
`stream` mode added in this change; same synthetic corpus, temporary database
removed automatically).

**Self-write feedback loop.** The fanotify watcher marks whole filesystems, so it
also saw the indexer's own writes to `/var/lib/instant-file-search`
(`index.sqlite3`, `-wal`, `-shm`, `changes.jsonl`). Applying those events wrote
the same files again and produced more events: two threads shared the work
(they serialise on the single SQLite connection mutex) and pinned one core at
100%, while `changes.jsonl` reached 53 MB. About 80% of the recent journal tail
was the indexer's own files. Events for those paths are now dropped before they
reach the index or the change ring buffer.

While the loop was active a content-indexing pass was still running after
80 minutes; it shares the same SQLite mutex, so it was likely competing with the
loop, but it was not separately diagnosed. If content indexing is still slow
after this fix, that is a separate follow-up.

**Restart still means a full walk on Linux.** `checkpoints_can_resume()` returns
false unconditionally on Linux (`#[cfg(not(any(windows, target_os = "macos")))]`):
fanotify keeps no persistent event history to replay, while Windows USN
checkpoints and macOS FSEvents can resume. Every Linux service start therefore
redoes the walk. The streaming scan makes that restart cheap in RAM, but it is
still a walk, which is why the same architecture behaves differently from
Windows. The fallback engine is not involved, and disk mode does not remove it.

**Startup progress.** The pipe server starts only after the initial walk, so
`search_status` cannot answer "where is the scan" while it runs. The indexer now
publishes progress three ways:

- `journalctl -u instant-file-search-indexer -f` logs `scanning <volume>
  (volume N/M)` when a volume starts, a heartbeat at most every 5 s or 100,000
  entries, whichever comes first
  (`scanning /mnt/2tb-drive: 22610 entries (708288 total)`), and
  `scanned <volume>: N entries (M total)` when that volume finishes.
- `/run/instant-file-search/scan.json` is world-readable and rewritten
  atomically every 2 s. It carries `state` (`scanning` / `ready` / `failed`), the
  current `volume` and `volume_index`, `entries`, `entries_in_volume`,
  `started_at`, `updated_at`, `finished_at`, `pid`, and the `volumes` list. Both
  the indexer and the MCP reader honour `INSTANT_FS_SCAN_STATUS_PATH` for tests
  and non-service runs.
- `systemctl status instant-file-search-indexer` shows the same heartbeat via
  `sd_notify(STATUS=...)`, and `search_status` includes the file as `scan`, so
  an agent asking during startup gets the current volume and entry count instead
  of only "not accepting connections".

The unit also sets `TimeoutStartSec=900` and a `RuntimeDirectory` for the status
file. systemd's default 90 s start timeout is too short for a cold-cache walk:
this machine measured 81 s warm for 1.63 M entries, and a later restart against
a cold `ntfs-3g`/FUSE volume took about three minutes for 2.69 M entries. Journal
entries from before the change show starts killed at 90 s and restarted in a
loop; 300 s was still not enough under the FUSE cold path.

## Porting plan (what would need to change)

| Windows module | Linux replacement | Effort |
|---|---|---|
| `mft.rs` (raw $MFT parse) | `getdents64` + `statx` walk (new `walk.rs`) | Medium |
| `usn.rs` (USN journal watcher) | fanotify FID watcher (new `fanotify.rs`) | Medium |
| `index.rs` (record-number keyed) | key by `(st_dev, st_ino)`; drop-on-delete | Low-Medium |
| `sector_reader.rs` (raw volume) | not needed (walk path) | Remove |
| `pipe.rs` (named pipe) | Unix domain socket | Low |
| `everything.rs` (Everything IPC fallback) | no fallback engine on Linux; native-only | Remove/disable |
| `handler.rs` / `tools.rs` / `native.rs` | mostly portable; `frn:` → ino, `attrib:` → statx mode | Low |
| Service host (SCM) | systemd unit | Low |
| Build (x86_64-pc-windows-gnu) | Linux target; `cfg(target_os)` | Low |

**Verdict:** a Linux port is feasible and the query surface (wildcards, filters, dates,
`recent_changes`, `aggregate_files`) ports cleanly. The two real costs are (1) the
initial scan is a walk, not a raw read: seconds-to-minutes instead of instant, and
(2) `recent_changes` loses kernel-side persistence, the daemon must own the ring buffer
and rescan after downtime. Everything's fallback engine is Windows-only, so Linux is
native-only by construction.
