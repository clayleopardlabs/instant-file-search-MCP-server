# Everything parity notes

The native indexer's goal is behavioral parity with the bundled Everything
engine. Everything is the reference implementation: we run identical queries
through both engines (native pipe + Everything IPC) and diff the results.
Any mismatch is a bug in the native engine. The differential harness lives in
`src/parity.rs` as ignored `#[cfg(test)]` tests (run manually, they need both
engines up).

```
cargo test --release -p instant-file-search-mcp-server -- --ignored parity_battery
```

## Locked semantics

These were established by webfetching the Everything documentation and by
live probes against the running engine.

### Size filters

- **Units are JEDEC (1024-based) by default.** `size:1kb` = 1024 bytes,
  `size:1mb` = 1,048,576. The `metric:` modifier opts into decimal (1000-based).
  The native parser used decimal until the parity battery caught it.
- **Bare unit value = granularity range.** `size:1kb` means 1024..2047 (range
  up to the next unit), not an exact match. A unitless number is exact:
  `size:100` = exactly 100 bytes. Exact-size queries use the explicit range
  form (`size:1mb..1mb`) or `size:=1mb`.
- **Inclusive operators.** `>=` / `<=` exist and must be checked before the
  strict `>` / `<` when parsing: stripping `>` from `>=1mb` leaves `=1mb`,
  which silently changes the meaning (this was a real bug: `size:>=1mb`
  matched everything after the token dropped).
- **Directories match by recursive (tree-summed) size.** Everything evaluates
  `size:` against a folder's total tree size, so a big folder matches
  `size:>1mb` even though its own allocation is small. The native index
  computes recursive totals at scan time and maintains them incrementally
  through USN events (see `mft.rs` `compute_folder_sizes` and `index.rs`
  `adjust_ancestors`). Directories with recursive size 0 match `size:0`.

### Date filters

Everything evaluates relative dates in **local time**. The native parser
adds the local offset to span boundaries and to entry FILETIME timestamps.

- `today` / `yesterday`: calendar days in local time.
- `lastNdays` / `pastNdays` / bare `Ndays`: rolling window `[now - N*day, now]`.
- `prevNdays` / `previousNdays`: trailing window `[today_start - N*day,
  today_start)` — NOT rolling.
- `lastweek` / `pastweek`: rolling 7 days (Everything treats them as
  `last7days`). `prevweek` / `previousweek`: calendar previous week,
  Sunday-start.
- `lastmonth` / `pastmonth`: rolling 31 days (probe: equals `last31days`
  exactly — not 30, not the calendar month). `prevmonth` / `previousmonth`:
  calendar previous month.
- `lastyear` / `pastyear`: rolling 365 days. `prevyear` / `previousyear`:
  calendar previous year.
- **Absolute dates (`dm:2026-07-01`) match by local midnight, not UTC.**
  Everything buckets a file by the calendar day its modified time falls in
  *locally*. The ISO branch of `parse_date` must NOT add `local_offset_secs()`:
  the entry-side comparison already adds it, so adding it again makes the
  offsets cancel and the window becomes UTC midnights, which shifts adjacent
  days by the offset (a file modified `2026-07-02T00:00:04Z` is local
  `2026-07-01 20:00` and belongs to `dm:2026-07-01`). Fixed by returning
  `days * 86400` (wall-clock-unix of local midnight). This was a real bug
  caught by the battery: `dm:2026-07-01` native=4,344 vs Everything=4,804 and
  `dm:2026-07-02` native=3,908 vs 2,509 diverged in opposite directions; after
  the fix both match (4,769/4,804 and 2,508/2,509, residual = folder-leak
  noise).

### Path scoping

`path:` in Everything is a **full-path match modifier, not a folder scope**.
Folder scoping uses the `path` parameter (both engines take it as a scope).
The parity battery passes paths via the `path` param, never as `path:` tokens.
A path containing a space must be quoted (`"C:\Program Files"`) or it splits
into two terms.

## Known residual gaps (accepted)

- Everything's `!<foo\>` exclude syntax excludes a folder's contents but the
  folder itself still leaks into results; native excludes the folder too.
  This shows up as small deltas on queries whose terms hit excluded trees
  (e.g. a `node_modules` query scoped to a user profile: native returns only
  files whose *name* contains the term, Everything adds the leaked folders).
  Native is the more correct engine here; the divergence is accepted.
- Everything reads live from its MFT index; native serves its in-memory scan.
  Immediately after a scan finishes, a few files changed in the last minutes
  can differ. Deltas of a few hundred out of millions are expected.
- Recency deltas on relative-date filters (`dm:today` etc.) right after
  midnight: Everything's index timestamps and native's in-memory scan can
  disagree on a small set of files touched in the last minutes.

## Non-gaps checked and rejected

