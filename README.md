# Search millions of files. Instantly.

![](demo.gif)

Search every file on your computer. All 3 million of them. In under a second.


## Start Here

This tool gives your AI assistant a fast way to find files on your computer. After installation, you can ask normal questions such as:

- "Find the project's README file."
- "Show me the largest files in this folder, ignoring build output."
- "List source files but ignore dependencies and build output."
- "Show me what changed in this project since yesterday."
- "Find files that look like secrets or local config."
- "How many JSON files are on this computer?"
- "Count how many test files exist beside implementation files."
- "Find old exports, duplicate downloads, or forgotten installers."
- "Give me the project's shape before reading the code."

You do not need to learn a search language: everyday questions work in plain English. The optional query filters under Technical Details give you more control when you want it.

## How It Works

Computers record every file creation, rename, move, and deletion as it happens. This server maintains a live index of those records, so agents can filter the entire filesystem the same way a search engine reads a document index. Because every change event is captured, the record is forensic in practice: a file modified in milliseconds still leaves a trace your agent can find.

### Frequently asked questions

**Do I need to install another search program?**

No. The installer includes everything the tool needs.

**Does anything leave my computer?**

No. Searches run locally. Your file names, file contents, and search results are not sent to a cloud service by this project.

**Why is it fast?**

The tool prepares a local index in the background. Your AI can search that index instead of opening every folder one at a time.

**Will it slow down my computer?**

The first scan uses some resources while it builds the index. After that, the background service watches for changes and normally stays quiet.

**What happens if the background service is unavailable?**

On Windows, ordinary searches can still work through the fallback search engine that ships with this release. It is the Windows-native Everything engine, distributed with the installer for exactly this case. Linux and macOS use the native indexer only and will report when it is not available.

**What permissions does it need?**

Windows needs administrator approval once to install the background service. On macOS, Full Disk Access must also be granted manually in System Settings if you want to search protected folders such as Documents or Desktop.

**How do I know it is working?**

Ask your AI assistant to run `search_status`, or use the platform health-check command described below.

### Install on Windows

1. Open PowerShell.
2. Run:

   ```powershell
   powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instant-file-search-MCP-server/master/scripts/install.ps1 | iex"
   ```

3. Approve the Windows permission prompt.
4. Restart your AI app.

The installer detects supported AI apps, installs the search service, and updates their configuration. You can safely run it again when a new version is available.

To check the installation from a repository checkout:

```powershell
.\scripts\doctor.ps1 -RequireNative
```

If the check fails because Windows blocked the administrator step, the installation is incomplete. Approve the prompt and run the installer again.

### Install on Linux or macOS

Linux:

```sh
sudo bash scripts/install-linux.sh
```

macOS:

```sh
sudo bash scripts/install-macos.sh
```

On macOS, grant Full Disk Access to the installed indexer in System Settings, then restart the launchd service. See the platform-specific build guides below for the details and current limitations.

### Ask your AI for help

The assistant can search, count, summarize, or inspect recent changes. For broad searches, it can measure the result size first and choose a useful response on its own instead of flooding the conversation with thousands of matches. It skips common noise folders by default; tell it to include everything when you really need those folders.

## What Your AI Can Do With It

Once installed, your AI assistant can:

| Tool | What it does |
|------|--------------|
| `find_files` | Discover the exact files an agent should read, with names, paths, dates, sizes, and filters |
| `count_files` | Measure the size of a search before returning a large result set |
| `search_status` | Confirm which local search engine is available and whether the index is healthy |
| `recent_changes` | Investigate what was created, modified, renamed, or deleted, newest first |
| `aggregate_files` | Answer roll-up questions like largest files, file counts by type, or total size |

## What This Unlocks for AI Agents

Most coding assistants can read a file when you give them its path. The hard part is knowing which files matter in the first place. This MCP gives the assistant a fast, live map of your computer so it can investigate before it starts opening files one by one.

### 1. Understand an unfamiliar project in seconds

An agent can discover the shape of a codebase before touching the source:

> "Show me the project's source files, tests, configuration, generated output, and largest directories. Ignore dependencies and build artifacts."

