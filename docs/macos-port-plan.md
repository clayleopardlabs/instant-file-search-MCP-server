# macOS Port Plan

Goal: the native indexer + MCP server run natively on macOS (native-only —
Everything fallback is Windows-only by construction). Companion doc:
`docs/macos-support.md` (architectural research). This document is the phased
execution plan.

## Environment facts (2026-08-03)

- No Mac hardware on the dev machine. Live macOS testing requires either a Mac
  or GitHub Actions CI on `macos-latest`. Verify target for the plan:
  **GitHub Actions CI on macos-15 / macos-26 (arm64)** for build/test; a real
  Mac later for live FSEvents / launchd / TCC smoke tests.
- The Linux port already did the hard seam work: `types.rs`, `platform.rs`,
  `protocol.rs`, portable query/content/index. macOS adds a third backend to
  the existing seam — a **smaller delta than Linux was**.
- Windows-only crates today: `windows`, `ntfs`, `everything-ipc`,
  `windows-service`. Linux-only additions: `nix`, `rustix`, `sd-notify`.
  Everything else is cross-platform. macOS needs `libc` (already present for
  Linux) plus an FSEvents binding (`objc2-core-services` recommended;
  `fsevent-sys` is deprecated "in favour of" it; `notify` is an alternative
  but pulls a full watcher).

## Architecture: platform abstraction

The codebase already separates portable core from platform backends. The port
adds macOS backends behind the same seams:

| Module | Windows today | Linux backend | macOS backend | Change |
|---|---|---|---|---|
| `indexer/src/index.rs` | in-memory index, keyed `(volume, file_ref)` | keyed `(st_dev, ino)` | keyed `(real devid, fileid)` | Key abstraction (cfg or trait) |
| `indexer/src/query.rs` | query engine | same | same | NFC/case canonicalization (Phase 4) |
| `indexer/src/mft.rs` | `ntfs` crate MFT parse | `walk.rs` (getdents64 + statx) | `walk_macos.rs` (getattrlistbulk) | New file, `cfg(windows)` gate on mft |
| `indexer/src/usn.rs` | USN Change Journal watcher | `fanotify.rs` watcher | `fsevents.rs` watcher | New file |
| `indexer/src/sector_reader.rs` | raw volume reads | not needed | not needed | `cfg(windows)` |
| `indexer/src/pipe.rs` | named pipe (Win32) | Unix socket | **same Unix socket** | Reuse `pipe_unix.rs` verbatim |
| `indexer/src/main.rs` | SCM service host | systemd unit host | launchd LaunchDaemon | cfg'd service bootstrap |
| `indexer/src/scan.rs` | scan orchestration | same | same | Portable as-is |
| `src/everything.rs` | Everything IPC + auto-launch | removed | removed | `cfg(windows)` |
| `src/native.rs` | named-pipe client | Unix socket client | **Unix socket client** | Reuse Linux arm |
| `src/handler.rs` / `src/tools.rs` | MCP surface | same | same | Portable as-is (minus everything) |

## Current implementation status (2026-08-05)

Phases 0–6 are represented in the repository: the macOS target dependencies and
CI workflow exist; `walk_macos.rs` implements bulk metadata enumeration;
`fsevents.rs` implements persistent event-ID replay, extended-data file IDs,
rename handling, and overflow rescans; the Unix socket and native-only routing
are shared with Linux; and launchd packaging plus FDA guidance are present.

The remaining work is validation and release hardening:

- Confirm the workspace builds and tests on `macos-15` in GitHub Actions.
- Run a real Mac smoke test covering APFS enumeration, firmlinks, external
  volumes, FSEvents churn/rename/overflow, launchd restart, and MCP round trips.
- Validate Full Disk Access for the deployed executable or signed bundle; root
  access alone is not sufficient.
- Verify the installer on a clean Mac, including idempotent bootstrap, logging,
  permissions, and uninstall.
