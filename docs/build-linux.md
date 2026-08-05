# Building and Running on Linux

The native indexer and MCP server build and run on Linux. This page covers
building from source, installing as a systemd service, and the known gaps
versus the Windows build.

## Prerequisites

- Ubuntu 24.04 LTS recommended (kernel 6.8). Kernel 5.17+ is required for
  full change-tracking support (`FAN_RENAME`); 5.13+ gives unprivileged
  fanotify with FID.
- Rust toolchain with the `x86_64-unknown-linux-gnu` target:
  ```sh
  rustup target add x86_64-unknown-linux-gnu
  ```
- `cargo`, `gcc`/`clang` (for the `ring`/`windows`-free build), `pkg-config`.

## Build

```sh
cargo build --release --workspace
```

Outputs:

- `target/release/instant-file-search-mcp-server` — the MCP server
- `target/release/instant-file-search-indexer` — the native indexer

Cross-checking from Windows (no Linux linker needed):

```sh
cargo check --workspace --target x86_64-unknown-linux-gnu
```

## Install

```sh
sudo bash scripts/install-linux.sh
```

The script:

1. Builds both binaries in release mode.
2. Installs them under `/usr/local/lib/instant-file-search/`.
3. Installs the systemd unit `instant-file-search-indexer.service`
   (`Type=notify`, `AmbientCapabilities=CAP_SYS_ADMIN` for fanotify).
4. Enables and starts the service.
5. Registers the MCP client with OpenCode (`~/.config/opencode/opencode.jsonc`
   or `.json`), including the oh-my-opencode-slim sub-agent `mcps` patch.

## Runtime

The indexer listens on a Unix socket at `/tmp/instant-file-search-indexer.sock`
(mode 0666, so the user-level MCP server can connect). The systemd unit uses
`Type=notify`; the indexer sends `READY=1` once the socket is bound and
`STOPPING` on shutdown.

Manual run (foreground, for diagnostics):

```sh
sudo /usr/local/lib/instant-file-search/instant-file-search-indexer serve
```

One-shot scan diagnostic:

```sh
sudo /usr/local/lib/instant-file-search/instant-file-search-indexer scan
```

## Linux backend pillars

| Windows | Linux |
|---|---|
| $MFT raw scan | `getdents64` walk + `statx` (inode = `file_ref`, btime = created) |
| USN Change Journal | fanotify FID-mode marks (root/CAP_SYS_ADMIN; no inotify fallback yet) |
| Named pipe | Unix socket `/tmp/instant-file-search-indexer.sock` |
| Everything fallback | Not used — native is the only engine |
| Windows service (SCM) | systemd `Type=notify` + sd_notify |

## Known gaps (Linux)

- **fanotify needs root.** The systemd unit grants `CAP_SYS_ADMIN` via
  `AmbientCapabilities`. There is currently no inotify fallback; unprivileged
  execution fails clearly instead of providing incomplete change tracking.
- **`date_accessed` is unreliable** on Linux defaults (relatime/noatime
  mounts). `date_accessed` queries may be stale or zero.
- **Content store**: the 256MB bounded content store carries over unchanged.
- **btrfs**: fanotify `EXDEV` caveat applies; there is no fallback yet.
- **No Everything fallback**: on Linux the native indexer is the only search
  path, so `search_status` reports the native engine only.

## CI

`.github/workflows/linux.yml` runs `cargo check`, `cargo test`, and a release
build on `ubuntu-latest` for every push/PR, plus `shellcheck` on
`install-linux.sh` and a Python syntax check on `register-linux-client.py`.
