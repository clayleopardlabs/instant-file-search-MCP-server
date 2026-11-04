# Tools

5 tools exposed over MCP:

| Tool | Purpose | Key detail |
|------|---------|------------|
| `find_files` | Full search with wildcards, regex, filters, path scope, sort, pagination, field selection | Max 100 results per call; use `offset` for paging |
| `count_files` | Fast count — returns total only, no file data | Call this FIRST for broad patterns to gauge result size |
| `search_status` | Check engine health (native indexer + Everything fallback) | Call when tools fail unexpectedly |
| `recent_changes` | List files changed recently (USN change journal), newest first | Pass `hours=N` for a time window; `reasons=` to filter; `limit` caps (default 100) |
| `aggregate_files` | Roll-up stats: total count/size, per-extension breakdown, largest files | USE THIS for disk-usage questions instead of summing results manually. Prefer `query="file:"`; a bare `*` also matches folders and a folder's size is its whole subtree, so `total_size` double-counts. `total_file_size` is always files-only |

## Key behaviors

- **Default auto-exclusions** (skip unless `include_all=true`): `node_modules`, `.git`, `WinSxS`, `$Recycle.Bin`, `System Volume Information`
- **`exclude_path` separator**: `;` (semicolon), not comma
- **Default scope**: ALL indexed drives. Narrow with `path` (forward slashes like `C:/Users` work; the engine normalizes them)
- **`find_files` and `count_files` return files, not directories.** Directories are indexed and carry the aggregate size of everything under them, so an unqualified `size:>1gb` used to rank `C:` above every real file. The native engine applies this as a `files_only` option rather than by rewriting the query: appending a `file:` token to the query string silently broke `regex:` and `regex: true` (8252786, 68a1c23). Add `is:folder`, `folder:`, or an `attrib:` filter (`attrib:d`) to include them — `aggregate_files` still reports files and folders separately.
- Every result carries `is_dir`, so a caller can filter either way without re-querying.
- `aggregate_files` returns **two** size totals. `total_size` sums every matched
  entry, and because a folder's size is its entire subtree it double-counts
  whenever the query also matches folders (`query="*"` reported ~4x the real
  figure on a real profile). `total_file_size` sums matched files only and is the
  number to use for disk usage. Query `file:` and the two agree.
- Files-by-default for `find_files`/`count_files` is applied as a `files_only`
  option on the native engine, **not** by rewriting the query string. Appending
  `file:` to the query silently broke `regex:` and `regex: true` (see 8252786,
  68a1c23); the Everything fallback still appends the token because that is how
  its engine expresses a type filter.
- Response includes `total` (all matches), `returned` (current page count), `offset` (page position), and `note` (exclusion info)

## find_files result fields

`filename`, `path`, `is_dir`, `size`, `date_modified`, `date_created`, `date_accessed`, `attributes`, `extension`, `run_count`, `date_run`. The last two are Everything-only and are **omitted** (not `null`) on the native engine, which does not track launch counts.

## recent_changes

USN Change Journal events since a time, **newest first**:

- `hours=1` — last hour (server computes the FILETIME; the agent never needs 18-digit integers)
- `since=<FILETIME>` — events strictly newer than this Windows FILETIME (100ns since 1601-01-01, NOT .NET ticks)
- `limit` — max events returned; **default 100**, pass `0` for all (capped at 100k). The ring buffer holds the last 100,000 events.
- `reasons=created,modified,renamed,deleted` — comma-separated filter; use `reasons=created,modified` to skip delete noise from the NTFS `$Deleted` staging area

Each event carries `timestamp` (the raw 100ns-since-1601 value, for round-tripping into `since`) **and** `date`, its ISO 8601 UTC equivalent, so nothing has to convert by hand.

## Sort options (14)

`name`, `name_desc`, `path`, `path_desc`, `size`, `size_asc`, `date_modified`, `date_modified_asc`, `date_created`, `date_created_asc`, `date_accessed`, `date_accessed_asc`, `extension`, `extension_desc`

Default: `name` (NameAscending).

The `sort` parameter is a strict enum (not a free-form string): invalid values are rejected at parse time with an error, rather than silently falling back to name sort. These 14 are exactly the tokens both the native indexer and the Everything fallback engine support — Everything-only sort fields (`run_count`, `date_run`, `type_name`, `date_recently_changed`) are intentionally not exposed because the native engine cannot honor them.

## Field names (12)

`filename`, `path`, `size`, `date_modified`, `date_created`, `date_accessed`, `attributes`, `extension`, `run_count`, `date_run`, `date_recently_changed`, `file_list_filename`

Default (if omitted): all common fields (`filename` through `extension`).

## Query syntax (native engine)

Everything-compatible tokens the native engine supports. Full parity detail lives in `parity.md`.

### Name matching

