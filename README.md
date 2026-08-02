# Instant File Search for AI Agents

![](demo.gif)

An MCP server that gives AI agents instant filesystem search on Windows. Agents can find files by name, extension, size, or date, scope searches to a directory, exclude noise folders, and count matches across millions of files in milliseconds, without walking folders one by one.

## How It Works

Windows records every file creation, rename, move, and deletion as it happens. This server maintains a live index of the NTFS Master File Table from those records, so agents can query the entire filesystem the same way a search engine queries a document index.

Typical agent use cases:

* "Show me what changed in this project since yesterday."
* "Find files that look like secrets or local config."
* "List source files but ignore dependencies and build output."
* "Count how many test files exist beside implementation files."
* "Find old exports, duplicate downloads, or forgotten installers."
* "Give me the project's shape before reading the code."

## Search Engine (Built In)

The server is self-contained: nothing to install, configure, or launch beyond the server itself:

1. **A native indexer** runs as a Windows service (`instant-file-search-indexer`, auto-start). On first launch it reads the NTFS Master File Table directly; a full scan of ~2.4 million files takes about 15 seconds. After that it tracks creates, renames, moves, and deletes through the Windows change journal, so the index is always current.
2. **A backup engine** ships with the installer. If the indexer service is ever stopped or unreachable, searches are answered by the backup engine automatically. You never have to start anything yourself.

Searches hit an in-memory index over a named pipe and return in milliseconds. No index files on disk, no external runtime.

## One-Command Install

```powershell
powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server/master/scripts/install.ps1 | iex"
```

This installs everything needed (the MCP server, the indexer service, and the bundled backup engine) with no separate prerequisites and no Rust toolchain.

## What You Need

