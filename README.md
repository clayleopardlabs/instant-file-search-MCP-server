# Instant File Search for AI Agents

A normal file search has to go looking.

That is fine when you are searching one folder. It is not fine when an AI agent needs to reason across 30,000 files, 300,000 files, or 3 million. A simple question like "where are the logs?" or "which projects have config files?" can turn into a slow crawl through the filesystem.

This server gives the agent the shortcut.

Ask for every `.log` file modified today. Ask which folders contain `.env` files. Ask for all Python files outside virtual environments. Ask for the 100 largest files under a workspace. Ask how many PDFs, CSVs, ZIPs, screenshots, installers, exports, backups, or test files are on the machine.

The answer comes back instantly.

## The Simple Idea

Imagine trying to find a book in a library by walking down every aisle and reading every spine.

That works, but it is the stupid way to do it if the library already has a catalog.

File search has the same problem. The slow method starts at a folder, opens it, checks what is inside, then repeats that for every folder underneath it. That is called crawling the filesystem. It is simple, but it repeats work the computer has already done.

The better method is to ask the catalog.

Your computer already keeps records of files: their names, locations, sizes, and modification times. When files are created, renamed, moved, or deleted, those changes are tracked. So the trick is not to make the agent search harder. The trick is to let it query a current map of the filesystem.

## The Real Mechanism

On Windows, NTFS stores file records in a structure called the Master File Table. Instead of opening folder after folder, a search tool can read that file table and build an index of names and paths.

Then it watches for filesystem changes. When a file appears, moves, gets renamed, or disappears, the index updates. So each new search does not begin from zero. It begins from a live index that already knows what exists.

That is why a query over millions of files can return in milliseconds. It is not magic. It is the difference between walking the shelves and asking the catalog.

## What This Server Adds

This project turns that instant local file index into an MCP tool.

An MCP-compatible AI client can ask structured questions about local files: name, path, extension, folder, date modified, size, pattern, and count. The agent does not need to run slow recursive shell commands or guess from partial directory listings. It can ask the filesystem what exists and get a usable answer immediately.

That changes agent workflows. Before reading code, the agent can map the project. Before editing configs, it can find every relevant config file. Before summarizing a client folder, it can gather the documents. Before touching a repo, it can separate source files from dependencies, build output, logs, and junk.

## The Final Layer

Under the hood, this server connects MCP clients to Everything by Voidtools.

Everything is a free Windows search utility known for searching huge NTFS drives in milliseconds. This server exposes that capability to AI assistants, coding agents, and sub-agents through MCP.

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