| Token | Example | Effect |
|-------|---------|--------|
| bare term | `handler` | Case-insensitive substring match |
| wildcards | `*.ts`, `file[0-9].txt`, `img#.png` | `*` any run (not `\`), `**` any run incl. `\`, `?` one char, `[set]`/`[!set]` classes, `#` one digit, `\x` escape |
| anchors | `^foo`, `bar$`, `^exact$` | `^` start-of-name, `$` end-of-name; also `start-with:`/`end-with:`/`prefix:`/`suffix:` |
| `regex:` | `regex:^foo\d+` | Regex match (also available as the `regex` MCP param, whole-query). In whole-query mode a type token such as `file:` is lifted out of the pattern rather than matched as literal text |
| `case:` | `case:Foo` | Case-sensitive term |
| `wholeword:` / `ww:` | `ww:foo` | Whole-word match (also available as the `match_whole_word` MCP param, whole-query) |
| `" "` | `"exact phrase"` | Exact phrase |
| `content:` | `content:"fn main"` | Match file contents. Memory mode uses a bounded 256 MB cache. Disk mode can use a temporary SQLite cache with a configurable budget. |

### Filters

| Token | Example | Effect |
|-------|---------|--------|
| `file:` / `folder:` | `file: *.ts` / `folder: src` | Type filter |
| `ext:` | `ext:rs,py` | Extension filter (comma list) |
| `size:` | `size:>10mb` | Size filter; JEDEC units (1024-based) by default; constants `tiny`(<1kb), `small`(<1mb), `medium`(<1gb), `large`(>1gb), `huge`(>4gb), `gigantic`(>16gb), `empty`(=0) |
| `metric:` | `metric:size:>1000kb` | Switch size interpretation to decimal (1000-based) |
| `len:` | `len:>10` | Filename-length filter (same operators as `size:`) |
| `frn:` | `frn:>1000` | File-reference-number filter |
| `attrib:` | `attrib:h`, `attrib:!d` | NTFS attribute filter (`h` hidden, `s` system, `r` readonly, `d` directory, `a` archive, `t` temp, `c` compressed, `e` encrypted, `o` offline, `p` reparse, `i` not-indexed, `n` normal); supports `!attrib:d` and `attrib:!d` |
| `is:` | `is:hidden`, `is:folder` | Attribute/type shorthand: `folder`/`file`, `hidden`, `system`, `readonly`, `archive`, `temporary`, `compressed`, `encrypted`, `offline`, `reparse`, `not-content-indexed`, `normal` |
| `dm:` / `dc:` / `da:` | `dm:today`, `dc:thisweek` | Modified/created/accessed date. Relative: today, yesterday, Ndays, lastNdays, prevNdays, lastweek/lastmonth/lastyear (rolling), prevweek/prevmonth/prevyear (calendar), Nhours/minutes/secs, lastNhours/lastNminutes/lastNseconds. Calendar: month names `jan`–`dec` (current year), day names `sun`–`sat` (current week), `mtd`/`ytd`/`qtd`. Absolute: `dm:2026-07-01` (local midnight) |
| `path:` | `path:"C:\Program Files"` | Full-path match modifier (folder scope uses the `path` param instead) |

### Logic

| Token | Example | Effect |
|-------|---------|--------|
| `!` | `!*.tmp` | Exclude a pattern |
| `\|` | `*.ts \| *.tsx` | Match either pattern |
| `<>` | `foo<bar>` | Group — combined terms must all match |
| `and:` / `or:` / `not:` | `and:foo`, `or:bar`, `not:baz` | Operator aliases: `and:` = default AND, `or:` = OR with previous, `not:` = exclude |

### Invalid filters

The **engine** follows Everything here: an invalid filter (e.g. `size:garbage`, `attrib:q`, `ext:`) matches **nothing**, not everything, so a query made only of invalid tokens returns zero results.

The **MCP layer rejects them instead.** `find_files`, `count_files` and `aggregate_files` return an error naming the offending token, because a silent empty result is indistinguishable from "the file is not there" — the failure an agent is least able to notice. Call the indexer directly if you need engine-level parity.

## Response diagnostics

Every `find_files`, `count_files` and `aggregate_files` response carries two extra fields so a caller can see how its query was understood without reading this file.

`parsed` — what the server did with the request:

```json
"parsed": {
  "query": "*.bin | *.7z",
  "files_only": true,
  "include_all": false,
  "excludes": ["node_modules", ".git", "WinSxS", "$Recycle.Bin", "System Volume Information"]
}
```

`warnings` — present only when something is likely wrong. Each carries a `code`, a `message`, and usually a `fix` that can be pasted back as-is:

| code | meaning |
|------|---------|
| `SUSPECT_OR` | `OR` was parsed as a literal filename term; use the pipe |
| `PATH_IN_QUERY` | `path:` inside the query instead of the `path` parameter |
| `PIPE_FILTER_SCOPE` | a filter applies to only one side of a `\|`; the other side is unfiltered |
| `INVALID_FILTER` | a filter token that can never match — returned as an error, not as a result |
| `DEFAULT_EXCLUDES_ACTIVE` | the five default exclusions are in force; `include_all=true` for a true total |
| `FOLDERS_INCLUDED` | a bare `*` in `aggregate_files` also matched folders, inflating `total_size` |

Correctly written queries produce no warnings, so a clean response means the numbers are what they appear to be.

`search_status` additionally emits `BUILD_MISMATCH` / `COMMIT_MISMATCH` when the MCP server and the running indexer come from different builds — the usual reason a fixed bug still misbehaves on the wire.
