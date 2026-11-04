# Backlog

Open defects and known gaps, with the evidence for each so nobody has to
rediscover it. Ordered by severity.

Read `AGENTS.md` first for the diagnostic ladder. Several entries here began life
as a misdiagnosis.

---

## Open defects

### 1. Content search covers about 10,000 of 3.9 million files (medium)

The content index is budget limited (2 GB configured, roughly 26 MB used), so
`content:"..."` queries are near-useless for "find the save file containing X".
A configuration decision rather than a parser bug, but agents will keep trying to
use it.

### 2. No way to total an arbitrary list of paths (low)

Agents triaging several known folders drop to PowerShell because there is no
"sum these N paths" operation. It is the most cited reason for leaving the MCP
surface.

### 3. No `min_size` first-class argument (low)

`size:>5mb` works and is documented, but several agents reached for a `min_size`
argument first. Consider an alias.

---

## Recently closed, for context

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

---

## Deployment state

The running pair is **1.0.19**. Everything above that says "fixed" is built into
**1.0.20**, staged but not live, because swapping the indexer service needs
elevation:

```
C:\Users\Omen\Documents\deepseek-harness\default-workspace\switch-indexer-to-1.0.20-as-admin.ps1
```

Run it as Administrator, then restart DeepSeek Harness. `search_status` reports
`BUILD_MISMATCH` while client and indexer disagree, which is the fastest way to
confirm the swap took.

---

## How to add to this file

State the symptom, the exact reproduction, and the tool version. A claim without a
reproduction is a rumour, and this session produced several of those by testing
the wrong thing.
