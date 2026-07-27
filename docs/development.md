# Development

## Testing

Tests are in `src/everything.rs` under `#[cfg(test)]`:

- **Unit tests only** — sort parsing, field parsing, timestamp formatting, attribute formatting
- **No integration tests** — Everything GUI must be running for real queries
- Passes without Everything running (unit tests don't touch IPC)

```sh
cargo test
```

## Logging

Controlled by `EVERYTHING_MCP_LOG` env var (tracing-subscriber with env-filter). Unset by default — no log output.

## Style

- Standard Rust 2021 edition
- `anyhow` for error handling
- `rmcp` for MCP transport — tools defined via `#[tool]` proc macro on handler
- `schemars` + `#[derive(JsonSchema)]` for JSON Schema generation from param structs
- No unsafe blocks, no async Everything calls, no generated code
- `windows` crate used only for `Win32_Foundation` (minimal surface)
