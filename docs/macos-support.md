# macOS Support Assessment

Feasibility research for porting the native indexer to macOS (three research
lanes: change tracking, enumeration + index key, service host + transport).
Status: assessment only — no code written.

## Executive summary

The macOS port is **feasible and structurally mirrors the Linux port**: the
`platform.rs` seam already isolates the Windows/Linux backends, and macOS plugs
into the same seam. Two pillars have direct replacements (FSEvents for change
tracking, `getattrlistbulk` for enumeration), the index key `(real devid,
fileid)` is sound, and the transport (`pipe_unix.rs`) is reusable verbatim.

Three genuinely new problems, none blocking:

1. **TCC / Full Disk Access** — root does NOT equal FDA. The daemon needs a
   manual, per-binary/bundle FDA grant in System Settings, and launchd jobs do
   not inherit a terminal's TCC state. This is the one hard requirement for
   reading user data.
2. **Case + Unicode normalization** — default APFS is case-insensitive AND
   normalization-insensitive but byte-preserving, so `readdir` returns a mix of
   NFC/NFD names. The query engine's byte-exact `to_ascii_lowercase` matching
   silently misses NFD-stored names; both index and query keys must be
   canonicalized (NFC + Unicode lowercase).
3. **No raw-APFS fast path** — the NTFS $MFT single-table scan has no macOS
   equivalent (APFS inodes live in a copy-on-write b-tree; default installs are
   FileVault-encrypted). Initial index of 2-3M files is ~1-2 minutes via
   `getattrlistbulk`, not 2-4 seconds. Must ship FSEvents-driven incremental
   sync or the first scan looks broken.

## 1. Change tracking (USN journal replacement)

**FSEvents** is the replacement — path-based, per-volume, with a monotonic
event ID and a persistent on-disk store (`.fseventsd`, at the Data volume root
on Big Sur+). The USN pillar maps almost 1:1 onto what was already built for
Linux:

- USN watermark → **(volume UUID + event ID)**; replay with the `since`
  parameter (`FSEventStreamEventId`).
- File-level events with `kFSEventStreamCreateFlagFileEvents` (10.7+).
- **Renames**: reported as two unpaired `kFSEventStreamEventFlagItemRenamed`
  events with no from/to pairing — disambiguate with `UseExtendedData` file
  IDs (`kFSEventStreamCreateFlagUseExtendedData`, macOS 10.13+).
- **Dropped events**: `MustScanSubDirs`, `UserDropped`, `KernelDropped` flags
  → full rescan of the affected subtree (same protocol as fanotify overflow).
- FSEvents delivers **no timestamps** — the existing user-space ring buffer
  adds them (exactly the fanotify approach on Linux).