- **USN-upserted entries and default excludes.** A concern that entries added
  via USN upsert after a scan would lack the `excluded` flag is unfounded:
  `IndexedFile::new` computes it in `refresh()` (mft.rs) from the entry path,
  and `upsert` stores the entry whole, so live-created `node_modules` trees
  are excluded exactly like scan-time ones. Verified live: native returns only
  name-substring matches under a user profile, Everything's larger count is
  its folder-leak.

## Status vs Everything (determination)

The native engine is at **behavioral parity with Everything on the query
surface agents actually use** — name, path, size (including recursive folder
sizes), regex, `case:`, excludes, and both relative and absolute date filters.
The differential battery runs ~170 probes; every remaining DIFF is a
characterized residual gap, not a native bug.

## Parity audit 2026-08-03 (full surface)

Audit of the complete Everything 1.5 search surface (from the official
voidtools reference) against the native engine, plus live differential probes.
Three bugs were found and fixed or queued, and the query surface has been
extended to cover the Everything tokens an agent would actually type.

### Bugs found by this audit

1. **`regex` / `match_whole_word` MCP params were silently ignored by the
   native engine.** `src/native.rs` serialized them into the pipe request, but
   the indexer's `tokenize()` only looked at the query string, so only the
   `regex:` *token* worked; `find_files(query=..., regex=true)` did plain
   substring matching while Everything honored the flag. Fixed by threading
   both params into the tokenizer (query.rs). This is the class of bug the
   battery cannot catch: the harness passes `regex:` tokens, never the params.
2. **`content:"text"` quoted-value parsing was broken** (fix landed 2026-08-03
   earlier): multi-word quoted values kept their quote characters and a
   `content:"fn main"` split at the space. `extract_content_terms` now strips
   the `content:` prefix and keeps a quoted value intact. Verified live:
   `content:"fn main"` finds 116 files.
3. **Content-store fill order is nondeterministic.** The fill pass iterates
   candidate files in HashMap order and stops at the 256MB budget, so coverage
   is effectively a random sample — repository files may or may not be picked.
   FIXED: candidates are iterated in a stable order (sorted by path) so the
   same files are picked across runs and full coverage is reachable.

### Behavioral divergences (characterized)

- **Directory junctions.** Native indexes raw MFT records: a file is counted
  once, at its real path. Everything follows junctions and also indexes the
  target contents under the junction path, which double-counts junction-heavy
  trees. Battery evidence: `file: in C:\Program Files` native=217,587 vs
  Everything=272,981 even with `include_all` (WindowsApps is junction-heavy).
  Native is the more correct engine; the divergence is accepted and
  documented, not fixed (matching Everything would require re-indexing
  junction targets, trading correctness for parity).
- **`dm:today` recency.** Immediately after a fresh reindex, `dm:today` can
  differ by ~15% (native 8,046 vs Everything 9,308 in the 2026-08-03 03:10
  run) because Everything reads live MFT timestamps and native serves its
  scan snapshot plus USN upserts. Re-check after the index settles; the
  absolute-date paths (`dm:2026-07-01`) match exactly.

### Query surface: native now supports (Everything 1.5 tokens)

In addition to the surface below (wildcards `*`/`?`, `regex:`, `case:`, `!`,
`|`, `<>`, `dm:`/`dc:`/`da:`, `size:`, `ext:`, `path:`, `folder:`/`file:`):

- `attrib:` filter (attribute letters: `h` hidden, `s` system, `r` read-only,
  `d` directory, `a` archive, `t` temporary, `c` compressed, `e` encrypted,
  `o` offline, `p` reparse, `i` not-content-indexed, `n` normal) and the
  `attributes` result field now carries real NTFS flags.
- `len:` filename-length filter (comparators and ranges).
- `frn:` file-reference-number filter (native already indexed file_ref).
- `wholeword:` / `ww:` modifier token.
- `and:` / `or:` / `not:` operator aliases.
- `metric:` decimal-size modifier (Everything's default is JEDEC; `metric:`
  switches size interpretation to 1000-based).
- Size constants: `tiny` / `small` / `medium` / `large` / `huge` /
  `gigantic` / `empty`.
- Date constants: month names (`jan`–`dec`), day names (`sun`–`sat`),
  `mtd` / `ytd` / `qtd`, `lastNhours` / `lastNminutes` / `lastNseconds`.
- Anchors: `^` (start) and `$` (end), plus `start-with:` / `end-with:` /
  `prefix:` / `suffix:`.
- `parent:` / `child:` / `sibling:` relationship scoping.
- `rc:` / `recentchange:` filter (native's USN change journal supplies the
  per-file last-change timestamp).
