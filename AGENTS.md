# Instant File Search MCP Server — Repository Map

## Installation acceptance criteria

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

```
instant-file-search-MCP-server/             # repo name; binary: instant-file-search-mcp-server
├── Cargo.toml              # Cargo workspace (MCP server + indexer crate)
├── Cargo.lock
├── src/
│   ├── main.rs             # Entrypoint — stdio transport, tokio main
│   ├── handler.rs          # MCP tool handler (3 tools: find_files, count_files, search_status) + native-first routing
│   ├── tools.rs            # Param structs with JSON Schema derives (SearchParams, CountParams)
│   ├── everything.rs       # Everything IPC client wrapper + engine manager (auto-launch) + unit tests
│   └── native.rs           # Native indexer pipe client (byte-mode reads, newline-terminated responses)
├── indexer/                # Native NTFS indexer crate (binary: instant-file-search-indexer.exe)
│   ├── src/main.rs         # `serve` (pipe server + scan + USN watch) / `scan` (one-shot diagnostic); SCM service host
│   ├── src/mft.rs          # Raw $MFT stream parser (USA fixup; times from $STANDARD_INFORMATION, size from $DATA)
│   ├── src/usn.rs          # USN Change Journal watcher (tail-start; re-scan on truncation)
│   ├── src/index.rs        # Path-keyed in-memory index + record-number→path ref map
│   ├── src/query.rs        # Query engine (substring CI, scope/excludes/filters/sort, bounded top-N)
│   ├── src/pipe.rs         # Named pipe server (Everyone DACL, keep-alive, newline-framed JSON)
│   ├── src/scan.rs         # Scan orchestration
│   └── src/sector_reader.rs# 4096-byte aligned volume reader (raw volume handles reject unaligned reads)
├── plugin/                 # OpenCode plugin adapter (optional — sub-agent support)
│   ├── package.json
│   ├── tsconfig.json
│   └── src/index.ts
├── vendor/everything/      # Bundled portable Fallback Engine (zip + ini + LICENSE)
├── scripts/
│   ├── install.ps1         # Installer: build, bundle deploy, indexer service registration, client registration
│   ├── doctor.ps1          # Diagnostics: binary, bundle, service, registrations
│   └── fetch-everything.ps1# Re-fetch pinned portable Fallback Engine zip (SHA256-verified)
├── docs/                   # Detailed documentation
│   ├── architecture.md
│   ├── build.md
│   ├── development.md
│   └── tools.md
└── target/                 # Build artifacts (gitignored)
```

## Engine routing (native-first)

`src/handler.rs` routes search/count through `src/native.rs` when the native
indexer's named pipe (`\\.\pipe\instant-file-search-indexer`) is reachable;
otherwise it falls back to the Everything engine via `src/everything.rs`.

`search_status` reports BOTH engines: native (indexed count, volumes) and
everything (engine_source: existing / installed_launched / bundled / none).

## Native indexer service

- SCM service name: `instant-file-search-indexer` (auto-start). Console modes:
  `serve` (pipe + scan + USN watch) and `scan` (one-shot diagnostic).
- Requires admin/SYSTEM to open `\\.\C:` volume devices for the MFT scan.
- Deployed at `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\indexer\`.

## Everything engine acquisition (fallback, self-contained)

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

- Rust target pinned `x86_64-pc-windows-gnu`; needs mingw-w64 on PATH — rustup's self-contained `dlltool.exe` wrapper is broken (CreateProcess error). Use WinLibs (winget: BrechtSanders.WinLibs.POSIX.UCRT) and prepend `mingw64\bin` to PATH before `cargo build`.
- Everything portable zip is SHA256-pinned; re-fetch with `scripts/fetch-everything.ps1`.

## Known indexer pitfalls (verified 2026-08-01)

- NTFS `$FILE_NAME` attribute metadata goes stale (timestamps/sizes only refresh on rename) — scan reads times from `$STANDARD_INFORMATION`, size from `$DATA` valid-data-length.
- USN reason constants: DELETE=0x200 (NOT 0x2, which is DATA_EXTEND), RENAME=0x1000|0x2000, HARD_LINK_CHANGE=0x10000, CLOSE=0x80000000, CREATE=0x100.
- Named pipes distribute across ALL listeners — a stale duplicate `serve` process steals queries (empty results from an empty index). Check for leftovers before debugging.
- When redeploying for diagnostics, verify the deployed exe timestamp matches the fresh build (copy errors get hidden by `>nul` redirects).
- Never run the indexer in the foreground — it blocks until timeout; use fire-and-forget elevated starts with output redirected to files.

See `docs/` for architecture, build instructions, tool reference, and development notes.
