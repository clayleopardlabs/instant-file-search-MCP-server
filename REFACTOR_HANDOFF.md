# Handoff: Installation Experience Refactor — Instant File Search MCP Server

## Purpose of this document

This is a handoff spec for a **refactor** of the installation experience of this Windows MCP server. It was written because the current maintainer cannot run admin-level steps right now (no UAC access), and the change touches several files with subtle cross-dependencies. Your job is to **implement the remaining work, verify it, and ship it** — not to redesign it. The existing (uncommitted) rewrite of `scripts/install.ps1` is the **reference implementation**: its behavior, function names, asset names, and messages are the source of truth, and the other files must be brought in line with it.

Read this whole document first. The numbered fixes, the “Already done” section, and the “Remaining work” section are meant to be cross-checked against each other: if you find an inconsistency (a file listed as done but not reflecting it, a referenced asset name that doesn’t match the code, a line number that points at the wrong text), **flag it rather than silently guessing** — the author would rather know than have a silent drift.

## Context

**Repo:** `C:\Users\sophi\Documents\Clay's Work\instant-search-repo` (working clone, on branch `master`, tracking `https://github.com/clayleopardlabs/instant-file-search-MCP-server.git`)

A Windows MCP server that gives AI agents instant filesystem search. It has two engines:
1. **Native indexer** (`instant-file-search-indexer.exe`) — runs as a Windows service (`instant-file-search-indexer`), fast in-memory index. Registering it requires **administrator** rights.
2. **Fallback Engine** (`instant-file-search-fallback-engine-1.5.0.1418b`) — bundled Everything engine, auto-starts, works with no admin.

The project recently went through a naming standardization: repo is now `instant-file-search-MCP-server`, all artifacts are `instant-file-search-*` (server, indexer, fallback engine zip/ini/license). The fallback engine is deliberately NOT called "Everything" in user-facing copy (only the LICENSE section legally names it).

## The problem being solved

A new user installing via the one-liner (`irm https://raw.githubusercontent.com/clayleopardlabs/instant-file-search-MCP-server/master/scripts/install.ps1 | iex`) gets a degraded experience. Five concrete defects were identified:

1. **Native indexer silently never installs.** The one-liner runs from a NON-elevated PowerShell. `Install-NativeService` prints a yellow WARN and skips — user never gets the fast in-memory indexer and isn't prompted to elevate.
2. **Config split-brain.** The installer hardcodes `opencode.json`, but most machines (including this one) use `opencode.jsonc`. opencode deep-merges both, so it *works*, but the config is split across two files and opencode's own JSONC comments would break the installer's `ConvertFrom-Json`.
3. **OpenCode sub-agent plugin is promised but never shipped.** README says sub-agents need "the plugin adapter", but a release download has no built `plugin/dist` and the installer just prints "no plugin build found... skipping". Sub-agents can't use the tools.
4. **One-liner users can't run diagnostics.** README says "Run `.\scripts\doctor.ps1`" but a `irm | iex` user has no checkout, so that path doesn't exist.
5. **Naming inconsistency.** Some user-facing strings still say "Everything" (doctor.ps1 "Everything license notice", "Bundled Everything engine present"; install.ps1 "PASS: Bundled Everything deployed", "Everything is running").

## What has ALREADY been done (in the working tree, NOT yet committed)

`scripts/install.ps1` has been fully rewritten and contains ALL FIVE fixes. This is the reference implementation — the remaining files must be brought in line with it. Review it first; it is the source of truth for behavior.

