[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\EverythingMCP')
)

$ErrorActionPreference = 'Continue'
$serverName = 'everything'
$binaryName = 'instantaneous-windows-file-search-mcp-server.exe'
$stableBinary = Join-Path $InstallRoot $binaryName
$openCodePluginRoot = Join-Path $env:USERPROFILE '.config\opencode\plugins\everything-mcp-plugin'
$openCodeConfig = Join-Path $env:USERPROFILE '.config\opencode\opencode.json'
$claudeConfig = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'
$failures = 0
$warnings = 0

function Pass([string]$Message) { Write-Host "PASS: $Message" -ForegroundColor Green }
function Warn([string]$Message) { $script:warnings++; Write-Host "WARN: $Message" -ForegroundColor Yellow }
function Fail([string]$Message) { $script:failures++; Write-Host "FAIL: $Message" -ForegroundColor Red }

Write-Host 'Instantaneous Windows File Search MCP doctor' -ForegroundColor Cyan

if ($env:OS -eq 'Windows_NT') { Pass 'Running on Windows.' } else { Fail 'This server requires Windows.' }

if (Test-Path -LiteralPath $stableBinary) {
    $file = Get-Item -LiteralPath $stableBinary
    if ($file.Length -gt 0) { Pass "Installed binary exists ($([math]::Round($file.Length / 1MB, 1)) MB)." }
    else { Fail 'Installed binary is empty.' }
} else {
    Fail "Installed binary not found at '$stableBinary'. Run .\scripts\install.ps1."
}

$codex = Get-Command codex -ErrorAction SilentlyContinue
if (-not $codex) {
    Fail 'codex.exe is not on PATH.'
} else {
    Pass "Codex found at '$($codex.Source)'."
    $listOutput = (& $codex.Source mcp list 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        Fail 'Codex could not list MCP servers.'
    } elseif ($listOutput -match '(?im)\beverything\b') {
        Pass "Codex MCP server 'everything' is registered."
    } else {
        Fail "Codex MCP server 'everything' is not registered. Run .\scripts\install.ps1."
    }
}

$everything = Get-Process -Name Everything -ErrorAction SilentlyContinue
if ($everything) {
    Pass 'Everything is running.'
} else {
    Warn 'Everything is not running. Start Everything before using find_files.'
}

$openCodeEntry = Join-Path $openCodePluginRoot 'dist\index.js'
if (Test-Path -LiteralPath $openCodeEntry) {
    Pass "OpenCode plugin is installed in '$openCodePluginRoot'."
} else {
    Warn "OpenCode plugin was not found at '$openCodePluginRoot'. Run .\scripts\install.ps1 or use -SkipOpenCode only if OpenCode is not needed."
}

$userBinary = [Environment]::GetEnvironmentVariable('EVERYTHING_MCP_BINARY', 'User')
if ($userBinary -eq $stableBinary) {
    Pass 'OpenCode is configured to use the stable installed binary.'
} elseif ($userBinary) {
    Warn "EVERYTHING_MCP_BINARY points to '$userBinary' instead of the stable installed binary."
} else {
    Warn 'EVERYTHING_MCP_BINARY is not set for OpenCode.'
}

if (Test-Path -LiteralPath $claudeConfig) {
    try {
        $claude = Get-Content -LiteralPath $claudeConfig -Raw | ConvertFrom-Json
        if ($claude.PSObject.Properties['mcpServers'].Value.PSObject.Properties['everything']) {
            Pass 'Claude Desktop MCP server is configured.'
        } else { Warn "Claude Desktop config exists but '$serverName' is not configured." }
    } catch { Warn "Claude Desktop config could not be parsed: $claudeConfig" }
} else {
    Warn "Claude Desktop config was not found at '$claudeConfig'."
}

Write-Host "`nSummary: $failures failure(s), $warnings warning(s)." -ForegroundColor Cyan
if ($failures -gt 0) { exit 1 }
exit 0
