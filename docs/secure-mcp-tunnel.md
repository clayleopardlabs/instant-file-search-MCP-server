# Secure MCP Tunnel

ChatGPT does not connect directly to a local MCP server. To expose this server to ChatGPT or other supported OpenAI surfaces without opening a public inbound port, use OpenAI's Secure MCP Tunnel client.

This repository keeps the existing stdio MCP server unchanged and adds a helper script that starts `tunnel-client` with a local-stdio profile.

## Requirements

- `Everything` must already be installed and running.
- The MCP server binary must exist at `target/release/instantaneous-windows-file-search-mcp-server.exe`, or you must pass a custom path.
- `tunnel-client` must be installed and on `PATH`.
- You need a valid tunnel ID and runtime API key from your OpenAI workspace.

## Quick start

```powershell
$env:CONTROL_PLANE_API_KEY = "your-runtime-api-key"
.\scripts\run-secure-mcp-tunnel.ps1 `
  -TunnelId tunnel_0123456789abcdef0123456789abcdef `
  -ControlPlaneApiKey $env:CONTROL_PLANE_API_KEY
```

Optional:

- Pass `-McpCommand` if your binary lives somewhere else.
- Pass `-ProfileName` if you want more than one saved tunnel-client profile.

## What the script does

1. Verifies `tunnel-client` is installed.
2. Verifies the local MCP server binary exists.
3. Initializes a `sample_mcp_stdio_local` tunnel-client profile.
4. Runs `tunnel-client doctor --explain` to confirm the tunnel is ready.
5. Starts the foreground daemon with `tunnel-client run`.

## What this does not change

- It does not alter the OpenCode main-session setup.
- It does not add an inbound public port to the server.
- It does not replace the existing stdio transport used by OpenCode and the plugin adapter.

## When to use it

Use Secure MCP Tunnel when you want a private local server to be reachable from ChatGPT or another supported OpenAI product without exposing the machine directly to the internet.

## GitHub PAT pushes

If you want this clone to push to GitHub using a PAT instead of Windows schannel, run:

```powershell
.\scripts\setup-git-auth.ps1
```

That configures repo-local Git settings to:

- prefer OpenSSL over schannel
- ask the helper script for GitHub credentials
- read the token from `GITHUB_TOKEN` or `GH_TOKEN`
