# Technical Details

The following sections are for developers, advanced users, and anyone who wants to understand how the system works under the hood.

## Search Engine (Built In)

No separate programs to install. This tool brings its own search engine, so it works out of the box:

1. **A background indexer.** A small service keeps a live, always-up-to-date list of everything on your drives. It runs as a Windows service on Windows, a systemd unit on Linux, or a launchd daemon on macOS. The first scan takes about 15 seconds on a typical Windows PC (roughly 2.4 million files). After that it stays current on its own by watching for new, renamed, or deleted files.
2. **An optional Fallback Engine (`instant-file-search-fallback-engine-1.5.0.1418b`).** Windows only, and only present if you install it with `-IncludeFallbackEngine` (or already have Everything installed). When it is present and the background indexer is stopped, searches are answered by it instead. Linux and macOS use the native indexer exclusively.

Searches are local. RAM saving mode stores the index on disk, while super duper fast mode keeps it in RAM. RAM saving mode keeps query memory small; on Linux in RAM saving mode the initial scan streams entries into the database as they are discovered instead of holding the whole corpus in memory. Nothing leaves your machine.

## Storage-mode performance on a slow hard drive

The published 500,000-file benchmark ran on NVMe storage. The hard-drive figures below are planning estimates based on the recorded I/O volume.

The RAM saving run recorded 6,159,225,044 bytes read and 2,752,666,044 bytes written while building its SQLite database. That is 8,911,891,088 bytes, or about 8.3 GiB of I/O. A 4,500 RPM hard drive that manages 50 to 100 MB/s of sustained transfer would need roughly 90 to 180 seconds for that transfer alone. Random reads, seeks, and SQLite's write pattern will add time, so a few minutes is a more realistic expectation for the first 500,000-file build.

Super duper fast mode avoids writing that SQLite database, but it still has to read the filesystem to build the index. Once it is built, its searches stay in RAM. The 500,000-file run used 488 MiB, or roughly 1 MiB per 1,000 synthetic file records. Real paths and metadata vary, so treat that as a planning estimate, not a capacity guarantee.

Those benchmark RAM figures are measured after the build, with the benchmark's input already dropped. Initial-scan memory on a real corpus is a different number: on a 1.6M-entry Linux machine the deployed 1.0.20 indexer reached a 1.19 GB VmHWM during the scan and held 710 MB VmRSS steady afterwards, in disk mode. The Linux scan now streams entries into SQLite as they are discovered instead of collecting them first, so the corpus is no longer held in memory; on the same corpus the synthetic scan peak fell to 9.9 MiB and a real full walk of 1,627,303 entries peaked at 11.5 MiB, with build time and query results unchanged. See [Linux Support](linux-support.md) for the measurement, its date and host, the socket caveat, and the restart caveat.

For a very large server, file count matters more than the total number of bytes stored. A petabyte made of a few large files is easy to index. A petabyte made of billions of small files is not. RAM saving mode is the only practical choice for those catalogs, but this project's generic disk queries can still stream many SQLite rows. It is not yet a petabyte-scale search architecture without further sharding and query indexes.

## Detailed Installation Reference

Every Windows installer flag, the DeepSeek Harness registration model (root and
preset scopes), verification exit codes, stale-client checks, and uninstall are
documented in [installer.md](installer.md).

The one-line installer command shown in Install on Windows is the recommended method. Notes for agents and power users:

- The installer downloads the MCP server and the native indexer from the latest release on GitHub, plus the optional `instant-file-search-fallback-engine` only when you pass `-IncludeFallbackEngine`. The Rust toolchain is not required; the installer does not compile files. It also installs the OpenCode plug-in automatically so sub-agents can use the same search tools.
- Approving the permission prompt is the only step that uses administrator rights; it puts the background indexer into service. If you cannot see the permission prompt, the native indexer is not installed; if the optional fallback engine is also present, searches are answered by it (slower), otherwise the tools report that no engine is available. You can register the native indexer subsequently by running the installer in elevated mode.
- If an AI agent reports that it installed only the fallback engine, the installation is incomplete. Ask it to run the repository installer with the required UAC approval and verify with `doctor.ps1 -RequireNative`.
- Updates are staged in a new versioned folder before the installer switches the MCP client and Windows service to it. Running sessions keep their old executable until you restart the AI app; the installer never overwrites an executable that is in use. The active version and paths are recorded in `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\current.json`.

### Agent installation contract

Agents should treat the native indexer as a required part of the normal Windows installation. The Everything engine is a resilience fallback, not a substitute for the service. A fallback-only install must never be reported as complete unless the user explicitly requested fallback mode.

For an agent-run install, use the repository installer and then run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\doctor.ps1 -RequireNative
```

The installation is complete only when that command exits with code 0. If UAC, policy, or permissions block service registration, report the installation as incomplete and ask the user to approve or perform the elevated step.

### You have the source code

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

### Configure one app at a time

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
       "instant": {
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
  instant:
    command: 'C:\path\to\instant-file-search-mcp-server.exe'
    enabled: true
```

The Windows installer writes this Hermes entry automatically and preserves the rest of the YAML file. The installer adds the OpenCode plug-in adapter automatically. `doctor.ps1` reports a partial upgrade if the configured client or service still points to an older version.

3. Restart your AI app. The tool list will show the new tools.

## For Developers

