# Linux Support Assessment

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
- **Permissions:** `FAN_MARK_FILESYSTEM` requires root/CAP_SYS_ADMIN (unprivileged
  fanotify since 5.13 is `FAN_CLASS_NOTIF` + FID but inode marks only). Run as a
  systemd service with `AmbientCapabilities=CAP_SYS_ADMIN`, or fall back to per-directory
  inode marks unprivileged (Tracker's model).
- **Event identity:** FID events carry `(fsid, file handle)` + parent handle + name —
  survives renames, resolvable via `open_by_handle_at`. `FAN_RENAME` (5.17+) is a single
  atomic event with old+new parent+name (the USN rename analogue).
- **Scalability:** one event queue per group, system-wide; default cap 16384 events
  (`FAN_Q_OVERFLOW` = rescan signal). FSearch handles "tens of thousands of changes/sec".
- **Known blind spots:** no events for mmap/msync write-back, no remote NFS changes,
  btrfs subvolumes may reject filesystem marks with `EXDEV` (fall back to inotify, as
  Tracker does).

**There is no USN journal on Linux.** ext4's JBD2 is a metadata journal (not an API);
XFS exposes an LSN (no userspace API); btrfs has an internal change log (no stable API).
The kernel change-journal proposal (Amir Goldstein, LSFMM 2018-2025) is design-stage,
unmerged. eBPF kprobes on `vfs_*` are viable (Datadog/Wazuh scale) but fragile across
kernel versions, need root, and still have no persistence — wrong tool for an indexer.
auditd can stream file events but loses events under load and is security-shaped.

**Consequence for `recent_changes`:** the "since X" API must be a user-space
construction — the daemon tails fanotify, persists every event into its own ring buffer
(append log / SQLite), and serves "since X" from that store. Events that occurred while
the daemon was down are unrecoverable → rescan required. This is exactly the Windows
ring-buffer design, minus the kernel-side persistence.

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
  ABI. **Not recommended as the default** — a walk is the sane engineering choice.

**Timestamps:** `stx_btime` ↔ date_created (ext4/XFS/btrfs yes; ext3/tmpfs/NFS no),
`stx_mtime` ↔ date_modified, `stx_atime` ↔ date_accessed (unreliable on Linux defaults —
`relatime`/`noatime`). `stx_ctime` is NOT a file-change time (it's inode status change).

## 3. Index key (inode vs NTFS FRN)

**Key by `(st_dev, st_ino)`, never `st_ino` alone** (POSIX identity pair; inode numbers
repeat across filesystems). Rules:

- **Drop entries on delete** — inode numbers are reused immediately and undetectably
  (no sequence-number guard, unlike NTFS FRN's 16-bit sequence).
- **Do not persist `(st_dev, st_ino)` across reboots** — `st_dev` is not stable across
  reboots (btrfs/ZFS change it; Baloo bug 402154). Persist `f_fsid`/file handles or
  rescan.
- **Handle multi-path-per-inode** (hard links, bind mounts) — the "one path per file"
  model already needs this on NTFS.
- **btrfs subvolumes** report a per-subvolume anonymous `st_dev` (not in mountinfo, not
  stable); use `stx_subvol` (6.10+) or `(fsid, subvol, ino)`.
- **`frn:` filter maps to `st_ino`**, scoped by device/mount, documented as best-effort
  (no reuse guard).

**Parent resolution:** build `(dev, ino) → path` during the scan (as the Windows engine
builds FRN→path). For incremental updates, fanotify FID events give the parent handle +
child name (`FAN_REPORT_DIR_FID|FAN_REPORT_NAME`) or the child's own handle
(`FAN_REPORT_TARGET_FID`) — resolve via `open_by_handle_at` + `fstatat`. `FAN_RENAME`
updates the path old→new atomically.

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
initial scan is a walk, not a raw read — seconds-to-minutes instead of instant, and
(2) `recent_changes` loses kernel-side persistence — the daemon must own the ring buffer
and rescan after downtime. Everything's fallback engine is Windows-only, so Linux is
native-only by construction.