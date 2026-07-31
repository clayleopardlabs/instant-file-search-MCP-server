# Instant File Search MCP Server — Repository Map

```
instantaneous-windows-file-search-mcp-server/   # repo name (legacy); binary: instant-file-search-mcp-server
├── Cargo.toml              # Rust project manifest (2021 edition, rmcp + everything-ipc)
├── Cargo.lock
├── src/
│   ├── main.rs             # Entrypoint — stdio transport, tokio main
│   ├── handler.rs          # MCP tool handler (3 tools: find_files, count_files, search_status)
│   ├── tools.rs            # Param structs with JSON Schema derives (SearchParams, CountParams)
│   └── everything.rs       # Everything IPC client wrapper + engine manager (auto-launch) + unit tests
├── plugin/                 # OpenCode plugin adapter (optional — sub-agent support)
│   ├── package.json
│   ├── tsconfig.json
│   └── src/index.ts
├── vendor/everything/      # Bundled portable Everything engine (zip + Everything.ini + LICENSE)
├── scripts/
│   ├── install.ps1         # Installer: build, bundle deploy, client registration (codex/opencode/claude)
│   ├── doctor.ps1          # Diagnostics: binary, bundle, registrations
│   └── fetch-everything.ps1# Re-fetch pinned portable Everything zip (SHA256-verified)
├── docs/                   # Detailed documentation
│   ├── architecture.md
│   ├── build.md
│   ├── development.md
│   └── tools.md
└── target/                 # Build artifacts (gitignored)
```

## Engine acquisition (self-contained)

`src/everything.rs` `ensure_engine()` picks the best available search engine on first use:
1. Already-running Everything (IPC window reachable) — used as-is, zero extra RAM
2. Installed Everything (no window; e.g. service-only) — launches its GUI, which connects to the service
3. Bundled portable engine (`<binary dir>\everything\Everything.exe`) — launched as the DEFAULT instance (not a named instance; the Everything service only serves the default instance), `-startup` tray mode, `admin_service=1` ini, waits for DB load (timeout: `EVERYTHING_ENGINE_TIMEOUT_SECS`, default 60)

`search_status` reports `engine_source`: existing / installed_launched / bundled / none.

## Environment variables

- `EVERYTHING_MCP_LOG` — tracing filter (stderr; never stdout, to keep NDJSON clean)
- `EVERYTHING_ENGINE_EXE` — override bundled engine path
- `EVERYTHING_ENGINE_TIMEOUT_SECS` — engine DB-load wait in seconds (default 60)
- Plugin: `INSTANT_FS_MCP_BINARY` (legacy: `EVERYTHING_MCP_BINARY`) — override MCP binary path

## Build notes

- Rust target pinned `x86_64-pc-windows-gnu`; needs mingw-w64 on PATH — rustup's self-contained `dlltool.exe` wrapper is broken (CreateProcess error). Use WinLibs (winget: BrechtSanders.WinLibs.POSIX.UCRT) and prepend `mingw64\bin` to PATH before `cargo build`.
- Everything portable zip is SHA256-pinned; re-fetch with `scripts/fetch-everything.ps1`.

See `docs/` for architecture, build instructions, tool reference, and development notes.
