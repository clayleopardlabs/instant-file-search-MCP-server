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

This tool is Windows-only.

### The recommended method

Open PowerShell:

1. Press the Start key.
2. Type `powershell`.
3. Press the Enter key.

Run the installer:

4. Enter the command below in the PowerShell window.

   ```powershell
   powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instant-file-search-MCP-server/master/scripts/install.ps1 | iex"
   ```

5. Press the Enter key.

NOTE: The installer downloads the MCP server, the native indexer, and the `instant-file-search-fallback-engine` from the latest release on GitHub. The Rust toolchain is not required. The installer does not compile files. It also installs the OpenCode plug-in automatically. Then the sub-agents can use the same search tools.

6. Press **Yes** when Windows shows the permission prompt.
7. Restart your AI app. It can be VS Code, Cursor, Claude Desktop, or OpenCode.

NOTE: This step puts the background indexer into service. It is the only step that uses administrator rights.

NOTE: If you cannot see the permission prompt, the tools work with the fallback engine. They are slower. You can register the native indexer subsequently by running the installer in elevated mode.

After the installation, your AI assistant has five new tools: `find_files`, `count_files`, `search_status`, `recent_changes`, and `aggregate_files`.

### You have the source code

If you downloaded or cloned this repository, operate the installer from the folder. You can operate it from the checkout copy of the source code.

```powershell
.\scripts\install.ps1
```

NOTE: The installer finds the AI apps on your computer: Codex, OpenCode, and Claude Desktop. It selects the apps to set up. Press **A** to select all the detected apps. You can also type a comma-separated list. The installer saves a copy of the configuration files before it changes them. It is safe to operate the installer again at any time. If you operate it from a checkout, it uses the newly built files in `target/release`. It downloads the other files from the release.

Do the health check:

1. Operate the command below to make sure that all is in good order.

   ```powershell
   & "$env:LOCALAPPDATA\ClayLeopardLabs\instant-file-search\doctor.ps1"
   ```

2. Operate `./scripts/doctor.ps1` from the repository if you installed from a checkout.

### Configure one app at a time

The installer puts the server in this location:

```
%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\instant-file-search-mcp-server.exe
```

Add the server to the MCP client configuration:

1. Open the configuration for the MCP host.
2. Add the section below to the configuration.

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

NOTE: For a plain MCP host, use the configuration shown above. For OpenCode, add the configuration to `opencode.json` or to `opencode.jsonc`. The installer adds the plug-in adapter automatically.

3. Restart your AI app. The tool list will show the new tools.

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
| `size:` | `size:>10mb` | Larger than 10 MB (constants: tiny<1kb, small<1mb, medium<1gb, large>1gb, huge>4gb, gigantic>16gb, empty=0) |
| `attrib:` | `attrib:h`, `attrib:!d` | Match by NTFS attribute (h hidden, s system, r readonly, d directory, a archive, t temp, c compressed, e encrypted, o offline, p reparse, i not-indexed, n normal) |
| wildcards | `*.ts`, `file[0-9].txt`, `img#.png`, `**.rs` | `*` any run (not `\\`), `**` any run incl. `\\`, `?` one char, `[set]`/`[!set]` classes, `#` one digit, `\\x` escape |
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

The bundled **Fallback Engine** (`instant-file-search-fallback-engine-1.5.0.1418b`) is **Everything**, a file search utility by David Carpenter (https://www.voidtools.com) that maintains its own index of the filesystem; it ships here as the fallback search engine when the indexer service is unreachable. Everything is copyright (C) 2018 David Carpenter, distributed under the MIT License, and its embedded **PCRE** component is copyright (c) 1997-2012 University of Cambridge, distributed under the MIT-style license reproduced in its terms. The full license texts ship with the installer at `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt` (source copy: `vendor/everything/LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt`), alongside the engine itself, as those licenses require.