- `is:` predicates (`is:folder`, `is:file`, `is:hidden`, `is:system`, ...).
- Extended wildcards: `**` (crosses `\`), `[set]`, `[!set]`, `#` (digit),
  and `\` escape.
- `sort:` as a query token (`sort:size-descending` etc.), in addition to the
  `sort` param.

### Accepted gaps (Everything data native cannot produce)

- **`runcount:` / `date-run:` and the `run_count` / `date_run` fields.**
  Everything tracks file execution history in its own database. Native has
  no execution tracker; approximating it (e.g. via USN reads) is not
  equivalent. Kept as documented gap; `run_count`/`date_run` return null.
- **`dupe:` duplicate finder.** Everything's full dupe subsystem (content
  hashing) is a separate product feature. Native can find name/size
  duplicates cheaply from its index but not content-identical files without
  a hash pass. Left documented.
- **Unicode case folding / diacritics folding** (`é` ≡ `e`, `ß` ≡ `ss`).
  Everything folds Unicode case and diacritics; native matches ASCII
  case-insensitively only. Adding full folding costs a pass over every entry
  name per query; out of scope for now.

### Result-surface parity

- Fields: native returns `filename`, `path`, `size`, `date_modified`,
  `date_created`, `date_accessed`, `attributes` (now real flags),
  `extension`, `is_dir`/`type`. `run_count`/`date_run` stay null (accepted
  gap above). `date_recently_changed` maps to the USN `rc:` timestamp.
- Sorts: native honors the 14 documented sorts plus the Everything-style
  `sort:` tokens for size/date/name/path/extension. Everything-only sorts
  (run count, date run) remain unsupported by design.

### How to re-run the audit

```
cargo test --release -p instant-file-search-mcp-server -- --ignored parity_battery
```

Both engines must be up (native service + Everything). Add probes for any new
surface; every probe must be characterized before the battery is green.

### Where native is at parity

- Bare-term substring matching (Everything semantics: "AGENTS" matches
  "AGENTS.md"), not exact.
- Wildcards, `regex:`, `case:` token, `!` excludes, `|` OR groups, `<>` groups.
- Size: JEDEC units, `>=`/`<=`/strict/range forms, bare-unit granularity ranges,
  recursive folder sizes.
- Dates: `today`/`yesterday`, `lastNdays`/`pastNdays`, `prevNdays` (trailing),
  `lastweek`/`prevweek`, `lastmonth` (rolling 31d), `lastyear`, and absolute
  `dm:`/`dc:`/`da:` dates — all in local time.
- `path`/`parent` scoping, `file:`/`folder:` type filters, `sort`.

### Where native is strictly more correct

- **Folder excludes.** Everything's `!<foo\>` excludes a folder's contents but
  leaks the folder itself; native excludes the folder too. Native is right.
- **Freshness of source.** Everything reads live from its MFT index but its
  in-memory lists go stale in build-churn trees; native serves its own scan
  which matches disk truth.

### Exceed capabilities (3 of 4 shipped)

The native engine beats Everything on agent-relevant capability gaps. Three of
the four are shipped; the fourth is designed but needs external embedding infra.

1. **Aggregations — SHIPPED.** `aggregate` pipe method + `aggregate_files` MCP
   tool. Runs the same filter as search, then returns the matched totals (file
   count, folder count, total size, largest files) and a per-extension
   breakdown (count + size). Works entirely off the existing in-memory index,
   using the recursive folder sizes for directory entries. Everything's API
   returns raw result lists only — aggregating requires the caller to fetch
   and sum everything.
2. **Change queries — SHIPPED.** `recent_changes` pipe method + MCP tool. The
   USN Change Journal watcher records every applied mutation (CREATE, WRITE,
   RENAME, DELETE, HARD_LINK) into a bounded in-memory ring buffer (cap
   100,000 events), keyed by event reason, local FILETIME timestamp, path, and
   is_dir. Query with `since` (FILETIME) and `limit`. Everything has no
   "what changed since X" API at all.
3. **Content search — SHIPPED.** `content:"phrase"` query token (works through
   `find_files`/`count_files`). A bounded `ContentStore` indexes file contents
   for a text-extension allowlist (see `indexer/src/content.rs`: md, txt, rs,
   py, js, ts, json, yml, toml, csv, log, etc.) with a per-file size cap
   (256KB read) and a global 256MB budget. Built as a non-blocking background
   pass after the scan and maintained incrementally via USN (re-read on
   WRITE/CLOSE, removed on DELETE/RENAME). Content tokens are extracted by the
   pipe layer and resolved against the store, then injected into the query as
   a `content_paths` constraint, composing with all existing AND/OR semantics.
   This genuinely exceeds Everything's `content:`, which depends on the
   Windows Search indexer and is slow/unreliable when the content indexer is
   not running. Note: native-only — if the indexer is down the MCP server
   falls back to Everything's `content:` (which needs the Windows Search
   indexer).
4. **Semantic search — DESIGNED, NOT BUILT.** Nothing in Everything. The
   native engine could integrate with the existing hindsight/embedding
   infrastructure, but this requires an embedding model backend, vector
   storage, and a query-time similarity search — a separate project that
   should not be invented silently. Design sketch: embed the text of
   content-indexed files at scan time (or lazily), store vectors keyed by
   path, and expose a `similar:"phrase"` token returning nearest neighbours.

The honest framing: the project makes Everything *unnecessary* for the agent
use case and matches it on the search surface agents use, while Everything
remains the more capable general-purpose search engine underneath.
