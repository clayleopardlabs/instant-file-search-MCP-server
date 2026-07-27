# Everything MCP Server  -  Instant PC File Search

Have you ever needed to find a file on your computer and waited forever while Windows Search churns through folders? Or opened every folder one by one trying to remember where you saved something? This tool solves that.

The Everything MCP Server is a bridge between AI agents and a free Windows program called Everything (by Voidtools). Everything works differently than normal search - it reads the NTFS index that Windows already keeps on your hard drive. This means it finds files in milliseconds, not minutes. No crawling through folders, no waiting for indexing to catch up.

Without this tool, your AI agents have to search for files the slow way - by asking the computer to list every folder and file one at a time, like a person manually opening drawers in a filing cabinet. With this tool, agents can find any file on your PC instantly by name, type, location, date, or size.

For example, instead of telling your agent "look for the config file in the projects folder" and waiting while it lists every directory, you can just say "find the config file from last week" and get the answer in a second.

The best part: Everything is already running on millions of Windows PCs. If you have it installed, you already have the index. This server just opens it up to your AI agents.

Works with any MCP-compatible client (VS Code, Cursor, Claude Desktop) and with all agents in OpenCode - including sub-agents.

## Prerequisites

**You must have [Everything](https://www.voidtools.com) by Voidtools installed and running on your PC.** This tool is a bridge to Everything's NTFS index  -  it does not include a search engine itself.

- Everything v1.5 or later (the "alpha" branch supports the HTTP/JSON API, but this server uses the IPC interface available in all versions)
- Everything must be running (can run minimized to system tray, no GUI window needed)
- Works on Windows only (Everything indexes the NTFS Master File Table)

## Who needs what

| You are using... | You need | Why |
|-----------------|----------|-----|
| VS Code, Cursor, Claude Desktop, or any MCP client | **MCP binary only** | MCP hosts load tools directly via binary |
| OpenCode (main agent only) | **MCP binary** (configured as MCP server in opencode.json) | Main session gets tools via MCP |
| OpenCode (sub-agents  -  explore, librarian, task workers) | **MCP binary + Plugin adapter** | Sub-agents don't inherit MCP tools; the plugin makes them available to all agents |

**The plugin adapter is OPTIONAL.** If you only need tools in your main chatting session, just configure the MCP server. You only need the plugin if you also want sub-agents (explore, librarian, task workers) to access Everything search.

**OpenCode users need both MCP + plugin** if they want search everywhere. The plugin spawns the binary just like an MCP host would  -  the binary is always required.

## Build

### 1. Build the MCP Binary (Required for Everyone)

You need the [Rust toolchain](https://rustup.rs):

```sh
git clone https://github.com/clayleopardlabs/everything-mcp-server
cd everything-mcp-server
cargo build --release
```

Binary at `target/release/everything-mcp-server.exe`.

### 2. Build the Plugin Adapter (OpenCode Sub-Agent Support  -  Optional)

You need Node.js 18+:

```sh
cd plugin
npm install
npm run build
```

Output at `plugin/dist/index.js`.

## Configure

### For MCP Hosts (VS Code, Cursor, Claude Desktop, etc.)

Add to your MCP client configuration:

```json
{
  "mcpServers": {
    "everything": {
      "type": "local",
      "command": [
        "C:/absolute/path/to/everything-mcp-server.exe"
      ],
      "enabled": true
    }
  }
}
```

**Tools surface in the main conversation only.** Sub-agents do not inherit MCP tools  -  that limitation is per-host, not from this server.

### For OpenCode  -  MCP Server (Main Agent Only)

Add to your `~/.config/opencode/opencode.json` under the `mcp` key:

```json
{
  "mcp": {
    "everything": {
      "command": ["C:/absolute/path/to/everything-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

### For OpenCode  -  Plugin (Sub-Agent Support)

Deploy the plugin adapter so it's available to all agents:

```sh
# Deploy dist + dependencies to OpenCode's plugin directory
mkdir -p ~/.config/opencode/plugins/everything-mcp-plugin
cp -r plugin/dist ~/.config/opencode/plugins/everything-mcp-plugin/
cp -r plugin/node_modules ~/.config/opencode/plugins/everything-mcp-plugin/
cp plugin/package.json ~/.config/opencode/plugins/everything-mcp-plugin/
```

Register in the `plugin` array of `~/.config/opencode/opencode.json`:

```json
"file:///C:/Users/YOU/.config/opencode/plugins/everything-mcp-plugin/dist/index.js"
```

**You can keep the MCP entry AND add the plugin**  -  they coexist. The plugin spawns the same MCP binary. The MCP entry is used by the main session; the plugin makes tools available to all sub-agents.

### Plugin Lookup Order

The plugin adapter locates the MCP binary by:

1. `EVERYTHING_MCP_BINARY` environment variable (if set)
2. Default: resolved relative to the plugin's `dist/` directory: `../../target/release/everything-mcp-server.exe`

If neither resolves, the plugin will fail.

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
| `exclude_path` | string | Exclude paths (e.g. `node_modules;.git`) |
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
| `exclude_path` | string | Exclude paths (e.g. `node_modules;.git`) |
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
