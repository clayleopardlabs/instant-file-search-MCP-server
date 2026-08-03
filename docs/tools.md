# Tools

3 tools exposed over MCP:

| Tool | Purpose | Key detail |
|------|---------|------------|
| `find_files` | Full search with wildcards, regex, path filter, sort, pagination, field selection | Max 100 results per call; use `offset` for paging |
| `count_files` | Fast count — returns total only, no file data | Call this FIRST for broad patterns to gauge result size |
| `search_status` | Check Everything IPC health (window, DB load, version) | Call when tools fail unexpectedly |

## Key behaviors

- **Default auto-exclusions** (skip unless `include_all=true`): `node_modules`, `.git`, `WinSxS`
- **`exclude_path` separator**: `;` (semicolon), not comma
- **Default scope**: ALL indexed drives. Narrow with `path`.
- Response includes `total` (all matches), `returned` (current page count), `offset` (page position), and `note` (exclusion info)

## Sort options (14)

`name`, `name_desc`, `path`, `path_desc`, `size`, `size_asc`, `date_modified`, `date_modified_asc`, `date_created`, `date_created_asc`, `date_accessed`, `date_accessed_asc`, `extension`, `extension_desc`

Default: `name` (NameAscending).

The `sort` parameter is a strict enum (not a free-form string): invalid values are rejected at parse time with an error, rather than silently falling back to name sort. These 14 are exactly the tokens both the native indexer and the Everything fallback engine support — Everything-only sort fields (`run_count`, `date_run`, `type_name`, `date_recently_changed`) are intentionally not exposed because the native engine cannot honor them.

## Field names (12)

`filename`, `path`, `size`, `date_modified`, `date_created`, `date_accessed`, `attributes`, `extension`, `run_count`, `date_run`, `date_recently_changed`, `file_list_filename`

Default (if omitted): all common fields (`filename` through `extension`).
