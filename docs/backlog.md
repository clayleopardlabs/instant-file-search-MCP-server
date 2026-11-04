# Backlog

Open defects and known gaps, with the evidence for each so nobody has to
rediscover it. Ordered by severity.

Read `AGENTS.md` first for the diagnostic ladder. Several entries here began life
as a misdiagnosis.

---

## Open defects

### 1. `is:folder` binds to one side of a pipe only (medium)

`*save* is:folder | *config*` leaves the second branch unfiltered, because the
pipe splits the query into separate groups and the type token sits in only one of
them. A `PIPE_FILTER_SCOPE` warning is raised with a suggested fix, so it is
signposted, but an agent that ignores warnings reports the wrong count. One agent
nearly quoted 1,415 folders instead of 97.

Fix direction: hoist type tokens (`is:folder`, `is:file`, `file:`, `folder:`) out
of the branch tree and apply them as a global gate, so the filter cannot be
one-sided. This is a semantic change to the tokenizer, so the existing pipe tests
need reading first.

### 2. Content search covers about 10,000 of 3.9 million files (medium)

The content index is budget limited (2 GB configured, roughly 26 MB used), so
`content:"..."` queries are near-useless for "find the save file containing X".
This is a configuration decision rather than a parser bug, but agents will keep
trying to use it.

### 3. `top: 0` means default 20, not "no cap" (low)

Undocumented, and the opposite of what a caller expects from a zero.

### 4. `PATH_IN_QUERY` documentation is absolute where the behaviour is conditional (low)

The warning and the docs say a `path:` inside the query returns zero results. It
actually fails only when the path contains spaces, and otherwise raises the
warning while returning results. One agent spent a call proving the doc wrong.

### 5. No way to total an arbitrary list of paths (low)

Agents triaging several known folders drop to PowerShell because there is no
"sum these N paths" operation. It is the most cited reason for leaving the MCP
surface.

### 6. No `min_size` first-class argument (low)

`size:>5mb` works and is documented, but several agents reached for a `min_size`
argument first. Consider an alias.

---

## Recently closed, for context

- `aggregate_files` had no `offset`, so `top` capped a listing with no way to see
  the rest and folder enumerations needed subtraction tricks. `offset` now pages
  `largest`, in both the in-memory and streaming paths, covered by
  `aggregate_pages_through_largest_with_offset`. `by_extension` is still capped
  by `top` and is not paged; if that becomes a nuisance, page it the same way.
- Unknown tool arguments were silently accepted, so a caller typo returned a
  confident answer to a different question. All five parameter structs now use
  `deny_unknown_fields`, and the error names the bad field and lists accepted
  ones (`36110fb`). **Confirm after the swap** that the DeepSeek Harness client
  sends only declared fields; if it injects its own, the first call errors loudly
  rather than silently, which is the intended trade.
- `total_file_size` double counted nested folders under `is:folder`: an 11.17 GB
  tree reported 25.99 GB (`ea06092`).
- `recent_changes` accepted and ignored `path` (`9b43a8e`). Now scoped with a
  separator check, covered by
  `recent_changes_scoped_filters_to_the_requested_subtree`, which also asserts
  `C:\vPinball2` does not match a scope of `C:\vPinball`. Cannot be verified
  against a live indexer until 1.0.20 is deployed, because the old indexer has no
  `path` field and drops it.
- `fields` was declared, documented, and discarded (`9b43a8e`). Now projects rows.
- `by_extension` was unbounded, producing a 184 KB response that the harness
  spilled to disk and displayed as truncated (`9f8f03a`). Capped, sorted by size,
  with `by_extension_omitted`.
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
