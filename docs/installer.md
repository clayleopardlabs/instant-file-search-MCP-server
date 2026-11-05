# Installer reference (Windows)

`scripts/install.ps1` installs the MCP server, the native indexer service, and the
MCP registration for each selected client. The fallback engine is optional and is
only installed when asked for, or when Everything is already installed on the
machine:

```powershell
powershell -c "irm https://raw.githubusercontent.com/clayleopardlabs/instant-file-search-MCP-server/master/scripts/install.ps1 | iex"
```

Run it again any time to upgrade; it is idempotent and backs up every file it
edits.

## Quick starts

```powershell
# Interactive: detect clients and ask which to configure
.\scripts\install.ps1

# DeepSeek Harness only, unattended (agent / CI)
.\scripts\install.ps1 -Clients dsh -NonInteractive

# Preview everything without touching the machine
.\scripts\install.ps1 -Clients all -DryRun

# Remove the server, its service, and its registrations
.\scripts\install.ps1 -Uninstall -Clients dsh
```

## Options

| Option | Default | Meaning |
|---|---|---|
| `-Clients` | prompt | `codex`, `opencode`, `claude`, `hermes`, `dsh`, `all`, or a comma-separated list. |
| `-SkipCodex` `-SkipOpenCode` `-SkipClaude` `-SkipHermes` `-SkipDsh` | off | Remove one client from an `all`/detected selection. |
| `-InstallRoot` | `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search` | Where versioned binaries, `current.json`, downloads, and `doctor.ps1` live. |
| `-ServerBinary` `-IndexerBinary` | release download | Install specific builds (needed for `-SkipDownload` source installs). |
| `-IndexMode` | `disk` | `disk` = RAM-saving mode; `memory` = "super duper fast" RAM-resident index. |
| `-ContentMode` | `auto` | `auto`, `off`, `memory`, `disk` for the content-index cache. |
| `-SkipDownload` | off | Never download; requires local binaries or a checkout. |
| `-IncludeFallbackEngine` | off | Bundle the optional Everything fallback engine. Also included automatically when Everything is already installed; otherwise the installer asks, unless `-NonInteractive`. |
| `-SkipElevation` | off | Do not attempt the admin step; print the exact elevated helper command instead. |
| `-NonInteractive` | off | Never prompt: no client picker and no UAC attempt; prints the elevated command and records an incomplete install. Use this from agents and CI. |
| `-ElevationTimeoutSec` | `120` | How long to wait for the UAC prompt before continuing without the native indexer. `0` waits forever. |
| `-NativeWaitSec` | `180` | After starting the service, how long to wait for the indexer's named pipe (first index build) before reporting "still building". |
| `-DshProfile` | auto | DeepSeek Harness profile to edit. Auto-detected from `DSH_PROFILE`, then the running harness; the installer refuses to guess when several profiles exist. |
| `-DshPresets` | none | Mount the server inside the listed agent presets (for example `standard,reverse-engineering`) in addition to the patch root. |
| `-PruneOldVersions` | off | Delete superseded `versions/<version>` directories after a successful install. |
| `-Uninstall` | off | Remove registrations, client entries, the service, and installed files. |
| `-DryRun` | off | Print every planned action; change nothing. |
| `-SelfTest` | off | Run the installer's pure-function regression tests (used by CI); never touches a profile or service. |

## DeepSeek Harness behaviour

DeepSeek Harness composes one Cordis plugin tree per profile. The installer
edits the user layer, `<DSH_HOME>\profiles\<profile>\cordis.patch.yml`, and
never the bundle layers.

- **Root and preset scopes.** The server is registered once at the patch root
  (what a plain agent sees) and can additionally be mounted inside agent
  presets with `-DshPresets`. The installer recognises both preset shapes:
  inserted `@deepseek-ai/dsh-agent-preset` rows and top-level
  `- id: preset-<name>` overrides.
- **Every row is repointed on upgrade.** A profile can hold several rows for
  the same server name; leaving one on an old `versions/<v>` path is how a
  working install turns into "no tools" after an upgrade.
- **The root row is restored if it is missing.** A registration that only
  lives inside presets leaves plain agents without the tools.
- **Profile resolution never guesses.** With several profiles and no
  `DSH_PROFILE`, the installer stops with the profile list instead of editing
  the first one it finds.
- **Hot reload.** DSH watches this file: a successful edit needs no restart,
  and the `mcp__instant__*` tools appear on the next agent turn. Restart only
  if they do not.
- **Encoding safety.** The patch is written as UTF-8 with its original BOM and
  per-line LF/CRLF state, so em dashes, CJK preset text, and mixed line endings
  survive the edit.
- **Preset gaps are reported.** If the profile defines presets that do not
  mount the server, the installer says which ones and how to add them
  (`-DshPresets <name>`).

`doctor.ps1` read-only check for a configured profile:

```powershell
$env:DSH_PROFILE = 'web'; .\scripts\doctor.ps1 -RequireNative
```

## Verification and exit codes

After installing, the script:

1. starts the installed server and speaks MCP over stdio: `initialize`,
   `tools/list` (expects the five tools), and a real `search_status` call;
2. waits for the indexer pipe and reports the indexed file count;
3. runs `dsh --profile <p> --dump-config` (when `dsh` is on `PATH`) and checks
   that the profile actually composes the `instant` server.

| Exit code | Meaning |
|---|---|
| `0` | Complete and verified. |
| `1` | Installed, but verification failed (see the printed diagnostics hint). |
| `2` | Installed, but the native indexer still needs one elevated run; searches work through the Fallback Engine meanwhile. |

## Stale registrations in other clients

The installer only edits the clients it was asked to configure. After a
successful install it scans the other clients for entries that still point at an
older copy of the server (Codex legacy sections, `INSTANT_FS_MCP_BINARY`,
OpenCode/Claude/Hermes entries) and prints the `-Clients <name>` invocation that
fixes each one. `doctor.ps1` performs the same check read-only.

## Uninstalling

`-Uninstall` removes the DeepSeek Harness rows (root and preset scopes), the
client entries for the selected clients, the indexer service, the user
environment variables, and the install directory. Close AI apps that run the
server first: a running MCP server process keeps its binary locked. The index
database under `%PROGRAMDATA%\ClayLeopardLabs\instant-file-search` needs
administrator rights to delete and is left in place with a note.

## Troubleshooting

- **`doctor.ps1 -RequireNative` fails after a "successful" install**, the
  admin step was skipped or denied. Run the printed elevated helper command
  (or re-run without `-SkipElevation`/`-NonInteractive`) and check again.
- **DeepSeek Harness has no tools**: confirm the profile the app uses, run
  `install.ps1 -Clients dsh -DshProfile <name>`, and restart only if the tools
  still do not appear. `-DshPresets` covers preset-scoped agents.
- **Searches are slow or report the fallback engine**, the native indexer is
  still building its first index, or the service is not running
  (`sc.exe query instant-file-search-indexer`).
- **Old version directories are piling up**: re-run with
  `-PruneOldVersions`, after updating any clients the stale-registration scan
  reports.