- Keep the documented limitations explicit: initial scans are much slower than
  Windows, access to protected locations depends on FDA, and content indexing
  remains bounded by the shared 256 MB store.

## Phases

### Phase 0: Cross-platform build scaffolding (no runtime behavior change)
- **Deps**: add `[target.'cfg(target_os = "macos")'.dependencies]` (libc,
  objc2-core-services with the FSEvents feature). `nix` supports macOS/BSD
  fully; rustix's `getdents64`/`statx`/`io_uring` surface is Linux-gated (fine —
  the macOS walker is libc anyway).
- **Gates**: `#[cfg(target_os = "macos")]` around the macOS
  `walk_macos.rs` and `fsevents.rs`; `platform.rs` re-exports
  `walk_macos`/`fsevents`/`pipe_unix` for the native macOS build.
- **CI**: add `.github/workflows/macos.yml` — macos-15 + macos-26 arm64,
  `cargo build` + `cargo test` both crates. Windows path untouched.
- **Gate**: workspace compiles on all three targets; all portable unit tests
  pass on macOS.

### Phase 1: platform.rs macOS arm
- Re-export `discover_volumes`/`scan_volume` from `walk_macos`, `journal_tails`/
  `watch_all` from `fsevents`, `PipeServer` from `pipe_unix` (same Unix socket
  code as Linux — no new transport).
- `volume_of` on macOS: mount-aware, like Linux (inodes collide across
  filesystems). Use `getmntinfo` to map mount points → devid.
- **Gate**: workspace compiles for macOS; Windows parity battery still green.

### Phase 2b: Enumeration backend (`walk_macos.rs`)
- Mount discovery: `getmntinfo` → mounts with devid, fs type, mount point;
  filter `apfs` + `MNT_LOCAL`; skip `/System/Volumes/*` (Data reached via
  firmlinks from `/`) and `@`-snapshot mounts; index `/` + `/Volumes/*`.
- Walk: `getattrlistbulk` with `FSOPT_RETURN_REALDEV` (batched statx
  equivalent — name + fileid + real devid + crtime + modtime + size + mode +
  flags + objtype in one syscall batch). ~4.7x faster than readdir+stat,
  1,600x fewer syscalls.
- Emit the same scan item shape: path, name, size, mtime, btime (crtime —
  native on macOS), atime, mode/type, fileid, real devid.
- Content-store fill loop: reuse existing logic (same 256MB budget — known
  limitation carries over).
- **Gate**: `scan` one-shot mode works on macOS; index contents match a
  reference walk on a fixture tree (unit test with a temp dir).

### Phase 3: Change tracking (`fsevents.rs`)
- Per-volume FSEventStream with `kFSEventStreamCreateFlagFileEvents` +
  `kFSEventStreamCreateFlagUseExtendedData` (file IDs for rename
  disambiguation).
- Reason mapping → ChangeEvent: created/modified/renamed/deleted to the
  existing reason strings so `recent_changes` and `reasons=` work unchanged.
- `since=` replay via `FSEventStreamEventId`; USN watermark → (volume UUID +
  event ID). `MustScanSubDirs`/`UserDropped`/`KernelDropped` → rescan signal.
- Persistence: user-space ring buffer adds timestamps (FSEvents delivers none);
  append-only event journal so `recent_changes(since=X)` survives restart.
- **Gate**: a fixture churn test (create/modify/rename/delete) produces the
  right events in the ring buffer; restart + gap → rescan triggers.

### Phase 5: Service host (launchd)
- `main.rs`: macOS bootstrap — launchd foreground (no daemonize), readiness via
  connect+ping (no `Type=notify` equivalent). Console modes `serve`/`scan`
  preserved.
- `scripts/install-macos.sh`: build, install to `/usr/local/lib/instant-file-search/`,
  launchd plist in `/Library/LaunchDaemons` (root-owned), `launchctl bootstrap
  system`, FDA guidance (package as signed app bundle with a real
  CFBundleIdentifier; ad-hoc `codesign -s -` for dev).
