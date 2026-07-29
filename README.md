# Instant File Search for AI Agents

![](demo.gif)

## Purpose

This tool gives an AI assistant fast access to the files on your computer.

It can find files by name, extension, date, size, or location. It can count files, show file paths, and search inside folders. It gives the assistant immediate visibility into the files on your machine.

It answers in milliseconds, not seconds. It does not search folder by folder.

## How It Works

A normal file search starts at a folder. The tool opens the folder and looks at the items inside. It opens each subfolder and repeats the process. This is simple but slow.

The computer already keeps a record of the files on each drive. When you create a file, the computer adds it to the record. When you rename or delete a file, the computer changes the record. This record is the NTFS Master File Table.

This tool runs a search directly in that record. It does not walk through the directory tree. It sends one query and gets all the matching results.

This is why the search is fast. The answer is already in a list that the computer keeps up to date.

## Capabilities

You can use this tool for these types of searches:

* "Show me what changed in this project since yesterday."
* "Find files that look like secrets or local config."
* "List source files but ignore dependencies and build output."
* "Count how many test files exist beside implementation files."
* "Find old exports, duplicate downloads, or forgotten installers."
* "Give me the project's shape before reading the code."

## Technical Details

This server connects an MCP-compatible AI client to a high-speed file index.

It uses a free utility called Everything. Everything keeps an index of the NTFS Master File Table in RAM. It uses about 0.5 GB to 1 GB, based on the size of your disks. It stays current through filesystem change notifications. It does not crawl directories for each search.

Queries that normally need recursive searches are answered in milliseconds.

The server gives this capability to AI assistants and coding agents. The agent can query the local filesystem through structured tool calls. It does not need slow shell commands, repeated directory walks, or manual search loops.

The server works with any MCP-compatible client. This includes VS Code, Cursor, Claude Desktop, and OpenCode. It has optional agent and sub-agent support through a plugin adapter.

Two additional tools come with the same binary:

- `count_files` returns only the number of matches, without transferring file data. Call this first when you are not sure whether a pattern matches ten files or ten million.
- `search_status` reports whether Everything is running, the IPC channel is responding, and the database is loaded. Call this when the other tools fail unexpectedly to find out whether the engine is the problem.

## One-Command Install

```powershell
powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server/master/scripts/install.ps1 | iex"
```

This installs Everything by Voidtools (via winget) and downloads the latest pre-built binary. No Rust toolchain needed.

## What You Need

**Everything by Voidtools** must be installed and running. This project is a bridge, not a search engine. Everything is free, runs on any modern Windows machine, and works quietly from the system tray. Version 1.5 or later, any edition. Windows only - Everything indexes the NTFS Master File Table, which does not exist on other platforms.

Everything can be installed silently:

```powershell
winget install voidtools.Everything --accept-source-agreements --silent
```