It can then decide where to look instead of guessing from a shallow directory listing or spending minutes walking the tree.

### 2. Investigate what changed, not just what exists

`recent_changes` lets an agent answer questions that ordinary file search cannot:

- What changed in this project in the last hour?
- Which files were renamed or deleted during the failed build?
- What appeared after I installed this package?
- Show only created and modified files; hide delete noise.

The indexer records change events locally, at a forensic level: a file modified in only milliseconds still leaves evidence for the agent to find. This is especially useful for debugging, reviewing automated changes, and tracing unexpected activity. The tool returns events newest first.

### 3. Search the whole computer without drowning in results

Agents can search every indexed drive, then narrow the answer by folder, file type, date, size, or path. They can count first and list second:

> "How many JSON files are there outside dependencies? Now show me the first 30 under this project."

That lets an agent reason about scale before requesting thousands of results.

### 4. Answer questions involving totals and comparisons

`aggregate_files` gives the agent facts that normally require a shell script or a manual spreadsheet:

- total files and folders
- total disk space used
- largest matching entries
- counts and sizes grouped by extension

For example:

> "Which file types take the most space in this project, and what are the five largest files?"

The agent receives the answer directly instead of listing everything and trying to sum it inside the conversation.

### 5. Find secrets, stale artifacts, and suspicious leftovers

An agent can perform broad hygiene and forensic sweeps locally:

- find `.env`, credential, backup, dump, and installer files
- locate old exports and duplicate filenames
- find unexpectedly large files
- identify files created or modified during a suspicious time window
- search targeted text files with `content:"phrase"`

These searches are useful for security reviews, release preparation, incident triage, and cleaning up a project before sharing it.

### 6. Keep working at machine scale

The native index is designed for millions of files. The agent does not need to run slow recursive shell commands, ask you where every folder is, or read a directory listing into its context just to find one file. Search stays local, fast, and small enough to use repeatedly during a task.

### 7. Give sub-agents the same filesystem awareness

The optional OpenCode adapter exposes the same search abilities to sub-agents. An explorer can map a repository, a librarian can locate documentation, and a fixer can find related tests or configuration without falling back to slow shell scans.

The practical result is a different workflow: agents can discover, measure, investigate, and then read only the files that matter.

### 8. Know whether an answer is complete before relying on it

`search_status` gives the agent a coverage and health check before it begins an investigation. It can see whether the native index is available, how many files are indexed, and which volumes are covered. That means the agent can recognize the difference between "nothing matched" and "the search service is not ready," and can explain when a result is based on a fallback or a limited content index.

## Technical Details

The following sections are for developers, advanced users, and anyone who wants to understand how the system works under the hood.

### Search Engine (Built In)

No separate programs to install. This tool brings its own search engine, so it works out of the box:

1. **A background indexer.** A small service keeps a live, always-up-to-date list of everything on your drives. It runs as a Windows service on Windows, a systemd unit on Linux, or a launchd daemon on macOS. The first scan takes about 15 seconds on a typical Windows PC (roughly 2.4 million files). After that it stays current on its own by watching for new, renamed, or deleted files.
2. **A Fallback Engine (`instant-file-search-fallback-engine-1.5.0.1418b`).** Just in case. If the background indexer is ever stopped, your searches are answered automatically by the Fallback Engine instead. You never have to start or manage anything. It is a Windows-native engine (Everything) that ships with the release installer; Linux and macOS use the native indexer exclusively.

Searches are answered in milliseconds straight from memory. Nothing gets written to disk, and nothing leaves your machine.

### Search query filters

The AI can embed these in a search query:

