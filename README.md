# Instant File Search for AI Agents

A normal file search has to go looking. That is fine for a few folders. It is miserable when an AI agent needs to reason across 30,000 files, 300,000 files, or 3 million.

This server gives the agent the shortcut.

Ask which folders contain `.env` files. Ask for every log touched today. Ask for all Python files outside virtual environments. Ask for the 100 largest files under a workspace. Ask how many PDFs, CSVs, ZIPs, screenshots, installers, exports, backups, or test files are on the machine.

The answer comes back instantly.

## How It Works

The slow way is to inspect folders one by one.

The better way is to keep a live map. Your computer already notices when files appear, disappear, move, or change names. This tool lets the agent query that current map instead of crawling the disk from scratch.

That is the trick: it is not searching harder. It is asking the right list.

## Why Agents Need It

Agents waste time just discovering what exists. Before they can edit code, inspect logs, gather documents, find configs, or understand a repo, they often burn time on recursive scans and partial guesses.

With instant file lookup, the agent can start from the real shape of your machine.

## Technical Layer

This is an MCP server for Windows file search. It lets MCP-compatible clients query local files by name, path, extension, folder, date, size, and count through structured tool calls.

The instant lookup comes from a local NTFS-based filesystem index kept current through file change notifications.

## Final Layer

Under the hood, this server connects MCP clients to Everything by Voidtools, the free Windows search utility known for millisecond filename and path searches across huge drives.

Works with VS Code, Cursor, Claude Desktop, OpenCode, and other MCP-compatible clients, with optional plugin adapter support for agents and sub-agents.

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
git clone https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server
cd instantaneous-windows-file-search-mcp-server
cargo build --release
```

Binary at `target/release/instantaneous-windows-file-search-mcp-server.exe`.

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
        "C:/absolute/path/to/instantaneous-windows-file-search-mcp-server.exe"
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
      "command": ["C:/absolute/path/to/instantaneous-windows-file-search-mcp-server.exe"],
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
2. Default: resolved relative to the plugin's `dist/` directory: `../../target/release/instantaneous-windows-file-search-mcp-server.exe`

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
│  │  instantaneous-windows-file-search-mcp-server.exe     │  │
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
