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

## Sort options (22)

`name`, `name_desc`, `path`, `path_desc`, `size`, `size_asc`, `date_modified`, `date_modified_asc`, `date_created`, `date_created_asc`, `date_accessed`, `date_accessed_asc`, `extension`, `extension_desc`, `run_count`, `run_count_asc`, `date_run`, `date_run_asc`, `type_name`, `type_name_desc`, `date_recently_changed`, `date_recently_changed_asc`

Default: `name` (NameAscending).

## Field names (12)

`filename`, `path`, `size`, `date_modified`, `date_created`, `date_accessed`, `attributes`, `extension`, `run_count`, `date_run`, `date_recently_changed`, `file_list_filename`

Default (if omitted): all common fields (`filename` through `extension`).
