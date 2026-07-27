# Everything MCP Server  -  Instant PC File Search

This tool lets your AI assistant find files on your computer almost instantly. Instead of making the assistant search folder by folder, which can take a very long time, it uses a free program called Everything by Voidtools that already knows where every file on your PC is.

Everything is a free Windows application that keeps track of every file and folder on your computer at all times. It does not need to scan or crawl your disk because Windows itself tells it when files change. This means the information is always up to date and ready in an instant.

For your AI assistant, this changes searching from a slow wait into a quick answer. You can ask your assistant to "find all PDF files from last week" or "count how many config files are in my projects folder" and get the result in under a second. No waiting. No slowdowns.

Works with any MCP-compatible client including VS Code, Cursor, Claude Desktop, and OpenCode (all agents and sub-agents via the optional plugin adapter).

## How it works

This MCP server gives AI agents instant filesystem search by bridging to the Everything engine (Voidtools), a Windows utility that queries the NTFS Master File Table directly. Unlike recursive directory traversal, which forces agents to enumerate every folder one at a time, Everything returns results from the already-indexed MFT in milliseconds, regardless of disk size or file count.

Everything maintains a real-time view of the NTFS filesystem by reading the Master File Table, the low-level directory structure Windows keeps on every volume. It does not crawl folders, schedule scans, or consume CPU - the NTFS driver notifies Everything of changes as they happen, so the data is always current.

For AI agents, the difference is between a bounded instant lookup and an unbounded recursive walk. Searching for *.config across a project with node_modules can stall an agent for minutes as it descends every directory and waits for I/O. With this MCP server, the same query resolves in sub-second time, and agents can compose rich filters (date ranges, size constraints, regex patterns, exclusion paths) in a single call.

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
