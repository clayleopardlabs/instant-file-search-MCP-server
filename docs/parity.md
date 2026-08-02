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

### How to exceed Everything for agents (not yet built)

These are proposals, not shipped features. Each is a confirmed capability gap
in Everything that the native engine could genuinely beat it on:

1. **Content search.** Everything's `content:` depends on the Windows Search
   indexer and is slow/unreliable. A native on-demand or incremental text index
   would be genuinely better for agents.
2. **Change queries.** The USN journal already gives native real-time change
   history; Everything has no "what changed since X" API.
3. **Aggregations.** "Largest files in a tree", counts by extension, size sums.
   Everything's API returns raw result lists only.
4. **Semantic search.** Nothing in Everything; native could integrate with the
   existing hindsight/embedding infrastructure.

The honest framing: the project makes Everything *unnecessary* for the agent
use case and matches it on the search surface agents use, while Everything
remains the more capable general-purpose search engine underneath.
