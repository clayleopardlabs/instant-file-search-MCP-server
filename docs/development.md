# Development

## Testing

The workspace has three test suites:

- **Indexer unit tests** (`indexer/src/*.rs`, `#[cfg(test)]`): about 93 tests covering the query engine, the in-memory and SQLite index backends, disk migrations/recovery, the pipe protocol, the Linux `getdents64`+`statx` walk, and the fanotify watcher. They run without root, fanotify privileges, or an Everything install.
- **MCP server unit tests** (`src/*.rs`): the tool surface, result formatting, diagnostics, verify, and the optional Everything parity harness. The parity harness is `#[ignore]` and needs both engines available, so it never runs in a normal test pass.
- **Protocol handshake** (`tests/mcp_handshake.rs`): initializes the server over stdio and checks the advertised tool list.

```sh
cargo test --workspace
```

### Indexer benchmarks

The indexer has a synthetic benchmark that never opens or changes the user's
configured live index. Each run uses a separate process and removes its
temporary SQLite files when it finishes.

The old command form remains supported:

```sh
instant-file-search-indexer benchmark memory 500000
instant-file-search-indexer benchmark disk 500000
instant-file-search-indexer benchmark stream 500000
```

`memory` and `disk` collect the synthetic corpus into a `Vec` before inserting
and only then measure, which is what the Windows/macOS scan path still does.
`stream` builds the same corpus but feeds it through the streaming
`replace_streaming` path that the Linux initial `serve` scan uses, so comparing
`disk` with `stream` shows the scan-time memory cost of materializing the
corpus. All three modes use a temporary database and never touch the configured
live index.

For repeatable measurements with mixed file types, folders, excluded paths,
Unicode names, percentiles, reopen time, update time, process RSS, database
size, and process I/O, use the structured form:

```sh
instant-file-search-indexer benchmark synthetic --mode memory --entries 500000 --runs 10 --json
instant-file-search-indexer benchmark synthetic --mode disk --entries 500000 --runs 10 --json
```

The JSON has `schema_version: 1`. `rss_bytes_after_build` is measured after the build input is released; for `stream` there is no corpus vector, so it is simply the post-build RSS. `reopen_ms` is measured for every persistent mode (`disk` and `stream`); memory mode reports zero because it has no database to reopen. Operating system I/O counters are marked unavailable when the platform does not expose them to the process.

## Logging

- **MCP server:** controlled by the `EVERYTHING_MCP_LOG` env var (tracing-subscriber with env-filter). Unset by default, so the server logs nothing.
- **Indexer:** controlled by `RUST_LOG` via `EnvFilter::try_from_default_env()`, defaulting to `info`. In service mode this goes to the systemd journal (`journalctl -u instant-file-search-indexer`).

## Style

- Standard Rust 2021 edition
- `anyhow` for error handling
- `rmcp` for MCP transport: tools defined via `#[tool]` proc macro on handler
- `schemars` + `#[derive(JsonSchema)]` for JSON Schema generation from param structs
- No async Everything calls, no generated code
- `unsafe` is confined to platform backends where the OS API requires it: `indexer/src/fanotify.rs`, `indexer/src/walk.rs`, `indexer/src/walk_macos.rs`, `indexer/src/benchmark.rs` (RSS/stdout helpers), and `src/native.rs` (named-pipe handle handling). The MCP server itself has no unsafe outside `native.rs`.
- The `windows` crate is Windows-only: the MCP server enables `Win32_Foundation` + `Win32_System_Pipes` + `Win32_Storage_FileSystem`; the indexer enables its own feature list (pipes, security, threading, I/O, IOCTL, memory, process status).
