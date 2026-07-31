# Architecture

```
main.rs → handler.rs → everything.rs → everything-ipc (WM_COPYDATA) → Everything GUI
          (rmcp/stdio)  (blocking, spawn_blocking)                    (NTFS MFT index)
plugin/src/index.ts → spawns binary as subprocess → NDJSON-over-stdio
```

## Key facts

- **All Everything IPC calls are synchronous and blocking** — dispatched via `tokio::task::spawn_blocking`. The handler never holds the main thread, but it's still a single-threaded executor over blocking I/O.
- **Transport is stdio** (rmcp `transport-io`). The plugin uses **NDJSON** (newline-delimited JSON), NOT Content-Length framing — important if modifying the transport layer.
- **No CI, no workflows, no lint config.** Minimal project.
- The plugin spawns the binary as a child process per tool call, sends MCP init + `tools/call` via stdin, reads the JSON response on stdout, then exits.
- Everything communicates via Windows `WM_COPYDATA` IPC — native Win32 messaging, no HTTP.
