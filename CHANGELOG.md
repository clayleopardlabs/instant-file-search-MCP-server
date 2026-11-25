# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions use
`MAJOR.MINOR.PATCH`.

## [1.0.23] - 2026-11-24

A performance and correctness release. No API change, and no change to query
semantics beyond the `path` fixes below: queries that returned the wrong answer
before now return the right one.

### Fixed

- **Filling the content cache no longer pins a core for an hour.** Three
  independent superlinear costs were removed, found in sequence:
  - the disk-mode candidate page re-sorted the whole `files` table for every
    512-row page, instead of range-scanning the `path` primary key: 0.71 s per
    page over 1.86 M rows, about 3,639 pages, so roughly 43 CPU minutes of
    scanning per pass, repeated at every start because `serve()` clears the
    content cache (#14);
  - each stored file opened its own transaction and ran
    `SELECT SUM(byte_count) FROM file_contents` to enforce the budget, a full
    scan of the content table per file, which held the fill to 13 to 45 files per
    second (#16);
  - the batched budget read still scanned every blob, so throughput fell as the
    cache grew (#19).

  On the Linux host, against a 1.94 M entry index, the pass went from "still
  running after 80 minutes, never completed" to 229,569 files and 2.0 GB cached
  in 42 s, followed by 0% CPU.
- **`path` and `exclude_path` returned zero rows on Linux and macOS.** The
  descendant LIKE pattern hardcoded a backslash, while `path_key` is
  slash-separated off Windows, so the pattern demanded a character that never
  exists. The separator now comes from `platform::SEP` through `escape_like`,
  which leaves the Windows pattern byte-identical (#13).
- **A root path scope matched almost nothing.** After the fix above, `path: "/"`
  built a `//%` pattern, and the in-memory boundary check required a separator
  after the scope, so it returned 14 rows instead of 1,399,741. Fixed in the SQL
  builder, the entry filter, and `path_excluded` (#13 follow-up).
- **The Linux installer did not restart a running service.** `systemctl enable
  --now` starts a stopped unit and leaves an active one alone, so an upgrade
  deployed new binaries that only took effect at the next boot. It now runs
  `systemctl enable` then `systemctl restart`.
- **Documented how to invoke `scripts/install-linux.sh`.** The file is 100644 in
  git, so `./scripts/install-linux.sh` fails with `Permission denied`; the header
  now says `bash`, and notes the `sudo env "PATH=$PATH"` form needed when the
  toolchain lives in `~/.cargo/bin`.

### Changed

- The content cache is documented as budget limited rather than near-useless. On
  the Linux host the 2 GB cap holds 229,569 text files of 3.9 million, and the
  fill now completes.
- `docs/linux-support.md` records the content-fill diagnosis that it previously
  listed as an open follow-up.

## [1.0.22] - 2026-11-07

- The Linux initial scan streams instead of collecting, cutting peak RSS, and the
  fanotify self-write loop is skipped so the indexer no longer reacts to its own
  database writes.
- Startup scan progress is published to the journal, to `sd_notify`, and to
  `/run/instant-file-search/scan.json`, so `search_status` can report where a cold
  start is.
- The systemd unit sets a start timeout long enough for a cold `ntfs-3g` walk.

## Earlier

Versions 1.0.21 and earlier predate this file. Their notes are on the
[releases page](https://github.com/clayleopardlabs/instant-file-search-MCP-server/releases).
