# Linux Port Plan

Goal: the native indexer + MCP server run natively on Linux (native-only — Everything
fallback is Windows-only by construction). Companion doc: `docs/linux-support.md`
(architectural research). This document is the phased execution plan.

## Environment facts (2026-08-03)

- WSL2 is **not installed** on the dev machine — live Linux testing requires either
  WSL2 install (admin + likely reboot) or a remote Ubuntu box. Verify target for the
  plan: **GitHub Actions CI on ubuntu-latest** (repo is on GitHub) for build/test;
  WSL2 later for live fanotify/systemd smoke tests.
- Windows-only crates today: `windows`, `ntfs`, `everything-ipc`, `windows-service`.
  Everything else (`tokio`, `serde`, `serde_json`, `regex`, `anyhow`, `tracing`,
  `rmcp`, `schemars`) is cross-platform.
- Dev machine PATH: cargo lives at `C:\Users\sophi\.cargo\bin` (not on PATH);
  mingw64 WinLibs toolchain for the Windows GNU target.

## Architecture: platform abstraction

The codebase already separates portable core from platform backends. The port adds
Linux backends behind the same seams:

| Module | Windows today | Linux backend | Change |
|---|---|---|---|
| `indexer/src/index.rs` | in-memory index, keyed `(volume, file_ref)` | same, keyed `(st_dev, ino)` | Key abstraction (cfg or trait) |
| `indexer/src/query.rs` | query engine | same | Path separator normalization is platform-aware |
| `indexer/src/mft.rs` | `ntfs` crate MFT parse | `walk.rs` (getdents64 + statx) | New file, `cfg(windows)` gate on mft |
| `indexer/src/usn.rs` | USN Change Journal watcher | `fanotify.rs` watcher | New file |
| `indexer/src/sector_reader.rs` | raw volume reads | not needed (walk path) | `cfg(windows)` |
| `indexer/src/pipe.rs` | named pipe (Win32) | Unix domain socket (tokio) | Connection seam |
| `indexer/src/main.rs` | SCM service host | systemd unit host | cfg'd service bootstrap |
| `indexer/src/scan.rs` | scan orchestration | same | Portable as-is |
| `src/everything.rs` | Everything IPC + auto-launch | removed | `cfg(windows)` |
| `src/native.rs` | named-pipe client | Unix socket client | Connection seam |
| `src/handler.rs` / `src/tools.rs` | MCP surface | same | Portable as-is (minus everything) |

## Phases

### Phase 0: Cross-platform build scaffolding (no runtime behavior change)
- **Deps**: move `windows`, `ntfs`, `everything-ipc`, `windows-service` into
  `[target.'cfg(windows)'.dependencies]`; add `[target.'cfg(target_os = "linux")'.dependencies]`
  (libc, rustix for statx, inotify crate, sd-notify).
- **Gates**: `#[cfg(windows)]` around everything.rs, mft.rs, usn.rs, sector_reader.rs,
  Windows service bootstrap. Linux stubs return a clear "not yet implemented" for the
  backend modules so the workspace compiles.
- **CI**: add `.github/workflows/linux.yml` — ubuntu-latest, `cargo build` + `cargo test`
  both crates. Windows path untouched (existing build still works).
- **Gate**: workspace compiles on both targets; all portable unit tests (query engine,
  index, schemas, tools) pass on Linux.

