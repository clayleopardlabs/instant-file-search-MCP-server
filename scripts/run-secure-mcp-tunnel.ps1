param(
  [Parameter(Mandatory = $true)]
  [string]$TunnelId,

  [Parameter(Mandatory = $true)]
  [string]$ControlPlaneApiKey,

  [string]$ProfileName = "instantaneous-windows-file-search",

  [string]$TunnelClient = "tunnel-client",

  [string]$McpCommand = (Join-Path $PSScriptRoot "..\target\release\instantaneous-windows-file-search-mcp-server.exe")
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command $TunnelClient -ErrorAction SilentlyContinue)) {
  throw "tunnel-client was not found on PATH. Install it first, then rerun this script."
}

if (-not (Test-Path -LiteralPath $McpCommand)) {
  throw "MCP server binary not found at '$McpCommand'. Build the server first or pass -McpCommand."
}

# The tunnel-client uses these values for startup and daemon auth.
$env:CONTROL_PLANE_TUNNEL_ID = $TunnelId
$env:CONTROL_PLANE_API_KEY = $ControlPlaneApiKey

Write-Host ":: Initializing tunnel-client profile..." -ForegroundColor Cyan
& $TunnelClient init `
  --sample sample_mcp_stdio_local `
  --profile $ProfileName `
  --tunnel-id $TunnelId `
  --mcp-command $McpCommand

if ($LASTEXITCODE -ne 0) {
  throw "tunnel-client init failed."
}

Write-Host ":: Verifying tunnel-client profile..." -ForegroundColor Cyan
& $TunnelClient doctor --profile $ProfileName --explain

if ($LASTEXITCODE -ne 0) {
  throw "tunnel-client doctor failed."
}

Write-Host ":: Starting the Secure MCP Tunnel daemon..." -ForegroundColor Cyan
& $TunnelClient run --profile $ProfileName