The repo is a Cargo workspace with two binaries. You need the [Rust toolchain](https://rustup.rs).

```sh
cargo build --release
```

Outputs:

- `target/release/instant-file-search-mcp-server.exe` - the MCP server (Windows)
- `target/release/instant-file-search-indexer.exe` - the native indexer (Windows)
- `target/release/instant-file-search-mcp-server` - Linux build
- `target/release/instant-file-search-indexer` - Linux build

(Linux builds land in `target/release/` on a Linux host; a cross-compiled build puts them in `target/x86_64-unknown-linux-gnu/release/`.)

The installer registers the indexer as a Windows service (`instant-file-search-indexer`, auto-start). Run `indexer.exe scan` for a one-shot diagnostic, or `indexer.exe serve` to run the indexer in the foreground.

## Linux

The native indexer runs on Linux. Tested on Ubuntu 26.04 LTS, kernel 7.0, x86_64. The Linux backend swaps the Windows pillars for their Linux equivalents:

| Windows | Linux |
|---|---|
| $MFT raw scan | `getdents64` walk + `statx` (inode = `file_ref`, btime = created) |
| USN Change Journal | fanotify FID-mode marks (needs root/CAP_SYS_ADMIN; no inotify fallback is implemented) |
| Named pipe | Unix socket at `/tmp/instant-file-search-indexer.sock` (mode 0666) |
| Everything fallback engine | Not used; native is the only engine on Linux |
| Windows service (SCM) | systemd unit with `Type=notify` + sd_notify readiness |

Disk mode is active on Linux, and its initial scan streams: the walk feeds entries into one SQLite transaction as they are discovered (`walk::scan_volume_into` then `DiskIndex::replace_streaming`) instead of collecting a corpus-sized `Vec<IndexedFile>` first. Only that initial `serve` scan streams; the fanotify queue-overflow recovery keeps the collecting `build_index()` for an atomic swap while queries are served. Before this change the deployed service measured 710 MB VmRSS with a 1.19 GB high-water mark on a 1.6M-entry machine; with streaming, the same synthetic corpus peaked at 9.9 MiB and a real full walk of 1,627,303 entries peaked at 11.5 MiB, with build time and query results unchanged. [linux-support.md](linux-support.md) records the numbers, their date and host, the socket caveat, and the restart caveat. The fanotify watcher also ignores events for the indexer's own files (`index.sqlite3`, its `-wal`/`-shm` sidecars, and `changes.jsonl`); those events used to drive a self-write feedback loop that pinned a CPU core and grew the change journal to 53 MB.

Linux still re-walks every volume on each service start. `checkpoints_can_resume()` returns false on Linux because fanotify keeps no persistent event history, while Windows USN checkpoints and macOS FSEvents can resume. The streaming scan keeps that restart cheap in RAM but does not remove the walk.

Startup is observable even though the socket is not: the walk logs `scanning <volume>` plus heartbeats, sends the same text to `systemctl status` via `sd_notify(STATUS=...)`, writes `/run/instant-file-search/scan.json` every 2 s (`state`, `volume`, `entries`, timestamps), and `search_status` returns it as `scan`. The unit sets `TimeoutStartSec=900`; the default 90 s can kill a cold-cache walk (an `ntfs-3g`/FUSE volume is the worst case) and restart it in a loop.

Install from source on a Linux box with the command shown in Start Here. The script builds both binaries, installs them under `/usr/local/lib/instant-file-search/`, installs the systemd unit, starts the service, and registers the MCP client with OpenCode (including the OMO sub-agent `mcps` patch).

See `build-linux.md` and `linux-port-plan.md` for details and known gaps.

**Note:** `/tmp` is typically tmpfs (RAM-backed) and is excluded from the indexer scan. Use paths on real disk volumes (e.g. `/home`, `/var`, `/usr`) for test files.

## macOS (experimental)

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

See `build-macos.md` and `macos-support.md` for details, known gaps, and the TCC/app-bundle discussion.

### oh-my-opencode and slim (OMO) auto-configuration for subagents

If [oh-my-opencode-slim](https://github.com/anthropics/oh-my-opencode-slim) is installed, the installer automatically patches its config to add `instant` to every sub-agent's `mcps` array. This gives sub-agents (explorer, fixer, designer, oracle, librarian) access to the instant search tools so they can use them instead of falling back to slow shell commands.

Orchestrators with `mcps: ["*"]` are left untouched. The installer creates a timestamped backup before modifying the config.

### Hermes Auto Config

Installer detects Hermes and autoconfigures the MCP server

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
The Windows installer has its own pure-function suite: `powershell -File scripts/install.ps1 -SelfTest` (also run by CI).

More detail lives in `docs/`: `architecture.md`, `build.md`, `development.md`, `tools.md`, `parity.md`, `build-linux.md`, `linux-support.md`, `linux-port-plan.md`, `build-macos.md`, and `macos-support.md`.

## How It Fits Together

```
MCP Host (VS Code / Cursor / Claude Desktop / OpenCode / Hermes)
  └─ MCP server (Rust, stdin/stdout)
       └─ named pipe ──► indexer service (disk-backed index, primary)
                            └─ NTFS master file list + change journal
```

On Windows, if the indexer service is unreachable and the optional `instant-file-search-fallback-engine` was installed (or Everything is already present), the server answers from it instead with the same tools and results. Without it, the tools report that no engine is available. On Linux and macOS the native indexer is the only engine.

## License

This project (the MCP server, the native indexer, and the plugin adapter) is licensed under the MIT License.

The bundled **Fallback Engine** (`instant-file-search-fallback-engine-1.5.0.1418b`) is **Everything**, a file search utility by David Carpenter (https://www.voidtools.com) that maintains its own index of the filesystem; it ships here as the fallback search engine when the indexer service is unreachable. Everything is copyright (C) 2018 David Carpenter, distributed under the MIT License, and its embedded **PCRE** component is copyright (c) 1997-2012 University of Cambridge, distributed under the MIT-style license reproduced in its terms. The full license texts ship with the installer at `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt` (source copy: `vendor/everything/LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt`), alongside the engine itself, as those licenses require.
