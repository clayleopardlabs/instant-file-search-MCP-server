# Everything MCP Server — Instant PC File Search

An [MCP](https://modelcontextprotocol.io) server that exposes the [Everything](https://www.voidtools.com) search engine (NTFS index) as AI-agent tools. Provides sub-millisecond file searches across your entire filesystem.

## Installation

### Requirements

- [Everything](https://www.voidtools.com) desktop app (v1.5 or later) — must be running (background, no GUI needed)
- [Rust toolchain](https://rustup.rs) to build the binary
- Node.js 18+ (only for the OpenCode plugin adapter)

### Build

```sh
git clone https://github.com/clayleopardlabs/everything-mcp-server
cd everything-mcp-server
cargo build --release
```

Binary at `target/release/everything-mcp-server.exe` (Windows) or `target/release/everything-mcp-server` (Linux/Mac).

### MCP Host (VS Code, Cursor, Claude Desktop, etc.)

Add to your MCP client config:

```json
{
  "mcpServers": {
    "everything": {
      "type": "local",
      "command": [
        "/absolute/path/to/everything-mcp-server.exe"
      ],
      "enabled": true
    }
  }
}
```

Available immediately. Tools surface in the main session only — sub-agents (explore, librarian) do not inherit MCP tools.

### OpenCode (Plugin — Sub-Agent Support)

For [OpenCode](https://opencode.ai) users, install the **plugin adapter** to make these tools available to all agents including sub-agents:

```sh
# 1. Build the plugin
cd plugin
npm install
npm run build

# 2. Deploy to OpenCode
mkdir -p ~/.config/opencode/plugins/everything-mcp-plugin/dist
cp dist/* ~/.config/opencode/plugins/everything-mcp-plugin/dist/

# 3. Register in opencode.json
```

Add to your `~/.config/opencode/opencode.json` `plugin` array:

```json
"file:///C:/Users/YOU/.config/opencode/plugins/everything-mcp-plugin/dist/index.js"
```

**You can keep the MCP entry too** — both can coexist. The plugin is a thin TypeScript bridge that spawns the same MCP binary internally. Every other MCP host uses the binary directly.

## Usage

| Tool | Description |
|------|-------------|
| `find_files` | Search files by name with wildcards, regex, path filter, sort, pagination, and field selection |
| `count_files` | Instant count of matching files without transferring file data |
| `search_status` | Check if Everything engine is connected and working |

### Search modifiers

The `query` parameter supports Everything's built-in modifiers:

| Modifier | Example | Effect |
|----------|---------|--------|
| `file:` | `file:*.ts` | Files only (not directories) |
| `folder:` | `folder:src` | Directories only |
| `dm:` | `dm:today` `dm:2days` `dm:2026-01-15` | Date modified filter |
| `dc:` | `dc:thisweek` | Date created filter |
| `da:` | `da:yesterday` | Date accessed filter |
| `size:` | `size:>10mb` `size:1kb..1mb` | File size filter |
| `dupe:` | `dupe:filename` | Find duplicate filenames |
| `!` | `!*.tmp` | NOT operator (exclude pattern) |
| `\|` | `*.ts \| *.tsx` | OR operator |
| `" "` | `"exact phrase"` | Literal search |
| `regex:` | `^app.*\.ts$` | Regex pattern (use `regex=true` param instead) |

### Parameters

**find_files:**
| Param | Type | Description |
|-------|------|-------------|
| `query` | string (required) | Search query with Everything modifiers |
| `path` | string | Scope to directory (e.g. `B:\\Projects`) |
| `exclude_path` | string | Exclude paths (e.g. `node_modules,.git`) |
| `include_all` | boolean | Disable auto-exclusion of noise folders |
| `regex` | boolean | Enable regex parsing |
| `match_case` | boolean | Case-sensitive search |
| `match_whole_word` | boolean | Whole word match |
| `match_path` | boolean | Match against full path |
| `max_results` | number (max 100) | Results per page |
| `offset` | number | Pagination offset |
| `sort` | string | Sort order (22 options) |
| `fields` | string | Comma-separated field names (12 fields) |

**count_files:**
| Param | Type | Description |
|-------|------|-------------|
| `query` | string (required) | Search query |
| `path` | string | Scope to directory |
| `exclude_path` | string | Exclude paths |
| `include_all` | boolean | Disable auto-exclusion |
| `regex` | boolean | Enable regex |
| `match_case` | boolean | Case-sensitive |
| `match_whole_word` | boolean | Whole word match |

### Response format

```json
{
  "results": [
    {
      "filename": "example.ts",
      "path": "B:\\Projects\\my-app\\src",
      "size": 2048,
      "date_modified": "2026-07-07T15:30:00Z",
      "date_created": "2026-01-15T10:00:00Z",
      "date_accessed": "2026-07-07T16:00:00Z",
      "attributes": "A",
      "extension": ".ts",
      "run_count": null,
      "date_run": null
    }
  ],
  "total": 100,
  "returned": 2,
  "offset": 0,
  "note": ""
}
```

## Architecture

```
┌─────────────────────────────────────┐
│  MCP Host (VS Code / Cursor / CD)   │
│  ┌───────────────────────────────┐  │
│  │  everything-mcp-server.exe     │  │
│  │  (Rust, everything-ipc)        │  │
│  └───────────────────────────────┘  │
├─────────────────────────────────────┤
│  OpenCode (main + sub-agents)      │
│  ┌───────────────────────────────┐  │
│  │  plugin/dist/index.js          │  │
│  │  (spawns binary → MCP proxy)   │  │
│  └───────────────────────────────┘  │
└─────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│  Everything IPC (named pipe)        │
│  Everything Desktop App (NTFS MFT)  │
└─────────────────────────────────────┘
```

## License

MIT
