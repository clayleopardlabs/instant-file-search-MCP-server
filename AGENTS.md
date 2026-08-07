# Instant File Search MCP Server — Repository Map

## Installation acceptance criteria

### Windows

When asked to install this MCP server, use `scripts/install.ps1` and complete the
full Windows installation. Do not silently substitute the Everything fallback for
the native indexer service.

An installation is complete only when all of these are true:

1. The MCP server is registered with the requested host.
2. The `instant-file-search-indexer` Windows service is installed, set to
   auto-start, and running.
3. The service command points at the active version recorded in `current.json`.
4. `scripts/doctor.ps1 -RequireNative` succeeds.

If elevation is required, request UAC approval and wait for it. If policy or
permissions prevent the native service from being installed, report the install
as incomplete and identify that exact blocker. Fallback mode is valid only when
the user explicitly asks for it.

### Linux

When asked to install on Linux, use `scripts/install-linux.sh`. An installation
is complete only when all of these are true:

1. The MCP server binary is installed at `/usr/local/lib/instant-file-search/instant-file-search-mcp-server`.
2. The indexer binary is installed at `/usr/local/lib/instant-file-search/instant-file-search-indexer`.
3. The systemd unit `instant-file-search-indexer.service` is installed, enabled, and running.
4. The Unix socket `/tmp/instant-file-search-indexer.sock` exists and accepts connections.
5. The service has indexed all mounted volumes (check via `search_status`).

The indexer requires root or `CAP_SYS_ADMIN` + `CAP_DAC_OVERRIDE` +
`CAP_DAC_READ_SEARCH` to scan the full filesystem via fanotify and open
directories with restrictive permissions (e.g. `drwxr-x---` home dirs).

## Repository layout

```
instant-file-search-MCP-server/             # repo name; binary: instant-file-search-mcp-server
├── Cargo.toml              # Cargo workspace (MCP server + indexer crate)
├── Cargo.lock
├── src/
│   ├── main.rs             # Entrypoint — stdio transport, tokio main
│   ├── handler.rs          # MCP tool handler (5 tools) + native-first routing
│   ├── tools.rs            # Param structs with JSON Schema derives (SearchParams, CountParams, AggregateParams, RecentChangesParams)
│   ├── everything.rs       # Everything IPC client wrapper + engine manager (auto-launch); Windows-only, #[cfg(windows)]
│   ├── native.rs           # Native indexer pipe client — Windows named pipe + Linux Unix socket
│   └── results.rs          # Shared result types, FILETIME→ISO conversion, attribute formatting
├── indexer/                # Native indexer crate (binary: instant-file-search-indexer)
│   ├── src/main.rs         # `serve` (pipe/socket + scan + watcher) / `scan` (one-shot diagnostic)
│   ├── src/platform.rs     # Platform seam — re-exports platform-specific modules + path helpers
│   ├── src/index.rs        # Path-keyed in-memory index + record-number→path ref map
│   ├── src/query.rs        # Query engine (substring CI, scope/excludes/filters/sort, bounded top-N)
│   ├── src/types.rs        # IndexedFile struct (separator-agnostic, FILETIME epoch)
│   ├── src/content.rs      # Content store for content: query prefix
│   ├── src/protocol.rs     # IPC request/response protocol
│   ├── src/scan.rs         # Scan orchestration (delegates to platform)
│   ├── src/mft.rs          # Windows: Raw $MFT stream parser
│   ├── src/usn.rs          # Windows: USN Change Journal watcher
│   ├── src/pipe.rs         # Windows: Named pipe server
│   ├── src/sector_reader.rs# Windows: 4096-byte aligned volume reader
│   ├── src/walk.rs         # Linux: getdents64 + statx directory walker
│   ├── src/fanotify.rs     # Linux: fanotify FID-mode change watcher
│   ├── src/pipe_unix.rs    # Linux/macOS: Unix socket server
│   ├── src/walk_macos.rs   # macOS: getattrlistbulk directory walker
│   └── src/fsevents.rs     # macOS: FSEvents change watcher
├── plugin/                 # OpenCode plugin adapter (optional — sub-agent support)
│   ├── package.json
│   ├── tsconfig.json
│   └── src/index.ts
├── vendor/everything/      # Bundled portable Fallback Engine (zip + ini + LICENSE); Windows-only
├── scripts/
│   ├── install.ps1         # Windows installer: build, bundle deploy, indexer service registration
│   ├── install-linux.sh    # Linux installer: build, systemd unit, client registration
│   ├── doctor.ps1          # Windows diagnostics: binary, bundle, service, registrations
│   └── fetch-everything.ps1# Re-fetch pinned portable Fallback Engine zip (SHA256-verified)
├── docs/                   # Detailed documentation
│   ├── architecture.md
│   ├── build.md
│   ├── development.md
│   └── tools.md
└── target/                 # Build artifacts (gitignored)
```

## MCP tools

The server exposes 5 tools over stdio (JSON-RPC). All tools route through the
native indexer first; on Windows, `find_files` and `count_files` fall back to
the Everything engine if the native indexer is unreachable.

| Tool | Purpose |
|------|---------|
| `find_files` | Search by name, extension, content, date, size. Returns paths, names, sizes, dates. |
| `count_files` | Count matches without returning file data. Use before `find_files` for broad patterns. |
| `aggregate_files` | Disk usage stats: total count, total size, per-extension breakdown, largest files. |
| `recent_changes` | Recently created/modified/renamed/deleted files with timestamps. |
| `search_status` | Engine health: indexed file count, volumes, native/everything availability. |

### Common query patterns

