# Building and Running on macOS

The native indexer and MCP server build and run on macOS (Apple Silicon /
arm64). This page covers building from source, installing as a launchd
daemon, the Full Disk Access (TCC) requirement, and the known gaps versus the
Windows build. Architecture rationale lives in `docs/macos-support.md`.

## Prerequisites

- macOS 13+ (Apple Silicon recommended; the port targets `aarch64-apple-darwin`).
- Rust toolchain with the `aarch64-apple-darwin` target:
  ```sh
  rustup target add aarch64-apple-darwin
  ```
- `cargo`, the Xcode command line tools (`xcode-select --install`), `python3`.

## Build

```sh
cargo build --release --workspace
```

Outputs:

- `target/release/instant-file-search-mcp-server` — the MCP server
- `target/release/instant-file-search-indexer` — the native indexer

Cross-checking from another OS (no macOS linker needed):

```sh
cargo check --workspace --target aarch64-apple-darwin
```

## Install

```sh
sudo bash scripts/install-macos.sh
```

The script:

1. Builds both binaries in release mode.
2. Installs them under `/usr/local/lib/instant-file-search/`.
3. Installs the launchd daemon `com.clayleopardlabs.instant-file-search`
   (`/Library/LaunchDaemons/com.clayleopardlabs.instant-file-search.plist`)
   and bootstraps it with `launchctl bootstrap system`.
4. Registers the MCP client with OpenCode (`~/.config/opencode/opencode.jsonc`
   or `.json`), including the oh-my-opencode-slim sub-agent `mcps` patch.

### Full Disk Access (TCC) — required manual step

Root does NOT imply Full Disk Access. The daemon must be granted FDA to scan
protected locations (Desktop, Documents, Downloads, iCloud, Photos, Mail, ...):

1. System Settings > Privacy & Security > Full Disk Access
2. Add `/usr/local/lib/instant-file-search/instant-file-search-indexer`.
   If a bare binary is refused on your macOS version, wrap it in a signed app
   bundle with a real `CFBundleIdentifier` (ad-hoc `codesign -s -` works for
   dev) and grant FDA to the bundle instead.
3. Restart the daemon:
   ```sh
   sudo launchctl kickstart -k system/com.clayleopardlabs.instant-file-search
   ```

Grants are silently revocable by OS upgrades — if searches silently miss files
after an upgrade, re-check this setting.

## Runtime

The indexer listens on a Unix socket at `/tmp/instant-file-search-indexer.sock`
(mode 0666, so the user-level MCP server can connect). Unlike systemd, launchd
has no readiness-notification protocol; the MCP server determines readiness by
connect+ping once the daemon is up.

Manual run (foreground, for diagnostics):

```sh
sudo /usr/local/lib/instant-file-search/instant-file-search-indexer serve
```

One-shot scan diagnostic:

```sh
sudo /usr/local/lib/instant-file-search/instant-file-search-indexer scan
```

Logs: `/var/log/instant-file-search-indexer.log`.

## macOS backend pillars

| Windows | macOS |
|---|---|
| $MFT raw scan | `getattrlistbulk` walk (`FSOPT_RETURN_REALDEV`; `fileid` = `file_ref`, `crtime` = created) |
| USN Change Journal | FSEvents (persistent per-device journal, `since=` replay, `UseExtendedData` for renames) |
| Named pipe | Unix socket `/tmp/instant-file-search-indexer.sock` |
| Everything fallback | Not used — native is the only engine |
| Windows service (SCM) | launchd LaunchDaemon (readiness via connect+ping) |

## Known gaps (macOS)

- **Full Disk Access is mandatory and manual.** Root != FDA; the daemon needs
  a per-binary/bundle grant in System Settings, and grants are silently
  revocable by OS upgrades (detect via `EPERM` on protected paths). See the
  app-bundle discussion in `docs/macos-support.md`.
- **Initial index is slower than Windows.** A full scan of 2-3M files takes
  ~1-2 min (no $MFT-class 2-4 s acceleration). FSEvents incremental sync runs
  alongside, so subsequent searches stay current.
- **NFC/NFD normalization.** Default APFS is normalization-insensitive but
  byte-preserving; names may be stored in a mix of NFC/NFD. Case-insensitive
  matches canonicalize both index and query keys.
- **`attrib:` letters without a faithful source never match**: SYSTEM/ARCHIVE
  have no APFS equivalent (correct Everything-equivalent behavior: match
  nothing). HIDDEN, DIRECTORY, READONLY are mapped from dotfiles /
  `UF_HIDDEN` / mode bits.
- **Content store**: the 256MB bounded content store carries over unchanged.
- **No Everything fallback**: on macOS the native indexer is the only search
  path, so `search_status` reports the native engine only.

## CI

`.github/workflows/macos.yml` runs `cargo check`, `cargo test`, and a release
build on `macos-15` for every push/PR, plus `shellcheck` on
`install-macos.sh`, a Python syntax check on `register-linux-client.py`, and
`plutil -lint` on the launchd plist.