| Trick | Example | Effect |
|-------|---------|--------|
| `file:` | `file: *.ts` | Files only |
| `folder:` | `folder: src` | Folders only |
| `dm:` | `dm:today`, `dm:last2hours` | Modified date. Relative: today, yesterday, Ndays, lastNdays, prevNdays, thisweek/lastmonth/etc., Nhours/minutes/secs (rolling). Calendar: jan-dec (current year), sun-sat (current week), mtd, ytd, qtd |
| `dc:` | `dc:thisweek` | Created this week |
| `da:` | `da:yesterday` | Opened yesterday |
| `size:` | `size:>10mb` | Larger than 10 MB (constants: tiny<1kb, small<1mb, medium<1gb, large>1gb, huge>4gb, gigantic>16gb, empty=0) |
| `attrib:` | `attrib:h`, `attrib:!d` | Match by NTFS attribute (h hidden, s system, r readonly, d directory, a archive, t temp, c compressed, e encrypted, o offline, p reparse, i not-indexed, n normal) |
| wildcards | `*.ts`, `file[0-9].txt`, `img#.png`, `**.rs` | `*` any run (not `\\`), `**` any run incl. `\\`, `?` one char, `[set]`/`[!set]` classes, `#` one digit, `\\x` escape |
| `dupe:` | `dupe:filename` | Find duplicate filenames |
| `!` | `!*.tmp` | Exclude a pattern |
| `|` | `*.ts | *.tsx` | Match either pattern |
| `len:` | `len:>10`, `len:1..5` | Filename length filter (same operators as `size:`) |
| `frn:` | `frn:>1000` | File reference number filter |
| anchors | `^foo`, `bar$`, `^exact$` | `^` start-of-name, `$` end-of-name; also `start-with:`, `end-with:`, `prefix:`, `suffix:` |
| `is:` | `is:hidden`, `is:folder` | Type/attribute shorthand: `folder`/`file`, `hidden`, `system`, `readonly`, `archive`, `temporary`, `compressed`, `encrypted`, `offline`, `reparse`, `not-content-indexed`, `normal` |
| `and:`/`or:`/`not:` | `and:foo`, `or:bar`, `not:baz` | Operator aliases: `and:` = default AND, `or:` = OR with previous, `not:` = exclude |
| `metric:` | `metric:size:>1000kb` | Switch size interpretation from JEDEC (1024-based) to decimal (1000-based) |
| `wholeword:` | `wholeword:foo`, `ww:foo` | Match whole word only |
| `" "` | `"exact phrase"` | Match an exact phrase |
| `content:` | `content:"fn main"` | Match file contents. Backed by a bounded 256 MB store, so coverage is a subset of files; use it for targeted searches, not exhaustive counts |

Noisy folders such as `node_modules`, `.git`, and `WinSxS` are skipped by default so results stay useful. When a task genuinely requires a complete inventory, the agent can include those folders with `include_all=true`. Folder scoping also accepts ordinary paths such as `C:/Users`; the engine normalizes path separators for the agent.

### Detailed Installation Reference

The one-line installer command shown in Install on Windows is the recommended method. Notes for agents and power users:

- The installer downloads the MCP server, the native indexer, and the `instant-file-search-fallback-engine` from the latest release on GitHub. The Rust toolchain is not required; the installer does not compile files. It also installs the OpenCode plug-in automatically so sub-agents can use the same search tools.
- Approving the permission prompt is the only step that uses administrator rights; it puts the background indexer into service. If you cannot see the permission prompt, the tools work with the fallback engine (they are slower). You can register the native indexer subsequently by running the installer in elevated mode.
- If an AI agent reports that it installed only the fallback engine, the installation is incomplete. Ask it to run the repository installer with the required UAC approval and verify with `doctor.ps1 -RequireNative`.
- Updates are staged in a new versioned folder before the installer switches the MCP client and Windows service to it. Running sessions keep their old executable until you restart the AI app; the installer never overwrites an executable that is in use. The active version and paths are recorded in `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\current.json`.

#### Agent installation contract

Agents should treat the native indexer as a required part of the normal Windows installation. The Everything engine is a resilience fallback, not a substitute for the service. A fallback-only install must never be reported as complete unless the user explicitly requested fallback mode.

