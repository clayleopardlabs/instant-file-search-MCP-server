# Architecture

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
  from `$DATA` (resident: value length; nonresident: valid-data-length at
  attribute header +48) — the `$FILE_NAME` attribute's metadata goes stale on
  NTFS (only refreshed on rename). Full-volume scan: ~2.4M files in ~15s.
- **Storage modes:** the default in-memory mode holds path-keyed entries with
  precomputed lowercase name/path fields plus a record-number → path map for
  allocation-free matching. Set `INSTANT_FS_INDEX_MODE=disk` to store source
  metadata in a local SQLite database instead. Disk mode streams entries into
  the same matcher and resolves record numbers through a database index, so it
  has a substantially lower steady-state heap footprint at the cost of disk IO.
  On Windows it stores the USN journal ID and cursor after each applied batch;
  on macOS it stores the FSEvents ID after each applied event. A restart replays
  from the saved checkpoint when the operating system still has that history,
  and falls back to a full scan if the checkpoint is missing or invalid.
- **USN Change Journal watcher:** after the initial scan, incremental updates
  (create / rename / delete / close) come from the journal. Starts from the
  journal tail captured before the scan (the scan is already a full snapshot).
  On journal truncation (rollover), the affected volume is re-scanned instead
  of replaying — a backlog larger than journal capacity truncates again
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

## Everything engine fallback

- All Everything IPC calls are synchronous and blocking — dispatched via
  `tokio::task::spawn_blocking`. The handler never holds the main thread, but
  it's still a single-threaded executor over blocking I/O.
- Everything communicates via Windows `WM_COPYDATA` IPC — native Win32
  messaging, no HTTP. The `everything-ipc` Rust crate handles it.
- Engine acquisition is a 3-tier priority: reachable window → launch installed
  GUI (bridges service-only installs) → launch bundled portable engine
  (default instance, seeded ini with `admin_service=1`).

## Transport

- The MCP server speaks stdio (rmcp `transport-io`). The plugin uses **NDJSON**
  (newline-delimited JSON), NOT Content-Length framing — important if
  modifying the transport layer.
- The plugin spawns the binary as a child process per tool call, sends MCP
  init + `tools/call` via stdin, reads the JSON response on stdout, then exits.
- The indexer's pipe protocol mirrors the MCP server's JSON semantics
  (search_status / find_files / count_files), making the native engine a
  drop-in replacement.
