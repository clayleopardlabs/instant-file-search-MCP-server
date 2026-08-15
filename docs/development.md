# Development

## Testing

Tests are in `src/everything.rs` under `#[cfg(test)]`:

- **Unit tests only** — sort parsing, field parsing, timestamp formatting, attribute formatting
- **No integration tests** — Everything GUI must be running for real queries
- Passes without Everything running (unit tests don't touch IPC)

```sh
cargo test
```

### Indexer benchmarks

The indexer has a synthetic benchmark that never opens or changes the user's
configured live index. Each run uses a separate process and removes its
temporary SQLite files when it finishes.

The old command form remains supported:

```sh
instant-file-search-indexer benchmark memory 500000
instant-file-search-indexer benchmark disk 500000
```

For repeatable measurements with mixed file types, folders, excluded paths,
Unicode names, percentiles, reopen time, update time, process RSS, database
size, and process I/O, use the structured form:

```sh
instant-file-search-indexer benchmark synthetic --mode memory --entries 500000 --runs 10 --json
instant-file-search-indexer benchmark synthetic --mode disk --entries 500000 --runs 10 --json
```

The JSON has `schema_version: 1`. `rss_bytes_after_build` is measured after
the temporary input vector is dropped. `reopen_ms` is measured for disk mode;
memory mode reports zero because it has no persistent database to reopen.
Operating system I/O counters are marked unavailable when the platform does not
expose them to the process.

## Logging

Controlled by `EVERYTHING_MCP_LOG` env var (tracing-subscriber with env-filter). Unset by default — no log output.

## Style

- Standard Rust 2021 edition
- `anyhow` for error handling
- `rmcp` for MCP transport — tools defined via `#[tool]` proc macro on handler
- `schemars` + `#[derive(JsonSchema)]` for JSON Schema generation from param structs
- No unsafe blocks, no async Everything calls, no generated code
- `windows` crate used only for `Win32_Foundation` (minimal surface)