- Everything fallback: fully removed on macOS; `search_status` reports
  native-only.
- **Gate**: `launchctl bootstrap` → pipe reachable → search_status healthy on a
  Mac.

### Phase 6: Semantic parity mapping (macOS specifics)
- **Case/normalization (the big one)**: default APFS is case-insensitive AND
  normalization-insensitive but byte-preserving — `readdir` returns a mix of
  NFC/NFD names. Canonicalize index AND query keys to NFC + Unicode lowercase
  (`unicode-normalization` crate + `to_lowercase`, NOT `to_ascii_lowercase`);
  dedup on canonical key. Detect per-volume case via `pathconf(_PC_CASE_SENSITIVE)`
  / `VOL_CAP_FMT_CASE_SENSITIVE`.
- `attrib:` / `is:`: HIDDEN ← dotfile | `UF_HIDDEN`; DIRECTORY ← `S_IFDIR`;
  READONLY ← `!S_IWUSR` or immutable; SYSTEM/ARCHIVE have no faithful source —
  drop (never match, correct Everything-equivalent behavior).
- `frn:` → `(real devid, fileid)`, scoped by mount. APFS inodes are monotonic
  ~63-bit and NOT recycled — no reuse guard needed.
- Dates: date_created ← crtime (native), date_modified ← mtime, date_accessed ←
  atime.
- **Gate**: full query-surface unit tests pass on macOS (same fixtures).

### Phase 7: Verification
- CI (macos-15 + macos-26 arm64): full `cargo test` both crates + fixture
  tests. `launchctl bootstrap system` over passwordless sudo works headless;
  keep fixtures outside TCC-protected dirs; foreground execution inherits the
  image's pre-granted `/bin/bash` FDA for any protected-path test; FSEvents is
  CI-testable without privileges.
- Live smoke on a real Mac: full scan, FSEvents churn, launchd service, MCP
  round-trip, FDA grant flow.
- Windows regression: parity battery still green; UAT spot-check.

### Phase 8: Docs + release
- README: macOS section (native-only, no Everything), install instructions.
- docs/build-macos.md (toolchain, deps: libc/objc2-core-services), docs/
  macos-support.md stays as the architecture rationale.
- `scripts/install-macos.sh` + launchd plist in-tree.
- Commit per phase per rule #79 (update README, commit, push).

## Risks / decisions to confirm
1. **TCC / Full Disk Access**: root ≠ FDA. The daemon needs a manual,
   per-binary/bundle FDA grant in System Settings; grants are silently
   revocable by OS upgrades (detect via `EPERM`). Package as a signed app
   bundle with a real CFBundleIdentifier (bare binaries refused on some macOS
   versions). launchd jobs don't inherit terminal TCC. (Required — no way
   around it.)
2. **Expectation gap**: initial index of 2-3M files is ~1-2 min via
   getattrlistbulk (no $MFT-class 2-4 s path). Must ship FSEvents incremental
   sync with it or the first scan looks broken.
3. **Volume-group devid**: System/Data boot volumes share a logical devid —
   must use `FSOPT_RETURN_REALDEV` / `ATTR_CMNEXT_REALDEVID` for physical
   per-volume values.
4. **NFC/NFD normalization**: byte-exact `to_ascii_lowercase` matching silently
   misses NFD-stored names — canonicalize both index and query keys.
5. **Content store**: 256MB budget limitation carries over unchanged (known
   gap).

## Rough effort
- Phase 0: scaffolding + CI — ~1 session
- Phase 2b: walk_macos.rs + mount discovery + fixtures — ~1-2 sessions
- Phase 4: fsevents.rs + journal — ~2 sessions (highest complexity)
- Phase 5: launchd + install-macos.sh — ~1 session
- Phase 6: NFC/casefold + attribute mapping — ~1 session
- Phase 7-8: verification + docs/release — ~1-2 sessions
