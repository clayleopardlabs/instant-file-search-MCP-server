# Architecture

> **Scope:** the two-engine diagram, the native-first routing, and the
> Everything-fallback discussion below describe the **Windows** path. On Linux
> and macOS the `everything` module is not compiled and the native indexer is the
> only engine; the Linux backend is described under "The native indexer".

## Two engines, one interface

The server queries either of two engines behind the same tool surface
(`search_status` / `find_files` / `count_files`):

```
native-first:  main.rs → handler.rs → native.rs → named pipe → indexer (MFT index + USN journal)
fallback:      main.rs → handler.rs → everything.rs → everything-ipc (WM_COPYDATA) → Everything GUI
               (rmcp/stdio)  (blocking, spawn_blocking)                      (NTFS MFT index)
plugin/src/index.ts → spawns binary as subprocess → NDJSON-over-stdio
```

**Native-first routing:** when the native indexer's named pipe
(`\\.\pipe\instant-file-search-indexer`) is reachable, search/count requests go
to it; the Everything engine is the fallback. The status tool reports both
engines.

## The native indexer (`indexer/` crate)

A separate Rust binary (`instant-file-search-indexer.exe`) that owns the
filesystem map. Runs as the Windows service `instant-file-search-indexer`
(auto-start) or in console mode (`serve` / one-shot `scan`).

- **MFT scan:** reads the raw `$MFT` data stream sequentially in blocks and
  parses FILE records in memory (with USA fixup). Timestamps come from
  `$STANDARD_INFORMATION` (created@+0, modified@+8, accessed@+24) and sizes
  from the **unnamed** `$DATA` attribute (resident: value length; nonresident:
  valid-data-length at attribute header +48), the `$FILE_NAME` attribute's
  metadata goes stale on NTFS (only refreshed on rename). A *named* `$DATA`
  attribute is an alternate data stream, not the file content: a browser stamps
  a ~166-byte `Zone.Identifier` on every download, so accepting the first
  `$DATA` seen reported a 4.4 GB ISO as 166 bytes. The name length at attribute
  header +9 identifies a named stream and it is skipped. Full-volume scan:
  ~2.4M files in ~15s.
- **Storage modes:** the default disk mode stores source metadata in a local
  SQLite database. It streams entries into the shared matcher and resolves
  record numbers through a database index, so it has a substantially lower
  steady-state heap footprint at the cost of disk IO. Set
  `INSTANT_FS_INDEX_MODE=memory` for the fastest searches; memory mode holds
  path-keyed entries with precomputed lowercase name/path fields plus a
  record-number → path map for allocation-free matching.
  On Windows it stores the USN journal ID and cursor after each applied batch;
  on macOS it stores the FSEvents ID after each applied event. A restart replays
  from the saved checkpoint when the operating system still has that history,
  and falls back to a full scan if the checkpoint is missing or invalid.
- **Content index:** `content:` searches use a bounded cache by default. Memory
  metadata mode keeps it in RAM. Disk metadata mode keeps it temporarily in the
  same SQLite database. Disk content is bounded by
  `INSTANT_FS_CONTENT_DISK_BUDGET` and is updated by the same filesystem
  watchers as metadata.
- **USN Change Journal watcher:** after the initial scan, incremental updates
  (create / rename / delete / close) come from the journal. Starts from the
  journal tail captured before the scan (the scan is already a full snapshot).
  On journal truncation (rollover), the affected volume is re-scanned instead
  of replaying: a backlog larger than journal capacity truncates again
  mid-replay and loses records.
- **Query engine:** case-insensitive substring matching (Everything semantics),
  optional path scope / exclude paths / size & date filters / sort, with a
  bounded top-N selection so `*` on millions of matches stays sub-second.
  Disk mode adds versioned canonical path, name, and extension columns with
  conservative SQLite candidate filters for common path, type, size, and
  wildcard queries. The shared Rust matcher still verifies every candidate;
  unsupported grammar falls back to the full streamed scan.
- **Named pipe server:** per-connection request/response JSON over
  `\\.\pipe\instant-file-search-indexer`, newline-terminated responses,
  keep-alive until the client disconnects. The SYSTEM-created pipe grants
  Everyone read/write via a security descriptor so unprivileged clients work.