Or download the installer directly: [Everything-1.5.0.1418b.x64-Setup.exe](https://www.voidtools.com/Everything-1.5.0.1418b.x64-Setup.exe)

Everything must be running for the IPC channel to work. It can be minimized to the system tray; no GUI window needs to be visible.

## Two Pieces, One Binary

**The MCP binary** (Rust, single `.exe`) is all you need for VS Code, Cursor, Claude Desktop, or any MCP host. Point the host at this binary and the three tools appear in the agent's toolbox.

**The plugin adapter** (TypeScript, optional) is only needed for OpenCode users who want sub-agents (explore, librarian, task workers) to access the same tools. Sub-agents do not inherit MCP tools automatically. The plugin bridges that gap by spawning the binary as a child process.

## Quick Start (build from source)

```sh
git clone https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server
cd instantaneous-windows-file-search-mcp-server
cargo build --release
```

Binary at `target/release/instantaneous-windows-file-search-mcp-server.exe`.

The project pins the `x86_64-pc-windows-gnu` Rust target by default, so you only need [MSYS2/MinGW-w64](https://www.msys2.org/) (install via `winget install MSYS2.MSYS2`) instead of the full Visual Studio Build Tools. If you prefer MSVC, override the toolchain in `rust-toolchain.toml` or pass `--target x86_64-pc-windows-msvc`.

Add to your MCP client configuration:

```json
{
  "mcpServers": {
    "everything": {
      "type": "local",
      "command": ["C:/full/path/to/instantaneous-windows-file-search-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

Restart the client. Three tools appear in the agent's tool list: `find_files`, `count_files`, `search_status`.

## Who Needs What

| You use... | You need | Why |
|------------|----------|-----|
| VS Code, Cursor, Claude Desktop, or any MCP host | MCP binary only | The host loads tools directly |
| OpenCode main session | MCP binary as MCP server | Configured under `mcp` key in opencode.json |
| OpenCode sub-agents too | MCP binary + plugin adapter | Sub-agents do not inherit MCP tools |

## Build

### MCP binary (required)

Requires the [Rust toolchain](https://rustup.rs):

```sh
cargo build --release
```

Output: `target/release/instantaneous-windows-file-search-mcp-server.exe`

### Plugin adapter (optional)

Requires Node.js 18+:

```sh
cd plugin
npm install
npm run build
```

Output: `plugin/dist/index.js`

## Configure

### MCP hosts (VS Code, Cursor, Claude Desktop)

```json
{
  "mcpServers": {
    "everything": {
      "type": "local",
      "command": ["C:/path/to/instantaneous-windows-file-search-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

Tools appear in the main conversation only. Sub-agents do not inherit MCP tools - that is a per-host limitation, not from this server.

### OpenCode - main session

Add to `~/.config/opencode/opencode.json` under the `mcp` key:

```json
{
  "mcp": {
    "everything": {
      "command": ["C:/path/to/instantaneous-windows-file-search-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

### OpenCode - sub-agent support (requires plugin)

```sh
mkdir -p ~/.config/opencode/plugins/everything-mcp-plugin
cp -r plugin/dist ~/.config/opencode/plugins/everything-mcp-plugin/
cp -r plugin/node_modules ~/.config/opencode/plugins/everything-mcp-plugin/
cp plugin/package.json ~/.config/opencode/plugins/everything-mcp-plugin/
```

Register in `~/.config/opencode/opencode.json` under the `plugin` array:

```json
"file:///C:/Users/YOU/.config/opencode/plugins/everything-mcp-plugin/dist/index.js"
```

The MCP entry and the plugin coexist. The MCP entry serves the main session; the plugin serves sub-agents.

### Plugin binary resolution order

1. `EVERYTHING_MCP_BINARY` environment variable
2. Default path relative to `plugin/dist/`: `../../target/release/instantaneous-windows-file-search-mcp-server.exe`
3. `PATH`

Set `EVERYTHING_MCP_BINARY` if the default path does not match your layout.

## Tools

| Tool | Returns | When to call |
|------|---------|--------------|
| `find_files` | List of matching files with metadata | You need the actual file list |
| `count_files` | Total match count only | Before a broad search to check scale |
| `search_status` | Engine health diagnostics | When tools fail unexpectedly |

### Search modifiers

Embed these in the `query` parameter:

| Modifier | Example | Effect |
|----------|---------|--------|
| `file:` | `file:*.ts` | Files only |
| `folder:` | `folder:src` | Directories only |
| `dm:` | `dm:today` `dm:2days` | Date modified filter |
| `dc:` | `dc:thisweek` | Date created filter |
| `da:` | `da:yesterday` | Date accessed filter |
| `size:` | `size:>10mb` `size:1kb..1mb` | File size filter |
| `dupe:` | `dupe:filename` | Find duplicate filenames |
| `!` | `!*.tmp` | NOT / exclude pattern |
| `|` | `*.ts | *.tsx` | OR operator |
| `" "` | `"exact phrase"` | Literal search |

### Important behaviors

- **Auto-excluded** by default (bypass with `include_all=true`): `node_modules`, `.git`, `WinSxS`
- **exclude_path separator**: semicolon (`;`), not comma
- **Default scope**: all indexed drives - narrow with `path`
- **Response**: `total` (all matches), `returned` (page count), `offset` (page position), `note` (exclusion info)

### find_files parameters

| Param | Type | Description |
|-------|------|-------------|
| `query` | string (required) | Search query with Everything modifiers |
| `path` | string | Scope to a directory |
| `max_results` | number | Results per page (max 100) |
| `offset` | number | Pagination offset |
| `exclude_path` | string | Paths to exclude (`node_modules;.git`) |
| `include_all` | boolean | Disable auto-exclusion of noise folders |
| `regex` | boolean | Enable regex mode |
| `match_case` | boolean | Case-sensitive search |
| `match_whole_word` | boolean | Whole word match |
| `match_path` | boolean | Match against full path |
| `sort` | string | Sort order (22 options) |
| `fields` | string | Comma-separated fields to return |

### count_files parameters

| Param | Type | Description |
|-------|------|-------------|
| `query` | string (required) | Search query |
| `path` | string | Scope to a directory |
| `exclude_path` | string | Paths to exclude |
| `include_all` | boolean | Disable auto-exclusion |
| `regex` | boolean | Enable regex |
| `match_case` | boolean | Case-sensitive |
| `match_whole_word` | boolean | Whole word match |

### Sort options (22)

`name`, `name_desc`, `path`, `path_desc`, `size`, `size_asc`, `date_modified`, `date_modified_asc`, `date_created`, `date_created_asc`, `date_accessed`, `date_accessed_asc`, `extension`, `extension_desc`, `run_count`, `run_count_asc`, `date_run`, `date_run_asc`, `type_name`, `type_name_desc`, `date_recently_changed`, `date_recently_changed_asc`

Default: `name`.

### Field names (12)

`filename`, `path`, `size`, `date_modified`, `date_created`, `date_accessed`, `attributes`, `extension`, `run_count`, `date_run`, `date_recently_changed`, `file_list_filename`

Default (omit `fields`): all common fields.

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

## How It Connects

```
MCP Host (VS Code / Cursor / Claude Desktop)
  └─ MCP binary (Rust, stdin/stdout)
       └─ Everything IPC (WM_COPYDATA)
            └─ Everything Desktop App
                 └─ NTFS Master File Table

OpenCode (main + sub-agents)
  └─ Plugin adapter (TypeScript)
       └─ spawns MCP binary
            └─ Everything IPC (WM_COPYDATA)
                 └─ Everything Desktop App
                      └─ NTFS Master File Table
```

All IPC is native Windows messaging. No HTTP, no network, no sockets. The `everything-ipc` Rust crate handles the Win32 window messaging.

## Development

```sh
cargo test
```

Unit tests cover sort parsing, field parsing, timestamp formatting, and attribute formatting. No integration tests - Everything must be running for real IPC queries.

Logging is controlled by the `EVERYTHING_MCP_LOG` environment variable (tracing-subscriber env-filter). Unset by default - no log output.

Key dependencies: `rmcp` for MCP transport, `everything-ipc` for WM_COPYDATA IPC, `schemars` for JSON Schema generation, `tokio` for async runtime. Everything calls are blocking and dispatched via `spawn_blocking`. No unsafe blocks, no generated code, no build scripts.

Detailed docs in the `docs/` directory: `architecture.md`, `build.md`, `development.md`, `tools.md`.

## License

MIT
