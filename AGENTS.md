# Instantaneous Windows File Search MCP Server — Repository Map

```
instantaneous-windows-file-search-mcp-server/
├── Cargo.toml              # Rust project manifest (2021 edition, rmcp + everything-ipc)
├── Cargo.lock
├── src/
│   ├── main.rs             # Entrypoint — stdio transport, tokio main
│   ├── handler.rs          # MCP tool handler (3 tools: find_files, count_files, search_status)
│   ├── tools.rs            # Param structs with JSON Schema derives (SearchParams, CountParams)
│   └── everything.rs       # Everything IPC client wrapper + unit tests
├── plugin/                 # OpenCode plugin adapter (optional — sub-agent support)
│   ├── package.json
│   ├── tsconfig.json
│   └── src/index.ts
├── docs/                   # Detailed documentation
│   ├── architecture.md
│   ├── build.md
│   ├── development.md
│   └── tools.md
└── target/                 # Build artifacts (gitignored)
```

See `docs/` for architecture, build instructions, tool reference, and development notes.
