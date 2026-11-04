# Backlog

Open defects and known gaps, with the evidence for each so nobody has to
rediscover it. Ordered by severity. Anything here is unverified until the
evidence is reproduced.

Read `AGENTS.md` first for the diagnostic ladder. Several entries below began life
as a misdiagnosis.

---

## Open defects

### 1. Unknown tool arguments are silently accepted (high)

`find_files`, `count_files`, `aggregate_files` and `recent_changes` accept
arguments that do not exist in their schema, and ignore them. A caller typo
therefore produces a confident answer to a different question.

Reproduced against the server binary directly, DSH out of the path:

```
tools/call recent_changes {"hours":720,"limit":2,"zzz_bogus_argument":true}
-> success, 2 events, no error
```

This is the same failure class as two bugs fixed in 1.0.20: `fields` was declared
and discarded, and `recent_changes` accepted and ignored `path`. Both produced
plausible wrong answers rather than errors.

Fix direction: `#[serde(deny_unknown_fields)]` on the parameter structs, then
check that the resulting error surfaces as a normal tool error and not a panic.
Note this tightens the client contract, so confirm the shipped plugin and the
DeepSeek Harness client send only declared fields before releasing.

### 2. `is:folder` binds to one side of a pipe only (medium)

`*save* is:folder | *config*` leaves the second branch unfiltered. A
`PIPE_FILTER_SCOPE` warning is raised with a suggested fix, so it is signposted,
but an agent that ignores warnings reports the wrong count. One agent nearly
quoted 1,415 folders instead of 97.

### 3. `aggregate_files` has no `offset` (medium)

`top` caps both `largest` and `by_extension`, but there is no way to page past
that cap. Folder listings longer than `top` cannot be enumerated without
subtraction tricks. `find_files` has `offset`; aggregate should match.

### 4. Content search covers about 10,000 of 3.9 million files (medium)

The content index is budget limited (2 GB configured, roughly 26 MB used), so
`content:"..."` queries are near-useless for "find the save file containing X".
This is a configuration decision rather than a parser bug, but agents will keep
trying to use it.

### 5. `top: 0` means default 20, not "no cap" (low)

Undocumented. Docs say adding directories is the way to add them; `0` silently
means 20.

### 6. `PATH_IN_QUERY` documentation is absolute where the behaviour is conditional (low)

The warning and the docs say a `path:` inside the query returns zero results. It
actually fails only when the path contains spaces, and raises the warning
otherwise while returning results. One agent spent a call proving the doc wrong.

### 7. No way to total an arbitrary list of paths (low)

Agents triaging several known folders drop to PowerShell because there is no
"sum these N paths" operation. It is the most cited reason for leaving the MCP
surface.

### 8. No `min_size` first-class argument (low)

`size:>5mb` works and is documented, but every agent reached for a `min_size`
argument first. Consider an alias.

---

## Recently closed, for context

- `total_file_size` double counted nested folders under `is:folder`: an 11.17 GB
  tree reported 25.99 GB (`ea06092`). Client and harness were not involved; the
  indexer was.
- `recent_changes` accepted and ignored `path` (`9b43a8e`). Now scoped in the
  indexer with a separator check.
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
**1.0.20**, which is staged but not live because swapping the indexer service
needs elevation:

```
C:\Users\Omen\Documents\deepseek-harness\default-workspace\switch-indexer-to-1.0.20-as-admin.ps1
```

Run it as Administrator, then restart DeepSeek Harness so the client picks up the
new version. `search_status` reports `BUILD_MISMATCH` while client and indexer
disagree, which is the fastest way to confirm the swap took.

---

## How to add to this file

State the symptom, the exact reproduction, and the tool version. A claim without
a reproduction is a rumour, and this session produced several of those by testing
the wrong thing.
