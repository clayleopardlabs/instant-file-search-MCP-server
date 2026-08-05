# Tools

5 tools exposed over MCP:

| Tool | Purpose | Key detail |
|------|---------|------------|
| `find_files` | Full search with wildcards, regex, filters, path scope, sort, pagination, field selection | Max 100 results per call; use `offset` for paging |
| `count_files` | Fast count — returns total only, no file data | Call this FIRST for broad patterns to gauge result size |
| `search_status` | Check engine health (native indexer + Everything fallback) | Call when tools fail unexpectedly |
| `recent_changes` | List files changed recently (USN change journal), newest first | Pass `hours=N` for a time window; `reasons=` to filter; `limit` caps (default 100) |
| `aggregate_files` | Roll-up stats: total count/size, per-extension breakdown, largest files | USE THIS for disk-usage questions instead of summing results manually |

## Key behaviors

- **Default auto-exclusions** (skip unless `include_all=true`): `node_modules`, `.git`, `WinSxS`
- **`exclude_path` separator**: `;` (semicolon), not comma
- **Default scope**: ALL indexed drives. Narrow with `path` (forward slashes like `C:/Users` work; the engine normalizes them)
- Response includes `total` (all matches), `returned` (current page count), `offset` (page position), and `note` (exclusion info)

## recent_changes

USN Change Journal events since a time, **newest first**:

- `hours=1` — last hour (server computes the FILETIME; the agent never needs 18-digit integers)
- `since=<FILETIME>` — events strictly newer than this Windows FILETIME (100ns since 1601-01-01, NOT .NET ticks)
- `limit` — max events returned; **default 100**, pass `0` for all (capped at 100k). The ring buffer holds the last 100,000 events.
- `reasons=created,modified,renamed,deleted` — comma-separated filter; use `reasons=created,modified` to skip delete noise from the NTFS `$Deleted` staging area

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
| `regex:` | `regex:^foo\d+` | Regex match (also available as the `regex` MCP param, whole-query) |
| `case:` | `case:Foo` | Case-sensitive term |
| `wholeword:` / `ww:` | `ww:foo` | Whole-word match (also available as the `match_whole_word` MCP param, whole-query) |
| `" "` | `"exact phrase"` | Exact phrase |
| `content:` | `content:"fn main"` | Match file contents (bounded 256MB store — targeted searches, not exhaustive counts) |

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

An invalid filter (e.g. `size:garbage`, `attrib:q`, `ext:`) matches **nothing**, not everything — same as Everything. A query made only of invalid tokens returns zero results.
