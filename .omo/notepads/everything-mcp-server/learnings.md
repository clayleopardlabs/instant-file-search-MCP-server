# everything-mcp-server Learnings

## everything-ipc v0.1.4 API Summary

- **Module**: everything_ipc::wm — window message IPC (WM_COPYDATA), requires Everything GUI open
- **Client**: EverythingClient::new() -> Result<Self, IpcError> — creates IPC client (also checks window exists)
- **Query**: client.query_wait(query_str) → builder → .request_flags(RequestFlags) (REQUIRED) → .sort(Sort) → .max_results(u32) → .search_flags(SearchFlags) → .call() -> Result<QueryList, IpcError>
- **Results**: QueryList.iter() → QueryItem, QueryList.total_len() → usize
- **QueryItem**: .get_string(RequestFlags) -> Option<String>, .get_size(RequestFlags) -> Option<u64>, .get_time(RequestFlags) -> Option<FILETIME>, .get_u32(RequestFlags) -> Option<u32>
- **Thread safety**: EverythingClient is Send + Sync — OK to create inside spawn_blocking

### Sort enum variants
NameAscending=1, NameDescending=2, PathAscending=3, PathDescending=4, SizeAscending=5, SizeDescending=6, ExtensionAscending=7, ExtensionDescending=8, TypeNameAscending=9, TypeNameDescending=10, DateCreatedAscending/Descending=11/12, DateModifiedAscending/Descending=13/14, AttributesAscending/Descending=15/16, DateAccessedAscending/Descending=23/24

### RequestFlags bitflags
FileName, Path, FullPathAndFileName, Extension, Size, DateCreated, DateModified, DateAccessed, Attributes, FileListFileName, RunCount, DateRun, DateRecentlyChanged, HighlightedFileName, HighlightedPath, HighlightedFullPathAndFileName

### SearchFlags bitflags
MatchCase, MatchWholeWord, MatchPath, Regex, MatchAccents

### Key constraint
equest_flags is REQUIRED on the builder (must call before .call()). EverythingClient cannot be shared across threads for concurrent queries — query_wait handles serialization.

### IpcError variants
NoIpcWindow, CreateReplyWindow, Send, Timeout, Query(&'static str)

### FILETIME conversion
FILETIME is windows crate struct with dwLowDateTime/dwHighDateTime (u32). Represents 100-ns intervals since 1601-01-01 UTC.
Unix epoch offset: 11644473600 seconds = 116444736000000000 100-ns intervals.
- Field access (`ft.dwHighDateTime`) works without naming the type — Rust allows accessing public fields of a concrete type returned from a function.
- To name FILETIME in type signatures, add `windows = { version = "0.62", features = ["Win32_Foundation"] }` as a direct dependency.

### bon builder typestate behavior (everything-ipc v0.1.4)
- `query_wait` uses `#[builder]` from `bon`.
- Only `request_flags` is REQUIRED (no default) — tracked by typestate.
- `search_flags`, `sort`, `offset`, `max_results` all have `#[builder(default)]` or are `Option` — these are optional setters that return the SAME builder type.
- **`mut builder = builder.search_flags(...)` works fine** because optional setters return `Self` (same type). No special workaround needed.
- Use `.maybe_max_results(params.max_results)` for `Option<u32>` fields instead of conditional `.max_results()` calls.
- Single-chain style (compute values first, then chain) is cleaner than `mut builder` reassignment but both work.

### windows crate + GNU target requirement
- Rust toolchain: `stable-x86_64-pc-windows-gnu` — requires GNU binutils for linking.
- `dlltool.exe` from MSYS2 MinGW is needed: found at `C:\msys64\mingw64\bin\dlltool.exe`.
- **Fix**: Add `C:\msys64\mingw64\bin` to PATH before running cargo.
- Alternative: switch to `stable-x86_64-pc-windows-msvc` target (avoids this issue).
