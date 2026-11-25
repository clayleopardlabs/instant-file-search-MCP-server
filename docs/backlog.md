# Backlog

Open defects and known gaps, with the evidence for each so nobody has to
rediscover it. Ordered by severity.

Read `AGENTS.md` first for the diagnostic ladder. Several entries here began life
as a misdiagnosis.

---

## Open defects

### 1. Wrong-query cases should be errors, not warnings (high)

Two queries today return a plausible wrong answer with only an advisory warning:
egex: written inside the query instead of passed as egex: true, and
path: inside the query when the path contains spaces. The warning is easy to
skim past, and the result is a wrong answer that the agent then reports as a
server bug. Whole test rounds have ended with agents "fixing" our server because
they misused it.

Make both errors. Keep the text short and actionable: name the mistake, show the
corrected call, stop. An agent should be able to recover from the error alone
without reading the docs or guessing.

### 2. Content search is budget limited, not exhaustive (medium)

The content index is capped at 2 GB, so `content:"..."` queries only see the
files that fit. On the Linux host that cap is 229,569 text files of 3.9 million,
reached in about 42 s of background indexing. It is a configuration decision
rather than a parser bug, but agents will keep trying to use it for "find the
save file containing X" and get a false negative when the file is outside the
cache.

Before 1.0.23 the fill never completed on a large disk-mode index, because the
candidate walk and the writes were both quadratic (see the closed entries
below), so real coverage was far lower than the budget allowed and the process
consumed a core for an hour or more.

### 3. No way to total an arbitrary list of paths (low)

Agents triaging several known folders drop to PowerShell because there is no
"sum these N paths" operation. It is the most cited reason for leaving the MCP
surface.

---

## Recently closed, for context

- No `min_size` argument: `size:>5mb` worked and was documented, but callers
  reached for the argument first. All three query tools now take `min_size` in
  bytes and apply it query-wide, covered by
  `min_size_drops_entries_below_the_threshold`.
- `top: 0` is treated as omitted and returns 20 entries, verified against the
  running indexer rather than assumed. An agent had reported that 0 suppressed the
  largest list; it does not, so the behaviour was documented instead of changed.
- A `path:` inside the query is documented as failing for paths containing spaces,
  which is exactly what it does. The backlog entry claiming the doc was absolute
  was wrong: the doc was already conditional. Only the handshake wording needed
  tightening, so no behaviour change was made.
- Type filters bound to one side of a pipe, so `*a* is:folder | *b*` returned
  files from the second branch and an agent counting folders reported 1,415
  instead of 97. Type tokens are now hoisted out of the branch tree and applied as
  a query-wide gate, covered by `type_filter_applies_to_every_pipe_branch`. This
  also makes the runtime agree with the `PIPE_FILTER_SCOPE` diagnostic, which
  already treated type tokens as global.
- `aggregate_files` had no `offset`, so `top` capped a listing with no way to see
  the rest. `offset` now pages `largest`, covered by
  `aggregate_pages_through_largest_with_offset`. `by_extension` stays capped by
  `top` and is not paged.
- Unknown tool arguments were silently accepted, so a caller typo returned a
  confident answer to a different question. All five parameter structs use
  `deny_unknown_fields` and the error names the bad field (`36110fb`). **Confirm
  after the swap** that the DeepSeek Harness client sends only declared fields.
- `total_file_size` double counted nested folders under `is:folder`: an 11.17 GB
  tree reported 25.99 GB (`ea06092`).
- `recent_changes` accepted and ignored `path` (`9b43a8e`). Now scoped with a
  separator check, covered by
  `recent_changes_scoped_filters_to_the_requested_subtree`. Cannot be verified
  against a live indexer until 1.0.20 is deployed.
- `fields` was declared, documented, and discarded (`9b43a8e`). Now projects rows.
- `by_extension` was unbounded, producing a 184 KB response that the harness
  spilled to disk and displayed as truncated (`9f8f03a`). Capped, sorted by size.
- No depth control, so "what is loose in this folder" was inexpressible
  (`9f8f03a`). Added `depth`.
- The scope root was returned as its own child (`9f8f03a`). Excluded.
- `aggregate_files` reported a different `total` than `count_files` for the same
  query (`25ff9eb`). Both now files-by-default.
- The disk-mode content-candidate walk re-sorted the whole `files` table for every
  512-row page, so filling the content cache was quadratic: 0.71 s per page, about
  3,639 pages, roughly 43 CPU minutes per pass, repeated at every start because
  `serve()` clears the cache. Pinned the `path` primary-key index, measured
  0.0006 s per page (#14, `b67360e`).
- Each content-cache write opened its own transaction and ran
  `SELECT SUM(byte_count) FROM file_contents`, a full scan of the content table,
  so the fill managed about 13 to 45 files per second. Writes are batched now, one
  transaction and one byte-total read per 64 files (#16, `8cd8e8e`).
- The batched budget read still scanned every blob. A covering index on
  `file_contents(byte_count)` took the read from 0.0372 s to 0.0006 s on a 195 MB
  table (#19, `b51f25a`). With all three, the Linux host builds 229,569 files /
  2.0 GB in 42 s and then idles at 0% CPU; 1.0.22 never finished.
- `path` and `exclude_path` returned zero rows on Linux and macOS because the
  descendant LIKE pattern hardcoded a `\` and `path_key` is slash-separated off
  Windows. The separator now comes from `platform::SEP` through `escape_like`,
  which leaves the Windows pattern byte-identical (#13, `aa7863c`).
- A root scope was still wrong after that fix: the pattern came out as `//%`, and
  the in-memory boundary check demanded a separator after the scope, so
  `path: "/"` returned 14 rows instead of 1,399,741. Fixed in the SQL builder, the
  entry filter and `path_excluded` (#13 follow-up, `e8af553`).
- The Linux installer deployed new binaries but left a running service alone,
  because `systemctl enable --now` does not restart an active unit, so an upgrade
  only took effect at the next boot. It now runs `enable` then `restart`
  (`8cd8e8e`).

---

## Deployment state

The Linux host runs the **1.0.23** pair (installed 2026-11-24): MCP server and
indexer both in `/usr/local/lib/instant-file-search/`, service
`instant-file-search-indexer.service`, and `search_status` reporting matching
versions 1.0.23.

That install is built from source by `scripts/install-linux.sh`, not from the
release assets, so the host is only current once that script has run since the
last merge. Confirm with `search_status`: the version comes from the binary, so a
running process older than the installed file still reports the version it was
built from.

On Windows the deployment history was **1.0.19** running with **1.0.20** staged
but not live, because swapping the indexer service needs elevation:

```
C:\Users\Omen\Documents\deepseek-harness\default-workspace\switch-indexer-to-1.0.20-as-admin.ps1
```

That elevation step (run as Administrator, then restart DeepSeek Harness) is
still how a Windows indexer swap works. `search_status` reports
`BUILD_MISMATCH` while client and indexer disagree, which is the fastest way to
confirm the swap took.

---

## How to add to this file

State the symptom, the exact reproduction, and the tool version. A claim without a
reproduction is a rumour, and this session produced several of those by testing
the wrong thing.
