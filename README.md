# Instant File Search for AI Agents

![](demo.gif)

Here is what happens when you ask an AI agent to find every config file in a project:

The agent picks a folder and asks the operating system what is inside. It gets back a list of entries. It picks a subfolder and asks again. Then again. For every directory, one round trip to the filesystem. A project with ten thousand folders means ten thousand trips.

That finds the files eventually. But it is slow, and it is unnecessary, because the computer already keeps a complete list.

## The Record the Filesystem Already Keeps

Every time you save a file, Windows writes its name, its location, its size, and the current time into a master record on the drive. When you move or rename the file, the record updates. When you delete it, the slot marks free.

This record is not a separate index you have to build and maintain. It is part of how NTFS works - the filesystem cannot function without it. The data is always there, always current, always covering every file on every drive.

Normal search tools - `find`, `Get-ChildItem`, a recursive glob - ignore this record. They start from wherever you point them and walk the directory tree one node at a time. That works fine when you need one file in one folder. It is wasteful when the answer already lives in a single table the computer updates for you.

## Reading the Table Directly

A tool like Everything by Voidtools reads this master record directly. It connects to the running Everything desktop application through a standard Windows messaging channel (WM_COPYDATA - the mechanism two windows use to exchange text). It sends a query, the desktop app looks up the answer in its in-memory index, and sends back the results. No directory walking. No recursive calls. One round trip.

Everything keeps its index current by registering for filesystem notifications. When a file appears, disappears, or changes, the index updates in near real time. So every query starts from a complete picture of what exists, not from a snapshot that was correct the last time you searched.

This is why a search across every file on a multi-terabyte drive returns in milliseconds. The question goes to a place that already holds the answer.

## What This Project Does

This project wraps that query channel into three tools that speak the Model Context Protocol - a standard that lets AI agents call external capabilities through a simple JSON message exchange over stdin and stdout.

The agent calls `find_files` with a pattern and a path. The binary forwards the request to Everything through the Windows messaging channel, collects the structured result, and returns it as JSON. The agent never runs a slow shell command or reads a partial directory listing. It asks the catalog and gets the answer.

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
