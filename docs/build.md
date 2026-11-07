# Build

## Prerequisite

**A search engine is not required at build time.** The repo vendors the optional portable Everything engine (`vendor/everything/`). It is not compiled in and not deployed by default; the installer adds it only when you pass `-IncludeFallbackEngine` (or reuses an Everything already installed). Windows only.

## Binary

The project pins `x86_64-pc-windows-gnu` as the default Rust target (see `rust-toolchain.toml`). This avoids requiring the full Visual Studio Build Tools with C++ workload, only [MSYS2/MinGW-w64](https://www.msys2.org/) is needed for the GNU linker.

```sh
cargo build --release
```

Output: `target/release/instant-file-search-mcp-server.exe` and `target/release/instant-file-search-indexer.exe` (the workspace has two binaries). On Linux the same names build without the `.exe` suffix.

To build with MSVC instead, override the toolchain target:

```sh
cargo build --release --target x86_64-pc-windows-msvc
```

## Vendoring the portable engine

The bundled engine lives at `vendor/everything/instant-file-search-fallback-engine-1.5.0.1418b.zip` (SHA256-pinned, MIT-licensed, see `vendor/everything/LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt`). To re-fetch or update it:

```powershell
.\scripts\fetch-everything.ps1
```

When the fallback is opted into, the installer (`scripts\install.ps1 -IncludeFallbackEngine`) extracts this zip into the versioned install root's `versions\<version>\everything\` folder alongside `instant-file-search-fallback-engine-1.5.0.1418b.ini`. Without that switch nothing is extracted.

## Plugin (optional: OpenCode sub-agent support)

```sh
cd plugin
npm install
npm run build
```

Output: `plugin/dist/index.js`

## Plugin binary resolution

The plugin adapter finds the MCP binary by (in order):
1. `INSTANT_FS_MCP_BINARY` environment variable (`EVERYTHING_MCP_BINARY` accepted for backward compatibility)
2. Stable install: `%LOCALAPPDATA%\ClayLeopardLabs\instant-file-search\instant-file-search-mcp-server.exe`
3. Default path relative to the plugin's `dist/` directory: `../../target/release/instant-file-search-mcp-server.exe`
4. Falls back to `PATH`: fails if not found

If the binary path is wrong, all plugin tool calls fail silently. Set `INSTANT_FS_MCP_BINARY` to point at a release build.