Key things install.ps1 now does:
- **New param:** `[switch]$SkipElevation` — escape hatch so CI / the current user (who can't click UAC right now) can install without prompting.
- **`Test-Elevated`** helper (checks admin token).
- **`Install-NativeService`** now:
  - writes a tiny helper script `$InstallRoot\indexer\register-indexer-service.ps1` that does the `sc.exe create` + `Start-Service` (so elevation doesn't need quoting gymnastics);
  - if already elevated: runs the helper directly;
  - if NOT elevated and NOT `-SkipElevation`: launches `Start-Process powershell -Verb RunAs ... -File $registerHelper -Wait` and tells the user a UAC prompt will appear;
  - if NOT elevated AND `-SkipElevation`: warns and prints the exact elevated command to run later, but does NOT prompt.
- **JSONC-aware parsing:** new `ConvertFrom-JsonC` function (string-aware comment + trailing-comma stripper). `Read-JsonConfig` now tries `ConvertFrom-Json` first, falls back to `ConvertFrom-JsonC`.
- **`Resolve-OpenCodeConfig`:** uses existing `opencode.jsonc` if present, else existing `opencode.json`, else creates `opencode.json`.
- **`Remove-OrphanOpenCodeJson`:** if we switched to jsonc but a previous install left an `opencode.json` whose only entry is `instant-file-search`, back it up and delete it.
- **Plugin-from-release:** `Install-OpenCode` now, when no local `plugin/dist`, downloads three release assets (`instant-file-search-mcp-plugin-index.js`, `-package.json`, `-package-lock.json`) into `~/.config/opencode/plugins/instant-file-search-mcp-plugin/` and runs `npm ci --omit=dev --ignore-scripts`.
- **`Install-Doctor`:** installs `doctor.ps1` into `$InstallRoot\doctor.ps1` (from checkout, or downloads it as a release asset).
- **`Test-Installation`:** runs the MCP `initialize` smoke test at the end and prints PASS/FAIL.
- **Naming:** all user-facing strings say "Fallback Engine" / "fallback engine" instead of "Everything".

## What REMAINS to do (the actual work for the refactor agent)

### 1. `scripts/doctor.ps1` — bring in line with install.ps1

- **Naming (fix #5):** change user-facing strings:
  - L56–59: `"Bundled Everything engine present ('...')"` → `"Bundled Fallback Engine present ('...')"`
  - L66–70: `'Everything license notice present (required for redistribution).'` → `'Fallback Engine license notice present (required for redistribution).'` and the missing variant
  - L49/51: `'Everything is running.'` → `'Fallback Engine is running.'` (and the not-running WARN)
  - Keep variable names / `Everything.exe` path / process name checks as-is (the binary is still `Everything.exe`).
- **Config targeting (fix #2):** doctor.ps1 defines `$openCodeConfig` at L11 but **never uses it** (verified — it's dead code today; doctor.ps1 currently only checks the plugin dir and the `INSTANT_FS_MCP_BINARY` env var, not the config file). Resolve it like install.ps1's `Resolve-OpenCodeConfig`: prefer `opencode.jsonc`, then `opencode.json`, and actually USE it (e.g. verify the `instant-file-search` mcp entry is present). Copy the `ConvertFrom-JsonC` + `Read-JsonConfig` logic from install.ps1 so a JSONC config with comments doesn't throw.
- The smoke test block (L86–104) already exists — keep it. Verify it still works with the installed path.

### 2. `.github/workflows/release.yml` — ship the plugin + doctor.ps1 (fixes #3 and #4)

Current workflow: `build` job (windows-latest, msys2, `cargo build --release --workspace`) uploads one artifact `release-assets` containing the two exes + fallback engine zip/ini/license; `release` job downloads it, stages into `release-files/`, publishes via `softprops/action-gh-release`.

Add:
- **Build job:** after `cargo build`, add a step to build the OpenCode plugin:
  ```yaml
  - name: Build OpenCode plugin
    run: |
      cd plugin
      npm ci
      npm run build
    shell: bash
  ```
  (node/npm are preinstalled on `windows-latest`.) The build emits `plugin/dist/index.js`.
- **Upload artifact:** add these paths to the `release-assets` `path:` list:
  - `plugin/dist/index.js`
  - `plugin/package.json`
  - `plugin/package-lock.json`
  - `scripts/doctor.ps1`
  > Note: only `plugin/dist/` is gitignored (build output; the CI build step produces it, and `actions/upload-artifact` uploads whatever is on disk regardless of gitignore). `plugin/package.json` and `plugin/package-lock.json` are TRACKED in git (verified via `git ls-files` — the `.gitignore` line for `plugin/package-lock.json` is a dead entry), so they are present in the CI checkout. The `if-no-files-found: error` is safe because `plugin/dist/index.js` is guaranteed by the build step that runs immediately before it.
- **Release job staging:** add corresponding `cp` lines (the artifact preserves relative paths, so after `merge-multiple: true` download the files land under `plugin/dist/...` and `scripts/doctor.ps1`):
  - `cp plugin/dist/index.js release-files/instant-file-search-mcp-plugin-index.js`
  - `cp plugin/package.json release-files/instant-file-search-mcp-plugin-package.json`
  - `cp plugin/package-lock.json release-files/instant-file-search-mcp-plugin-package-lock.json`
  - `cp scripts/doctor.ps1 release-files/doctor.ps1`
  These exact asset filenames MUST match what install.ps1 downloads (see `Install-OpenCode` and `Install-Doctor`).
- `files: release-files/*` already globs them.

### 3. `README.md` — update install instructions (fixes #3 and #4 are user-visible)

- The install section currently says the one-liner "installs everything... no Rust toolchain". After this refactor, also mention:
  - A UAC prompt appears for the native indexer (and that if you skip it, searches still work via the Fallback Engine, just slower).
  - OpenCode sub-agents now get the plugin automatically (no manual build).
  - Diagnostics: `& "$env:LOCALAPPDATA\ClayLeopardLabs\EverythingMCP\doctor.ps1"` instead of `.\scripts\doctor.ps1` for one-liner users (keep the checkout path too).
- Keep it nontechnical and friendly (existing style).

### 4. Build + verify (no UAC — the current user is AWAY and CANNOT click UAC)

- Parse-check all PowerShell: `install.ps1`, `doctor.ps1` (use the PowerShell AST parser, like the pattern in this session).
- Build the Rust workspace: `cargo build --release --workspace` (with `C:\msys64\mingw64\bin` and `~\.cargo\bin` on PATH).
- Build the plugin locally: `cd plugin; npm ci; npm run build` (to confirm it compiles and produce a local dist for testing).
- **Re-run the installer with `-SkipElevation -Clients opencode`** (this machine already has Everything running and prior installs present). Confirm it:
  - does NOT attempt UAC;
  - installs/updates the binary + fallback engine;
  - merges the MCP entry into `opencode.jsonc` (NOT a new `opencode.json`), and removes any orphaned `opencode.json` whose only entry is ours;
  - installs the plugin into `~/.config/opencode/plugins/instant-file-search-mcp-plugin/` (with node_modules);
  - prints the "not elevated, run this elevated command" note for the service;
  - passes the built-in `Test-Installation` smoke test.
- **Smoke-test the installed binary** via an MCP handshake (initialize + tools/list + a tool call) to confirm it still works.

### 5. Commit + push + release

- Commit all changes on `master` with a clear message (e.g., "feat: install-experience improvements — self-elevating service, JSONC config targeting, bundled plugin, one-liner diagnostics").
- Push to `origin/master` (repo: `clayleopardlabs/instant-file-search-MCP-server`).
- Re-point the `v1.0.0` tag to the new master tip (delete remote tag via `gh api -X DELETE repos/clayleopardlabs/instant-file-search-MCP-server/git/refs/tags/v1.0.0`, then `git tag -f -a v1.0.0 -m "Release v1.0.0"`, then `git push origin v1.0.0`). This triggers the Release workflow.
- Watch the workflow (`gh run watch`) until green. Confirm the release now has the NEW assets: `instant-file-search-mcp-plugin-index.js`, `instant-file-search-mcp-plugin-package.json`, `instant-file-search-mcp-plugin-package-lock.json`, and `doctor.ps1` (in addition to the existing exes/zip/ini/license).
- If the release job leaves stale assets or fails staging, clean up / fix and re-run.
- **Do NOT register the actual Windows service** (UAC is unavailable). Leave it unregistered and tell the user to run the printed elevated command when they're back:
  `powershell -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\ClayLeopardLabs\EverythingMCP\indexer\register-indexer-service.ps1"`

## Constraints / gotchas

- **Non-interactive shell, no TTY.** Never use `Read-Host` expecting a reply, never open editors. Always pass `-Clients` explicitly to the installer.
- **UAC is OFF-LIMITS during this session.** Use `-SkipElevation` for all installer runs. The service must remain unregistered at the end.
- **The local machine's active opencode config is `opencode.jsonc`** (which itself contains NO comments currently, but treat it as if it might). `opencode.json` should NOT be created; the MCP entry must merge into the jsonc.
- Everything is already running on this machine; the Everything.exe binary name inside the fallback bundle must stay `Everything.exe` (it pairs with `Everything.lng` and the IPC window).
- Keep the existing file/function naming (`$serverName = 'instant-file-search'`, `$serviceName = 'instant-file-search-indexer'`, plugin key `instant-file-search` in the mcp config) — only user-facing DISPLAY strings and asset filenames change.
- The release asset names downloaded by install.ps1 are load-bearing: `instant-file-search-mcp-plugin-index.js`, `instant-file-search-mcp-plugin-package.json`, `instant-file-search-mcp-plugin-package-lock.json`, `doctor.ps1`. They must exist on the release exactly as named.

## Definition of done

- [ ] `doctor.ps1` uses JSONC-aware read + resolves jsonc config + "Fallback Engine" naming.
- [ ] `release.yml` builds the plugin, ships all new assets, staging matches install.ps1's expected filenames.
- [ ] `README.md` updated for UAC prompt, automatic plugin, and one-liner diagnostics path.
- [ ] Installer re-run with `-SkipElevation` verified: entry in `opencode.jsonc`, orphan json removed, plugin installed, smoke test PASS, no UAC attempted.
- [ ] All PowerShell parses, Rust builds, plugin builds.
- [ ] Committed + pushed to `master`; `v1.0.0` re-pointed; Release workflow green; new assets present on the release.
- [ ] Service NOT registered (user away); user told the exact elevated command to run when back.