### Phase 1: Portable core de-Noising
- Path normalization: `lower_path` currently assumes backslashes. Abstract to
  `path_sep()` / normalize user input (`/` and `\` both accepted on Linux input,
  stored with native separators). Touch query.rs (path scope, Token::Path, excludes)
  and index.rs.
- Volume identity: replace drive-letter volume strings with a
  `VolumeId { mount_point, st_dev, f_fsid }` abstraction; Windows keeps `C:` style,
  Linux uses mount points.
- **Gate**: parity battery still green on Windows (no behavior change on Windows).

### Phase 2: Enumeration backend (`walk.rs`)
- Mount discovery: parse `/proc/self/mountinfo` → mounts with st_dev, fs type, mount
  point; default exclusions (proc, sysfs, devpts, cgroup*, tmpfs, overlay, squashfs,
  fuse.portal, etc.).
- Walk: `getdents64` with large buffers (1-4 MiB), use `d_type` to skip stat on type
  decisions, batch `statx` (`STATX_BASIC_STATS | STATX_BTIME`) via a worker pool
  (rayon or tokio tasks), parallel by subtree (fd/ferret pattern).
- Emit the same scan item shape mft.rs produces: path, name, size, mtime, btime,
  atime, mode/type, ino, st_dev. Use `rustix` for statx (std has no btime on Linux).
- Content-store fill loop: reuse existing logic against walk output (same 256MB
  budget semantics — the known limitation carries over).
- **Gate**: `scan` one-shot mode works on Linux; index contents match a reference
  `find`-style walk on a fixture tree (unit test with a temp dir).

### Phase 3: Change tracking (`fanotify.rs`)
- `fanotify_init(FAN_CLASS_NOTIF | FAN_REPORT_DFID_NAME | FAN_REPORT_TARGET_FID)`,
  `FAN_MARK_FILESYSTEM` per mount for CREATE/DELETE/MOVED_FROM/MOVED_TO/RENAME/ATTRIB.
- Reason mapping → ChangeEvent: FAN_CREATE→created, FAN_CLOSE_WRITE/MODIFY→modified,
  FAN_DELETE→deleted, FAN_RENAME / MOVED pair→renamed. Map mask bits to the existing
  reason strings (created/modified/renamed/deleted) so `recent_changes` and the
  `reasons=` filter work unchanged.
- Parent resolution: resolve `(fsid, handle)` via `open_by_handle_at` + `fstatat`,
  update the `(st_dev, ino) → path` map. FAN_RENAME gives old+new parent+name
  atomically.
- Persistence: append-only event journal (sequence number per daemon, batched fsync)
  so `recent_changes(since=X)` works after daemon restart; detect downtime gap by
  comparing last-persisted seq + daemon uptime → rescan policy (full or
  mtime-incremental). `FAN_Q_OVERFLOW` → rescan signal.
- Fallback: per-directory inotify marks for unprivileged mode and btrfs-subvol
  `EXDEV` cases (Tracker's model); recursive-move synthetic-create handling (Baloo
  bug 342224 lesson).
- **Gate**: a fixture churn test (create/modify/rename/delete) produces the right
  events in the ring buffer; restart + gap → rescan triggers.

### Phase 4: Transport (Unix socket)
- `pipe.rs`: factor the server behind a small trait or cfg'd impl — Windows named
  pipe (Win32) vs `tokio::net::UnixListener` with the same newline-framed JSON
  protocol. Keep-alive behavior preserved.
- `src/native.rs`: cfg'd connect (named pipe vs `UnixStream`).
- **Gate**: MCP server ↔ indexer round-trip works over the Unix socket on Linux.

### Phase 5: Service host
- `main.rs`: Linux bootstrap — daemonize/systemd foreground, `sd_notify` READY,
  `AmbientCapabilities=CAP_SYS_ADMIN` for FAN_MARK_FILESYSTEM (systemd unit),
  capability-dropping after setup, console modes `serve`/`scan` preserved.
- `scripts/install-linux.sh`: build, install to `/usr/local/lib/instant-file-search/`,
  systemd unit + enable, config (env vars ported: log filter, timeouts).
- Everything fallback: fully removed on Linux; `search_status` reports native-only.
- **Gate**: `systemctl start` → pipe reachable → search_status healthy on a Linux box.

### Phase 6: Semantic parity mapping (Linux specifics)
- `attrib:` / `is:`: map statx mode + d_type: is:folder (S_IFDIR), is:file (S_IFREG),
  is:hidden (dotfile convention), is:system (mode semantics — define or skip).
- `frn:` → `(st_dev, st_ino)`, scoped by mount; document best-effort (no reuse guard).
- Dates: date_created ← btime (caveat: unavailable on ext3/tmpfs/NFS), date_modified ←
  mtime, date_accessed ← atime (**unreliable on relatime/noatime defaults — document**).
- `sort:`, filters, wildcards, operators: already portable, no change.
- **Gate**: full query-surface unit tests pass on Linux (same fixtures as Windows).

### Phase 7: Verification
- CI (ubuntu-latest): full `cargo test` both crates + Phase 2/3 fixture tests.
- Live smoke on WSL2 (once installed) or a remote Ubuntu box: full scan of a real
  tree, fanotify churn, service via systemd, MCP round-trip.
- UAT loop (context-free subagents) if a live Linux MCP instance is reachable.
- Windows regression: parity battery still green; UAT spot-check.

### Phase 8: Docs + release
- README: Linux section (native-only, no Everything), install instructions.
- docs/build-linux.md (toolchain, deps: libc/rustix/inotify), docs/linux-support.md
  stays as the architecture rationale.
- `scripts/install-linux.sh` + systemd unit in-tree.
- Commit per phase per rule #79 (update README, commit, push).

## Risks / decisions to confirm
1. **fanotify permissions**: FAN_MARK_FILESYSTEM needs root. Acceptable for a service
   install (systemd + AmbientCapabilities), with unprivileged per-dir inotify as the
   fallback? (Recommended: yes, mirror Tracker.)
2. **atime**: date_accessed is unreliable on Linux defaults. Ship it documented, or
   omit? (Recommend: ship, documented.)
3. **Raw ext4 reads**: skip for v1 (walk is fast enough on SSD; raw reads are
   ext4-only, root, and unsafe mounted-rw). Revisit only if cold-HDD latency matters.
4. **Content store**: 256MB budget limitation carries over unchanged (known gap).
5. **btrfs**: fanotify filesystem marks can fail (EXDEV) — inotify fallback covers it;
   index keys use `stx_subvol` where available.

## Rough effort
- Phase 0-1: scaffolding + de-noise (small, CI-heavy) — ~1 session
- Phase 2: walk.rs + mount discovery + fixtures — ~1-2 sessions
- Phase 3: fanotify.rs + journal + fallback — ~2-3 sessions (highest complexity)
- Phase 4-5: transport + systemd — ~1 session
- Phase 6-7: semantic mapping + verification — ~1-2 sessions
- Phase 8: docs/release — ~1 session