For an agent-run install, use the repository installer and then run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\doctor.ps1 -RequireNative
```

The installation is complete only when that command exits with code 0. If UAC, policy, or permissions block service registration, report the installation as incomplete and ask the user to approve or perform the elevated step.

#### You have the source code

If you downloaded or cloned this repository, operate the installer from the folder. You can operate it from the checkout copy of the source code.

```powershell
.\scripts\install.ps1
```

The installer finds the AI apps on your computer: Codex, OpenCode, Claude Desktop, and Hermes. It selects the apps to set up. Press **A** to select all the detected apps, or type a comma-separated list. The installer saves a copy of the configuration files before it changes them. It is safe to operate the installer again at any time. If you operate it from a checkout, it uses the newly built files in `target/release` and downloads the other files from the release.

Do the health check:

1. Run the command below to make sure that all is in good order.

   ```powershell
   & "$env:LOCALAPPDATA\ClayLeopardLabs\instant-file-search\doctor.ps1"
   ```

2. Operate `./scripts/doctor.ps1` from the repository if you installed from a checkout.

#### Configure one app at a time

The installer puts each server version in this location and records the active one in `current.json`:

```
%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\versions\<version>\instant-file-search-mcp-server.exe
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

For a plain MCP host, use the active server path from `current.json`. For OpenCode, add the configuration to `opencode.json` or `opencode.jsonc`. For Hermes, add the server under `mcp_servers` in `%LOCALAPPDATA%\hermes\config.yaml` (or the directory selected by `HERMES_HOME`):

   ```yaml
   mcp_servers:
     instant-file-search:
       command: 'C:\path\to\instant-file-search-mcp-server.exe'
       enabled: true
   ```

The Windows installer writes this Hermes entry automatically and preserves the rest of the YAML file. The installer adds the OpenCode plug-in adapter automatically. `doctor.ps1` reports a partial upgrade if the configured client or service still points to an older version.

3. Restart your AI app. The tool list will show the new tools.

### For Developers