Windows 10/11 with NTFS volumes (the index reads the Master File Table, which other platforms don't have). No other prerequisites.

The installer deploys the server, registers the indexer service so it starts with Windows (the only step needing administrator rights), and configures the MCP client of your choice. The server routes each search to the indexer service first and falls back to the backup engine automatically if the service isn't running.

## Components

**The MCP server binary** (Rust, single `.exe`) is all you need for VS Code, Cursor, Claude Desktop, or any MCP host. Point the host at this binary and the three tools appear in the agent's tool list.

**The plugin adapter** (TypeScript, optional) is only needed for OpenCode users who want sub-agents (explore, librarian, task workers) to access the same tools. Sub-agents do not inherit MCP tools automatically. The plugin bridges that gap by spawning the binary as a child process.

## Quick Start (build from source)

### Automatic setup for Codex, OpenCode, and Claude Desktop

Run the included installer from a checkout:

```powershell
.\scripts\install.ps1
```

The installer detects Codex, OpenCode, and Claude Desktop, then lets you choose which detected clients to configure. It builds the release binary if needed, copies it to a stable per-user location, registers the indexer service, registers the selected MCP server, installs the OpenCode adapter in `%USERPROFILE%\.config\opencode\plugins\instant-file-search-mcp-plugin`, and backs up JSON configuration files before editing them. It is safe to run again after an OS reinstall or a source update. To verify the installation later:

```powershell
.\scripts\doctor.ps1
```

For unattended setup, select clients explicitly:

```powershell
.\scripts\install.ps1 -Clients all
.\scripts\install.ps1 -Clients codex,opencode,claude
```

Use `-SkipBuild` when you already have a release binary, `-SkipCodex`, `-SkipOpenCode`, or `-SkipClaude` to omit a client, or `-DryRun` to preview the actions. Codex's native registration command is also available directly:

```powershell
codex mcp add instant-file-search -- C:\path\to\instant-file-search-mcp-server.exe
codex mcp list
```

After installation, restart Codex or start a new task so it reloads the MCP configuration.

```sh
git clone https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server
cd instantaneous-windows-file-search-mcp-server
cargo build --release
```

Binary at `target/release/instant-file-search-mcp-server.exe`.

The project pins the `x86_64-pc-windows-gnu` Rust target by default, so you only need [MSYS2/MinGW-w64](https://www.msys2.org/) (install via `winget install MSYS2.MSYS2`) instead of the full Visual Studio Build Tools. If you prefer MSVC, override the toolchain in `rust-toolchain.toml` or pass `--target x86_64-pc-windows-msvc`.

Add to your MCP client configuration:

```json
{
  "mcpServers": {
    "instant-file-search": {
      "type": "local",
      "command": ["C:/full/path/to/instant-file-search-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

Restart the client. Three tools appear in the agent's tool list: `find_files`, `count_files`, `search_status`.

## Who Needs What

| You use... | You need | Why |
|------------|----------|-----|
| VS Code, Cursor, Claude Desktop, or any MCP host | MCP server binary only | The host loads tools directly |
| OpenCode main session | MCP server binary as MCP server | Configured under `mcp` key in opencode.json |
| OpenCode sub-agents too | MCP server binary + plugin adapter | Sub-agents do not inherit MCP tools |

## Build

The repo is a Cargo workspace with two binaries.

### MCP server (required)

Requires the [Rust toolchain](https://rustup.rs):

```sh
cargo build --release
```

Output: `target/release/instant-file-search-mcp-server.exe`

### Indexer (recommended)

Built by the same workspace command (member crate `indexer/`):

```sh
cargo build --release -p instant-file-search-indexer
```

Output: `target/release/instant-file-search-indexer.exe`

Run it in one of three modes:

| Mode | Use |
|------|-----|
| `serve` | Named-pipe server + MFT scan + change-journal watcher (what the service runs) |
| `scan` | One-shot diagnostic scan, prints the entry count |
| `service` | SCM-managed Windows service (auto-start, runs `serve` internally) |

The service must run elevated (SYSTEM) to open volume devices (`\\.\C:`) for the raw MFT read. The installer registers it; `sc.exe create instant-file-search-indexer binPath= "C:\...\instant-file-search-indexer.exe service" start= auto` does it manually.

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
    "instant-file-search": {
      "type": "local",
      "command": ["C:/path/to/instant-file-search-mcp-server.exe"],
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
    "instant-file-search": {
      "command": ["C:/path/to/instant-file-search-mcp-server.exe"],
      "enabled": true
    }
  }
}
```

### OpenCode - sub-agent support (requires plugin)

```sh
mkdir -p ~/.config/opencode/plugins/instant-file-search-mcp-plugin
cp -r plugin/dist ~/.config/opencode/plugins/instant-file-search-mcp-plugin/
cp -r plugin/node_modules ~/.config/opencode/plugins/instant-file-search-mcp-plugin/
cp plugin/package.json ~/.config/opencode/plugins/instant-file-search-mcp-plugin/
```

Register in `~/.config/opencode/opencode.json` under the `plugin` array:

```json
"file:///C:/Users/YOU/.config/opencode/plugins/instant-file-search-mcp-plugin/dist/index.js"
```

The MCP entry and the plugin coexist. The MCP entry serves the main session; the plugin serves sub-agents.

### Plugin binary resolution order

1. `INSTANT_FS_MCP_BINARY` environment variable (`EVERYTHING_MCP_BINARY` accepted for backward compatibility)
2. Stable install: `%LOCALAPPDATA%\ClayLeopardLabs\EverythingMCP\instant-file-search-mcp-server.exe`
3. Default path relative to `plugin/dist/`: `../../target/release/instant-file-search-mcp-server.exe`
4. `PATH`

Set `INSTANT_FS_MCP_BINARY` if none of the default paths match your layout.

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
| `query` | string (required) | Search query with modifiers |
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
  └─ MCP server (Rust, stdin/stdout)
       └─ named pipe ──► indexer service (in-memory index, primary)
                            └─ NTFS Master File Table + change journal

OpenCode (main + sub-agents)
  └─ Plugin adapter (TypeScript)
       └─ spawns MCP server
            └─ (same routing as above)
```

If the indexer service is unreachable, the server answers from the bundled backup engine instead, with the same tools and results and no setup. All communication stays on the local machine: no HTTP, no network, no sockets.

## How It Compares to the Bundled Engine

The backup engine (Everything by voidtools) is the reference implementation this project is tested against. The native indexer is a drop-in replacement for the agent use case, not a clone of Everything's full feature set.

**Where the native engine goes beyond Everything:**

* **Self-contained.** Everything is a GUI application that must be launched (or run as a service) and keeps its index on disk. The native indexer runs as an auto-start Windows service with an in-memory index, so there is no GUI, no on-disk index, and no per-machine configuration.
* **Agent-facing by design.** Everything has no API that an AI agent can call directly. The MCP tools (`find_files`, `count_files`, `search_status`) are the entire point of this project; Everything can only be driven through its GUI, CLI, or HTTP server.
* **Portable-hygienic backup.** When the bundled Everything instance does run, it uses a named instance with a pre-seeded ini/db, so it never touches a user's real Everything installation.

**Where Everything remains more capable:**

* **Richer query language.** Everything supports `content:` (full-text), `dupe:` deduplication, attribute filters, extension lists, functions, and more. The native engine matches a subset: `dm:`/`dc:`/`da:` dates, `size:`, `case:`, `regex:`, `!` excludes, `|` OR, and path scoping.
* **Reference parity.** Because Everything is the reference, every query the native engine does support is differentially tested against it, and the two are kept within ~0.1% (residual gaps are documented in `docs/parity.md`).

The honest summary: this project makes Everything **unnecessary** for the agent use case and matches it on the search surface agents actually use, while Everything remains the more capable search engine underneath.

## Development

```sh
cargo test
```

Unit tests cover sort parsing, field parsing, timestamp formatting, and attribute formatting. No integration tests - a search engine must be running for real IPC queries.

Logging is controlled by the `EVERYTHING_MCP_LOG` environment variable (tracing-subscriber env-filter). Unset by default - no log output. Logs go to stderr so they never corrupt the MCP JSON stream. The indexer uses the same env var; as a service its output is invisible, so run `serve` from a console with redirection when diagnosing the watcher.

Key dependencies: `rmcp` for MCP transport, `everything-ipc` for the backup engine's IPC, `schemars` for JSON Schema generation, `tokio` for async runtime. Everything calls are blocking and dispatched via `spawn_blocking`. The indexer crate uses `windows` (0.62) for Win32 APIs (change journal, named pipes, service control), `ntfs` for MFT record parsing, and `windows-service` for the SCM integration. No unsafe blocks, no generated code, no build scripts.

Detailed docs in the `docs/` directory: `architecture.md`, `build.md`, `development.md`, `tools.md`.

## License

This project (the MCP server, the native indexer, and the plugin adapter) is licensed under the MIT License.

The bundled backup engine is **Everything**, a file search utility by David Carpenter (https://www.voidtools.com) that maintains its own index of the filesystem; it ships here as the fallback search engine when the indexer service is unreachable. Everything is copyright (C) 2018 David Carpenter, distributed under the MIT License, and its embedded **PCRE** component is copyright (c) 1997-2012 University of Cambridge, distributed under the MIT-style license reproduced in its terms. The full license texts ship with the installer at `%LOCALAPPDATA%\ClayLeopardLabs\EverythingMCP\LICENSE-Everything.txt` (source copy: `vendor/everything/LICENSE-Everything.txt`), alongside the engine itself, as those licenses require.
