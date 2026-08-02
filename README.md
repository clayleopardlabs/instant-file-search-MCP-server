# Search a million files. Instantly.

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

No separate programs to install. This tool brings its own search engine, so it works out of the box:

1. **A background indexer.** A small Windows service keeps a live, always-up-to-date list of everything on your drives. The first scan takes about 15 seconds on a typical PC (roughly 2.4 million files). After that it stays current on its own by watching for new, renamed, or deleted files.
2. **A Fallback Engine (`instant-file-search-fallback-engine-1.5.0.1418b`).** Just in case. If the background indexer is ever stopped, your searches are answered automatically by the Fallback Engine instead. You never have to start or manage anything.

Searches are answered in milliseconds straight from memory. Nothing gets written to disk, and nothing leaves your machine.

## Install

This tool is **Windows-only**: it reads the master file list that only Windows maintains. You need **Windows 10 or 11** — nothing else.

### The easy way (recommended)

1. Open **PowerShell**: press the Start key, type `powershell`, and press Enter.
2. Copy this one line, paste it into the PowerShell window, and press Enter:

   ```powershell
   powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instant-file-search-MCP-server/master/scripts/install.ps1 | iex"
   ```

   The installer downloads the MCP server, the native indexer, and the `instant-file-search-fallback-engine` straight from the latest GitHub release — no Rust toolchain, no compiling.

3. If Windows asks for permission, click **Yes**. (This registers the background indexer — the only step that needs administrator rights.)
4. Restart your AI app (VS Code, Cursor, Claude Desktop, or OpenCode).

That's it. Your AI assistant now has five new tools — `find_files`, `count_files`, `search_status`, `recent_changes`, and `aggregate_files`.

### Already have the code?

If you downloaded or cloned this repository, run the installer from inside the folder instead:

```powershell
.\scripts\install.ps1
```

It detects which AI apps you have (Codex, OpenCode, and Claude Desktop) and asks which ones to set up. Press **A** for all detected apps, or type a comma-separated list. It backs up your configuration files before touching them, and it's safe to run again any time. When run from a checkout, it prefers freshly built binaries in `target/release` and downloads the rest from the release.

To make sure everything is healthy later:

```powershell
.\scripts\doctor.ps1
```

### Set up a single app yourself

The installer puts the server here:

```
%LOCALAPPDATA%\ClayLeopardLabs\EverythingMCP\instant-file-search-mcp-server.exe
```

For a plain MCP host (VS Code, Cursor, Claude Desktop), add it to your MCP client configuration:

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

For **OpenCode**, add it under the `mcp` key in `~/.config/opencode/opencode.json`. OpenCode sub-agents also need the plugin adapter — see [docs/development.md](docs/development.md) if you want that.

After setting it up, restart your AI app. The tools appear in its tool list.

## What Your AI Can Do With It

Once installed, your AI assistant can:

| Tool | What it does |
|------|--------------|
| `find_files` | List files that match a search, with details like size and dates |
| `count_files` | Tell you how many files match, without listing them all |
| `search_status` | Check whether the search engine is working |
| `recent_changes` | Show files changed recently (via the Windows change journal) |
| `aggregate_files` | Answer roll-up questions like largest files, file counts by type, or total size |

Your AI can use the tools to answer things like:

- "What changed in this project since yesterday?"
- "Find files that look like secrets or local config."
- "List source files but ignore dependencies and build output."
- "Count how many test files exist beside implementation files."
- "Find old exports, duplicate downloads, or forgotten installers."
- "What are the five largest files in my downloads folder?"
- "What changed on this drive in the last week?"

### Handy search tricks

The AI can embed these in a search query:

| Trick | Example | Effect |
|-------|---------|--------|
| `file:` | `file:*.ts` | Files only |
| `folder:` | `folder:src` | Folders only |
| `dm:` | `dm:today`, `dm:2days` | Modified in the last 2 days |
| `dc:` | `dc:thisweek` | Created this week |
| `da:` | `da:yesterday` | Opened yesterday |
| `size:` | `size:>10mb` | Larger than 10 MB |
| `dupe:` | `dupe:filename` | Find duplicate filenames |
| `!` | `!*.tmp` | Exclude a pattern |
| `|` | `*.ts | *.tsx` | Match either pattern |
| `" "` | `"exact phrase"` | Match an exact phrase |

By default the tools skip noisy folders like `node_modules`, `.git`, and `WinSxS` to keep results useful. Use `include_all=true` to include them. To scope a search to one folder, pass the `path` parameter.

## Common Questions

**Do I need to install a separate search engine or any other program?** No. The search engine is built in.

**Does it send my files anywhere?** No. Every search stays on your machine. There is no network, no cloud, no server.

**Why is it so fast?** It keeps a live index of your files in memory, so it doesn't have to search your drives folder by folder.

**Will it slow down my computer?** It runs quietly in the background. The initial scan takes about 15 seconds, and after that it just tracks changes as they happen.

**Why Windows only?** It reads the NTFS master file list, which only Windows maintains.

## For Developers

The repo is a Cargo workspace with two binaries. You need the [Rust toolchain](https://rustup.rs).

```sh
cargo build --release
```

Outputs:

- `target/release/instant-file-search-mcp-server.exe` — the MCP server
- `target/release/instant-file-search-indexer.exe` — the native indexer

The installer registers the indexer as a Windows service (`instant-file-search-indexer`, auto-start). Run `indexer.exe scan` for a one-shot diagnostic, or `indexer.exe serve` to run the indexer in the foreground.

### Plugin adapter (OpenCode sub-agents)

Requires Node.js 18+:

```sh
cd plugin
npm install
npm run build
```

Output: `plugin/dist/index.js`

### Tests

```sh
cargo test
```

Unit tests cover sorting, field parsing, timestamp formatting, and attribute formatting.

More detail lives in `docs/`: `architecture.md`, `build.md`, `development.md`, `tools.md`, and `parity.md`.

## How It Fits Together

```
MCP Host (VS Code / Cursor / Claude Desktop / OpenCode)
  └─ MCP server (Rust, stdin/stdout)
       └─ named pipe ──► indexer service (in-memory index, primary)
                            └─ NTFS master file list + change journal
```

If the indexer service is unreachable, the server answers from the `instant-file-search-fallback-engine` instead, with the same tools and results and no setup.

## License

This project (the MCP server, the native indexer, and the plugin adapter) is licensed under the MIT License.

The bundled **Fallback Engine** (`instant-file-search-fallback-engine-1.5.0.1418b`) is **Everything**, a file search utility by David Carpenter (https://www.voidtools.com) that maintains its own index of the filesystem; it ships here as the fallback search engine when the indexer service is unreachable. Everything is copyright (C) 2018 David Carpenter, distributed under the MIT License, and its embedded **PCRE** component is copyright (c) 1997-2012 University of Cambridge, distributed under the MIT-style license reproduced in its terms. The full license texts ship with the installer at `%LOCALAPPDATA%\ClayLeopardLabs\EverythingMCP\LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt` (source copy: `vendor/everything/LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt`), alongside the engine itself, as those licenses require.