- Root daemons receive all events ("Only applications running as root are
  guaranteed to receive all events"). An unprivileged watcher sees a reduced
  set — run the watcher as root, same as Linux.

FSEvents is coarser than fanotify: latency-coalesced batches (default ~1 s),
directory-level paths unless FileEvents is on, and synthetic-firmlink path
quirks. The robust pattern (watchexec et al.) is to treat events as "rescan
this subtree" rather than precise per-file deltas. **Bottom line:** persistent
tailable journal equivalent exists (event ID + `.fseventsd`), with the same
user-space journal requirement for `recent_changes since=X` as Linux.

## 2. Fast enumeration ($MFT replacement)

**There is no raw-APFS equivalent of the $MFT read** — APFS has no single
inode table (inodes live in a CoW object-map b-tree) and default installs are
FileVault-encrypted (raw volume reads return ciphertext). Do not attempt it.

The right tool is **`getattrlistbulk(2)`** (OS X 10.10+): the statx-style
batched call. One syscall returns each directory entry's name PLUS fileid
(= inode), devid, fsid, crtime (birthtime — native on macOS, unlike Linux
btime), modtime, size, mode, flags, objtype, parentid, linkcount. ~4.7x faster
than `readdir`+per-file `stat` and 1,600x fewer syscalls (measured). Expected
cold full-volume walk of 2-3M files: ~1-2 minutes on SSD.

`statx` and rustix `getdents64` are Linux-gated; the macOS walker is a libc
`getattrlistbulk` (or `readdir`+`fstatat` at minimum) variant. Attribute-wise
the Linux statx mapping is nearly 1:1 (`stx_ino→ATTR_CMN_FILEID`,
`stx_btime→ATTR_CMN_CRTIME`, mode→`ATTR_CMN_ACCESSMASK`, flags→`ATTR_CMN_FLAGS`).

## 3. Index key (inode vs NTFS FRN)

`(real devid, ATTR_CMN_FILEID)` is a sound index key:

- `ATTR_CMN_FILEID` = `st_ino`, explicitly "unique within its mounted volume",
  persistent across mounts.
- APFS inode numbers are **monotonically incrementing ~63-bit values, not
  recycled** (`VOL_CAP_FMT_PATH_FROM_ID` implies "object IDs ... persistent and
  not recycled") — no NTFS-sequence-guard needed.
- `clonefile` creates a fresh inode; hard links share one — correct semantics
  mirroring Linux. Safe saves (write temp + rename) create a new inode per
  save; track `ATTR_CMN_DOCUMENT_ID` (via `FSOPT_ATTR_CMN_EXTENDED`) if
  save-stable identity matters.
- **Critical trap:** within the boot volume group, System and Data may present
  a **shared logical devid/fsid**. Request `FSOPT_RETURN_REALDEV` /
  `ATTR_CMNEXT_REALDEVID` for physical per-volume values. A mount-path key is
  unsafe (mount names change; the union `/` spans two volumes).
- `path_by_ref`'s Windows purpose (USN parent-FRN resolution) has no macOS
  analog (FSEvents is path-based) — keep the map for hard-link grouping and
  change detection only.

## 4. Service host, TCC/FDA, and transport

**launchd LaunchDaemon** replaces both SCM and systemd:

- Plist in `/Library/LaunchDaemons` (root-owned, not group/world-writable);
  auto-loaded at boot. `launchctl bootstrap system <plist>` for immediate
  start, `bootout system/<Label>` for uninstall. Modern API only (`load`/
  `unload` are legacy).
- Shape: `Label`, `ProgramArguments`, `RunAtLoad` + `KeepAlive`,
  `ProcessType=Background`, `StandardOutPath`/`StandardErrorPath`. No
  `WatchPaths` (launchd warns it is race-prone; KeepAlive makes it pointless).
- **No `Type=notify` equivalent** — readiness is implicit: connect + ping with
  timeout (how Windows already works mid-scan). Optional upgrade: launchd
  `Sockets` key + `launch_activate_socket` FFI (shadowsocks-rust has a
  production-grade Rust implementation) so launchd owns the socket lifecycle
  and the client can never connect before the daemon exists.

**TCC / FDA is the hard requirement.** Root ≠ FDA. The daemon binary/bundle
must be granted Full Disk Access once in System Settings
(Privacy & Security → Full Disk Access). Nuances:

- Grants are per-binary/bundle, system-wide, and silently revocable by OS
  upgrades (detect via `EPERM` and re-educate).
- Package the daemon as a **signed app bundle with a real CFBundleIdentifier** —
  on some macOS versions the FDA picker refuses bare binaries (macOS 26.2
  regression). Children inherit the grant, so a bundled daemon works.
- launchd jobs do NOT inherit a terminal's TCC state — no terminal trick works.
- Signing: ad-hoc `codesign -s -` suffices to run on Apple Silicon; a stable
  identity is needed for a persistent FDA grant (ad-hoc identity = content
  hash, breaks the grant on rebuild). Notarization only matters for
  distributed/quarantined artifacts. Never set the App Sandbox entitlement.
- A manually installed plist does not trigger the "Background Items Added"
  prompt (that path is SMAppService/app-bundle-associated).

**Transport:** reuse `pipe_unix.rs` verbatim — `/tmp/instant-file-search-indexer.sock`
(mode 0666) or `/var/run/`. macOS `sun_path` limit is 104 bytes (vs 108 Linux);
our 36-byte path is trivially safe. No abstract sockets on macOS; socket
connects are TCC-free.

## Porting plan (what would need to change)

The Linux port already did the hard seam work (`types.rs`, `platform.rs`,
`protocol.rs`, portable query/content/index). macOS adds a third backend to the
existing seam:

| Phase | Work | Key change |
|---|---|---|
| 0 | Build scaffolding | `cfg(target_os = "macos")` gates; add `libc` + FSEvents crate (`objc2-core-services` recommended — `fsevent-sys` is deprecated "in favour of" it; `notify` crate is an alternative but pulls a full watcher) |
| 1 | platform.rs macos arm | Re-export `walk_macos`/`fsevents`/`pipe_unix` (same Unix socket code) |
| 2 | Enumeration | `walk_macos.rs`: `getattrlistbulk` + `FSOPT_RETURN_REALDEV`; `(real devid, fileid)` keys; skip `/System/Volumes/*` + `@`-snapshot mounts (index `/` + `/Volumes/*`; Data reached via firmlinks from `/`); detect case behavior per volume (`pathconf(_PC_CASE_SENSITIVE)` / `VOL_CAP_FMT_CASE_SENSITIVE`) |
| 3 | Change tracking | `fsevents.rs`: FID-style per-volume streams, `since=` replay, `MustScanSubDirs`/dropped → rescan, `UseExtendedData` file IDs for renames, user-space ring buffer for timestamps |
| 4 | Case/normalization | Canonicalize index AND query keys to NFC + Unicode lowercase (`unicode-normalization` crate + `to_lowercase`, NOT `to_ascii_lowercase`); dedup on canonical key; attribute mapping (HIDDEN ← dotfile \| `UF_HIDDEN`; READONLY ← `!S_IWUSR` or immutable; DIRECTORY ← `S_IFDIR`; SYSTEM/ARCHIVE have no faithful source — drop) |
| 5 | Service host | launchd plist + `install-macos.sh` (bootstrap/bootout, plist ownership, FDA guidance, bundle packaging); READY-via-ping |
| 6 | CI | `.github/workflows/macos.yml`: `launchctl bootstrap system` over passwordless sudo works headless; keep fixtures outside TCC-protected dirs; foreground execution (inherits the image's pre-granted `/bin/bash` FDA) for any protected-path test; FSEvents is CI-testable without privileges; pin `macos-15` + `macos-26` arm64 (`macos-14` retired Nov 2026); target macOS 13+ |
| 7 | Docs/release | README macOS section, `docs/build-macos.md`; no Everything fallback (native-only, as on Linux) |

**Crate notes:** `nix` supports macOS/BSD fully; rustix's `getdents64`/`statx`/
`io_uring` surface is Linux-gated (fine — walker is libc anyway). Reusable from
Linux untouched: `pipe_unix.rs`, `protocol.rs`, `types.rs`, `platform.rs`
seam, query/content/index modules. New code: `walk_macos.rs` + `fsevents.rs`.

## Conclusion

macOS is a viable third target with a **smaller delta than Linux was**:
transport, protocol, portable core, and attribute plumbing all carry over, and
the change-tracking pillar (FSEvents) is more complete than fanotify (native
persistent journal, no CAP_SYS_ADMIN requirement). The real work is the
enumerating walker (`getattrlistbulk`), the FSEvents backend, NFC/case
canonicalization, and the FDA/launchd install story. The Everything fallback
stays Windows-only; macOS is native-only by construction, same as Linux.

**Top risks (ranked):** (1) TCC/FDA grants — per-binary, manual, revocable;
failure mode is silent `EPERM`. (2) Expectation gap — 1-2 min initial index
vs 2-4 s on Windows; needs incremental sync shipped with it. (3) Volume-group
devid + NFC/NFD normalization traps — both produce silent wrong results if the
canonicalization/realdev logic is wrong. (4) FSEvents path quirks across
synthetic firmlinks — rescan-subtree discipline required.