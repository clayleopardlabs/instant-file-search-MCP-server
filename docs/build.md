# Build

## Prerequisite

**Everything by Voidtools must be installed and running** (system tray is fine). This server is a bridge to Everything's NTFS index — it does not include a search engine. Windows only.

## Binary

```sh
cargo build --release
```

Output: `target/release/everything-mcp-server.exe`

## Plugin (optional — OpenCode sub-agent support)

```sh
cd plugin
npm install
npm run build
```

Output: `plugin/dist/index.js`

## Plugin binary resolution

The plugin adapter finds the MCP binary by (in order):
1. `EVERYTHING_MCP_BINARY` environment variable
2. Default path relative to the plugin's `node_modules`: `../../../../target/release/everything-mcp-server.exe`
3. Falls back to `PATH` — fails if not found

If the binary path is wrong, all plugin tool calls fail silently. Set `EVERYTHING_MCP_BINARY` to point at a release build.