- `*.rs` — all Rust source files
- `Cargo.toml` — exact filename match
- `*.md path:/home/user/project` — scoped to a directory
- `*.log path:C:/Windows` — Windows path (forward slashes preferred to avoid JSON escaping)
- `content:"fn main"` — content search (bounded 256MB store; targeted, not exhaustive)
- `dm:lastweek` — modified in the last week
- `size:>10mb` — larger than 10 MB
- `*.tmp | *.bak` — match either pattern
- `!node_modules` — exclude a directory
- `sort=size` — sort by size (largest first)

### Key parameters

- `query` — search pattern (wildcards, content:, date:, size:, exclude with !)
- `path` — directory scope (use forward slashes)
- `max_results` — cap result count (0 = default 100)
- `offset` — skip N results for pagination
- `sort` — `name`, `name_desc`, `size`, `date_modified`, `date_created`, `extension`, etc.
- `include_all` — include `node_modules`, `.git`, `WinSxS` (default false)

## Engine routing (native-first)

`src/handler.rs` routes search/count through `src/native.rs` when the native
indexer is reachable; otherwise it falls back to the Everything engine via
`src/everything.rs` (Windows only).

- **Windows**: named pipe at `\\.\pipe\instant-file-search-indexer`
- **Linux**: Unix socket at `/tmp/instant-file-search-indexer.sock`
- **macOS**: Unix socket at `/tmp/instant-file-search-indexer.sock`

`search_status` reports both engines: native (indexed count, volumes) and
everything (engine_source: existing / installed_launched / bundled / none).

## Native indexer service

### Windows

- SCM service name: `instant-file-search-indexer` (auto-start). Console modes:
  `serve` (pipe + scan + USN watch) and `scan` (one-shot diagnostic).
- Requires admin/SYSTEM to open `\\.\C:` volume devices for the MFT scan.
- Deployed at `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\indexer\`.

### Linux

- systemd unit: `instant-file-search-indexer.service` (Type=notify, enabled).
- Requires root or `CAP_SYS_ADMIN` + `CAP_DAC_OVERRIDE` + `CAP_DAC_READ_SEARCH`.
- Unix socket at `/tmp/instant-file-search-indexer.sock` (mode 0666).
- Scans all real-disk volumes (parsed from `/proc/self/mounts`, ext4/xfs/btrfs/vfat/etc.).
- fanotify watches all mounted volumes for live change tracking.

### macOS

- launchd daemon: `com.clayleopardlabs.instant-file-search`.
- Requires Full Disk Access in System Settings for protected directories.
- `getattrlistbulk` walker + FSEvents change tracking.

## Everything engine acquisition (fallback, Windows-only)

`src/everything.rs` `ensure_engine()` picks the best available search engine on first use:
1. Already-running Everything (IPC window reachable) — used as-is, zero extra RAM
2. Installed Everything (no window; e.g. service-only) — launches its GUI, which connects to the service
3. Bundled portable engine (`<binary dir>\everything\Everything.exe`) — launched as the DEFAULT instance (not a named instance; the Everything service only serves the default instance), `-startup` tray mode, `admin_service=1` ini, waits for DB load (timeout: `EVERYTHING_ENGINE_TIMEOUT_SECS`, default 60)

## Environment variables

- `EVERYTHING_MCP_LOG` — tracing filter (stderr; never stdout, to keep NDJSON clean)
- `EVERYTHING_ENGINE_EXE` — override bundled engine path
- `EVERYTHING_ENGINE_TIMEOUT_SECS` — engine DB-load wait in seconds (default 60)
- Plugin: `INSTANT_FS_MCP_BINARY` (legacy: `EVERYTHING_MCP_BINARY`) — override MCP binary path

## Build notes

### Windows

Rust target pinned `x86_64-pc-windows-gnu`; needs mingw-w64 on PATH — rustup's
self-contained `dlltool.exe` wrapper is broken (CreateProcess error). Use WinLibs
(winget: BrechtSanders.WinLibs.POSIX.UCRT) and prepend `mingw64\bin` to PATH
before `cargo build`. Everything portable zip is SHA256-pinned; re-fetch with
`scripts/fetch-everything.ps1`.

### Linux

Default target (`x86_64-unknown-linux-gnu`) works out of the box. The indexer
crate has Linux-specific dependencies: `rustix`, `nix` (fanotify), `sd-notify`.
Build with `cargo build --release` from the workspace root.

## Known indexer pitfalls (verified 2026-08-01)

- NTFS `$FILE_NAME` attribute metadata goes stale (timestamps/sizes only refresh on rename) — scan reads times from `$STANDARD_INFORMATION`, size from `$DATA` valid-data-length.
- USN reason constants: DELETE=0x200 (NOT 0x2, which is DATA_EXTEND), RENAME=0x1000|0x2000, HARD_LINK_CHANGE=0x10000, CLOSE=0x80000000, CREATE=0x100.
- Named pipes distribute across ALL listeners — a stale duplicate `serve` process steals queries (empty results from an empty index). Check for leftovers before debugging.
- When redeploying for diagnostics, verify the deployed exe timestamp matches the fresh build (copy errors get hidden by `>nul` redirects).
- Never run the indexer in the foreground — it blocks until timeout; use fire-and-forget elevated starts with output redirected to files.
- Linux: `CapabilityBoundingSet=CAP_SYS_ADMIN` alone strips `CAP_DAC_OVERRIDE`/`CAP_DAC_READ_SEARCH`, preventing traversal of `drwxr-x---` directories. The systemd unit must include all three capabilities.
- Linux: path separator in index.rs must use `crate::platform::SEP` — hardcoded `\\` breaks `adjust_ancestors`, `sum_children`, `remove_prefix`, `rename_prefix`.

See `docs/` for architecture, build instructions, tool reference, and development notes.