- **Linux backend:** the indexer runs as the systemd unit
  `instant-file-search-indexer.service` and serves the same JSON protocol over
  `/tmp/instant-file-search-indexer.sock` instead of a named pipe. Enumeration is
  a `getdents64` + `statx` walk (`walk.rs`) rather than a raw `$MFT` read, and
  change tracking is a fanotify `FAN_MARK_FILESYSTEM` watcher (`fanotify.rs`)
  rather than the USN journal. Because fanotify keeps no durable history there
  is no resumable checkpoint, so every start performs a full walk. With the
  default disk backend that walk streams entries straight into one SQLite
  transaction (`walk::scan_volume_into` -> `FileIndex::replace_streaming` ->
  `DiskIndex::replace_streaming`) instead of building a corpus-sized `Vec`,
  which is what keeps startup RSS near 10-35 MB on a 1.6-2.7 M-entry tree; the
  fanotify queue-overflow recovery keeps the collecting `build_index()` path so
  it can swap the index atomically while queries are served. The socket only
  starts listening after the walk, so the walk publishes its progress to
  `/run/instant-file-search/scan.json`, as `sd_notify(STATUS=...)` text, and as
  log heartbeats; `search_status` returns that file as `scan`. The unit sets
  `TimeoutStartSec=900` because a cold FUSE volume can make the walk outlast
  systemd's default 90 s. A `fuseblk` mount (e.g. `ntfs-3g`) cannot be marked,
  so changes there are only picked up by the next full scan.
- **macOS backend:** the indexer runs as a launchd daemon, enumerates with a
  `getattrlistbulk` walk (`walk_macos.rs`; `fileid` = `file_ref`, `crtime` =
  created), and tracks changes with an FSEvents watcher (`fsevents.rs`). Like
  Linux it is native-only (no Everything fallback) and can replay FSEvents IDs
  across restarts when the OS still has that history.
- **Service stop:** a stop flag is threaded into the serve loop and a wakeup
  client connection pokes the blocking `ConnectNamedPipe` so the service stops
  promptly.

### Disk index durability

The disk database uses SQLite WAL mode, a five second busy timeout, normal
synchronous writes, and a bounded WAL autocheckpoint. Its schema has an
explicit `user_version`. Existing unversioned databases migrate transactionally;
newer unsupported versions fail without modification.

On open, the index runs `PRAGMA quick_check(1)`. Only a confirmed SQLite
corruption or not-a-database error is quarantined. The original database and
sidecars are renamed with a timestamped `.corrupt-*` suffix, a fresh database is
created, and the normal missing-checkpoint path performs a full scan. Permission,
read-only, busy, and disk-full errors are reported without deleting or renaming
the database.

`search_status` reports the disk schema version, integrity result, WAL size, and
whether the current database was recreated after corruption. Clean service stop
runs `PRAGMA optimize` and truncates the WAL. Routine operation uses passive
checkpointing and never runs an automatic full `VACUUM`.

The portable recovery tests cover clean reopen, idempotent replay, checkpoint
ordering, schema migration, and confirmed corruption quarantine. Native USN,
FSEvents, and fanotify smoke tests still need to run on their real operating
systems with the required privileges.

## Everything engine fallback

This whole section applies only when the optional fallback engine is installed
(`-IncludeFallbackEngine`, or an existing Everything install). A default install
ships the native indexer only.

- All Everything IPC calls are synchronous and blocking, dispatched via
  `tokio::task::spawn_blocking`. The handler never holds the main thread, but
  it's still a single-threaded executor over blocking I/O.
- Everything communicates via Windows `WM_COPYDATA` IPC, native Win32
  messaging, no HTTP. The `everything-ipc` Rust crate handles it.
- Engine acquisition is a 3-tier priority: reachable window → launch installed
  GUI (bridges service-only installs) → launch bundled portable engine
  (default instance, seeded ini with `admin_service=1`).

## Transport

- The MCP server speaks stdio (rmcp `transport-io`). The plugin uses **NDJSON**
  (newline-delimited JSON), NOT Content-Length framing, important if
  modifying the transport layer.
- The plugin spawns the binary as a child process per tool call, sends MCP
  init + `tools/call` via stdin, reads the JSON response on stdout, then exits.
- The indexer's pipe protocol mirrors the MCP server's JSON semantics
  (search_status / find_files / count_files), making the native engine a
  drop-in replacement.