The repo is a Cargo workspace with two binaries. You need the [Rust toolchain](https://rustup.rs).

```sh
cargo build --release
```

Outputs:

- `target/release/instant-file-search-mcp-server.exe` - the MCP server (Windows)
- `target/release/instant-file-search-indexer.exe` - the native indexer (Windows)
- `target/x86_64-unknown-linux-gnu/release/instant-file-search-mcp-server` - Linux build
- `target/x86_64-unknown-linux-gnu/release/instant-file-search-indexer` - Linux build

The installer registers the indexer as a Windows service (`instant-file-search-indexer`, auto-start). Run `indexer.exe scan` for a one-shot diagnostic, or `indexer.exe serve` to run the indexer in the foreground.

### Linux

The native indexer runs on Linux. Tested on Ubuntu 26.04 LTS, kernel 7.0, x86_64. The Linux backend swaps the Windows pillars for their Linux equivalents:

| Windows | Linux |
|---|---|
| $MFT raw scan | `getdents64` walk + `statx` (inode = `file_ref`, btime = created) |
| USN Change Journal | fanotify FID-mode marks (needs root/CAP_SYS_ADMIN; inotify is a documented fallback) |
| Named pipe | Unix socket at `/tmp/instant-file-search-indexer.sock` (mode 0666) |
| Everything fallback engine | Not used; native is the only engine on Linux |
| Windows service (SCM) | systemd unit with `Type=notify` + sd_notify readiness |

Install from source on a Linux box with the command shown in Start Here. The script builds both binaries, installs them under `/usr/local/lib/instant-file-search/`, installs the systemd unit, starts the service, and registers the MCP client with OpenCode (including the OMO sub-agent `mcps` patch).

See `docs/build-linux.md` and `docs/linux-port-plan.md` for details and known gaps.

**Note:** `/tmp` is typically tmpfs (RAM-backed) and is excluded from the indexer scan. Use paths on real disk volumes (e.g. `/home`, `/var`, `/usr`) for test files.

### macOS (experimental)

The native indexer also builds and runs on macOS (Apple Silicon / arm64; any recent macOS version). The macOS backend is native-only: the Everything fallback engine is Windows-only by construction. The platform seam is the same one the Linux port established; macOS adds a third backend to it:

| Windows | Linux | macOS |
|---|---|---|
| $MFT raw scan | `getdents64` walk + `statx` | `getattrlistbulk` walk (batched metadata; `fileid` = `file_ref`, `crtime` = created) |
| USN Change Journal | fanotify FID-mode marks | FSEvents (persistent per-device journal, `since=` replay, `UseExtendedData` for renames) |
| Named pipe | Unix socket `/tmp/instant-file-search-indexer.sock` | Same Unix socket |
| Windows service (SCM) | systemd unit (`Type=notify`) | launchd LaunchDaemon (readiness via connect+ping) |
| Everything fallback engine | Not used | Not used (native-only) |

**Case + Unicode normalization:** APFS is case-insensitive and normalization-insensitive but byte-preserving, so `readdir` returns a mix of NFC and NFD names. The engine canonicalizes both index keys (`lower_path`, `lower_name`) and query patterns to NFC + Unicode-lowercase on macOS (ASCII lowercase elsewhere), so a query for `café` matches a file stored as `cafe\u{301}` regardless of which form the filesystem returned. Case-sensitive (`case:`, `match_case`) matching uses NFC-only normalization so true-case patterns still match exactly. Path scoping (`path:`, `exclude_path`) is separator-agnostic on macOS/Linux, accepting both `/` and `\`.

Install from source on a Mac with the command shown in Start Here. The script builds both binaries, installs them under `/usr/local/lib/instant-file-search/`, installs the launchd daemon (`com.clayleopardlabs.instant-file-search`), starts it, and registers the MCP client with OpenCode (including the OMO sub-agent `mcps` patch).

**Required manual step - Full Disk Access (TCC):** after install, grant Full Disk Access to `/usr/local/lib/instant-file-search/instant-file-search-indexer` in System Settings > Privacy & Security > Full Disk Access, then restart the daemon (`sudo launchctl kickstart -k system/com.clayleopardlabs.instant-file-search`). Root does not imply FDA; grants are silently revocable by OS upgrades.

See `docs/build-macos.md` and `docs/macos-support.md` for details, known gaps, and the TCC/app-bundle discussion.

#### oh-my-opencode and slim (OMO) auto-configuration for subagents

If [oh-my-opencode-slim](https://github.com/anthropics/oh-my-opencode-slim) is installed, the installer automatically patches its config to add `instant-file-search` to every sub-agent's `mcps` array. This gives sub-agents (explorer, fixer, designer, oracle, librarian) access to the instant search tools so they can use them instead of falling back to slow shell commands.

Orchestrators with `mcps: ["*"]` are left untouched. The installer creates a timestamped backup before modifying the config.

#### Hermes Auto Config

Installer detects Hermes and autoconfigures the MCP server

#### Plugin adapter (OpenCode sub-agents)

Requires Node.js 18+:

```sh
cd plugin
npm install
npm run build
```

Output: `plugin/dist/index.js`

#### Tests

```sh
cargo test
```

Unit tests cover sorting, field parsing, timestamp formatting, and attribute formatting.

More detail lives in `docs/`: `architecture.md`, `build.md`, `development.md`, `tools.md`, `parity.md`, `build-linux.md`, `linux-support.md`, `linux-port-plan.md`, `build-macos.md`, and `macos-support.md`.

## How It Fits Together

```
MCP Host (VS Code / Cursor / Claude Desktop / OpenCode / Hermes)
  └─ MCP server (Rust, stdin/stdout)
       └─ named pipe ──► indexer service (in-memory index, primary)
                            └─ NTFS master file list + change journal
```

On Windows, if the indexer service is unreachable, the server answers from the `instant-file-search-fallback-engine` instead, with the same tools and results and no setup. On Linux and macOS the native indexer is the only engine.

## License

This project (the MCP server, the native indexer, and the plugin adapter) is licensed under the MIT License.

The bundled **Fallback Engine** (`instant-file-search-fallback-engine-1.5.0.1418b`) is **Everything**, a file search utility by David Carpenter (https://www.voidtools.com) that maintains its own index of the filesystem; it ships here as the fallback search engine when the indexer service is unreachable. Everything is copyright (C) 2018 David Carpenter, distributed under the MIT License, and its embedded **PCRE** component is copyright (c) 1997-2012 University of Cambridge, distributed under the MIT-style license reproduced in its terms. The full license texts ship with the installer at `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt` (source copy: `vendor/everything/LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt`), alongside the engine itself, as those licenses require.
