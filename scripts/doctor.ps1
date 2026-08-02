[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\EverythingMCP')
)

$ErrorActionPreference = 'Continue'
$serverName = 'instant-file-search'
$binaryName = 'instant-file-search-mcp-server.exe'
$stableBinary = Join-Path $InstallRoot $binaryName
$openCodePluginRoot = Join-Path $env:USERPROFILE '.config\opencode\plugins\instant-file-search-mcp-plugin'
$openCodeConfig = Join-Path $env:USERPROFILE '.config\opencode\opencode.json'
$claudeConfig = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'
$failures = 0
$warnings = 0

function Pass([string]$Message) { Write-Host "PASS: $Message" -ForegroundColor Green }
function Warn([string]$Message) { $script:warnings++; Write-Host "WARN: $Message" -ForegroundColor Yellow }
function Fail([string]$Message) { $script:failures++; Write-Host "FAIL: $Message" -ForegroundColor Red }

Write-Host 'Instant File Search MCP doctor' -ForegroundColor Cyan

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
    } elseif ($listOutput -match '(?im)\binstant-file-search\b' -or $listOutput -match '(?im)\beverything\b') {
        Pass "Codex MCP server '$serverName' is registered."
    } else {
        Fail "Codex MCP server '$serverName' is not registered. Run .\scripts\install.ps1."
    }
}

$everything = Get-Process -Name Everything -ErrorAction SilentlyContinue
if ($everything) {
    Pass 'Everything is running.'
} else {
    Warn 'Everything is not running. It will auto-start on first search (if a bundled or installed engine is available).'
}

$bundleDir = Join-Path $InstallRoot 'everything'
$bundledEngine = Join-Path $bundleDir 'Everything.exe'
if (Test-Path -LiteralPath $bundledEngine) {
    Pass "Bundled Everything engine present ('$bundledEngine')."
} else {
    Warn "Bundled Everything engine missing ('$bundledEngine'). find_files will fall back to an installed Everything."
}
if (Test-Path -LiteralPath (Join-Path $bundleDir 'Everything.ini')) {
    Pass 'Bundled Everything.ini config present.'
} else {
    Warn 'Bundled Everything.ini is missing; the engine will use default settings.'
}
if (Test-Path -LiteralPath (Join-Path $InstallRoot 'LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt')) {
    Pass 'Everything license notice present (required for redistribution).'
} else {
    Warn 'Everything license notice is missing.'
}

$indexerExe = Join-Path $InstallRoot 'indexer\instant-file-search-indexer.exe'
$indexerSvc = Get-Service -Name 'instant-file-search-indexer' -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $indexerExe) {
    Pass "Native indexer binary present ('$indexerExe')."
} else {
    Warn "Native indexer binary missing ('$indexerExe'). Searches will use the Everything fallback."
}
if ($indexerSvc) {
    if ($indexerSvc.Status -eq 'Running') { Pass 'Native indexer service is running.' }
    else { Warn "Native indexer service exists but is '$($indexerSvc.Status)'. Start it with: sc.exe start instant-file-search-indexer" }
} else {
    Warn "Native indexer service is not installed. Run the installer elevated (or: sc.exe create instant-file-search-indexer binPath= `"$indexerExe service`" start= auto)."
}

if (Test-Path -LiteralPath $stableBinary) {
    # Smoke test: the binary must start, answer MCP initialize, and keep running
    # (if it exits immediately, the engine acquisition path is broken).
    $inFile = Join-Path $env:TEMP "doctor-in-$PID.txt"
    $outFile = Join-Path $env:TEMP "doctor-out-$PID.txt"
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"doctor","version":"1.0"}}}' |
        Set-Content -LiteralPath $inFile -Encoding UTF8
    $proc = Start-Process -FilePath $stableBinary -RedirectStandardInput $inFile -RedirectStandardOutput $outFile -PassThru -NoNewWindow
    $alive = $proc.WaitForExit(3000)
    if ($alive) {
        $out = Get-Content -LiteralPath $outFile -Raw -ErrorAction SilentlyContinue
        if ($out -match 'jsonrpc') { Pass 'MCP binary starts and answers initialize (engine acquisition runs on first tool call).' }
        else { Warn 'MCP binary started but produced no JSON-RPC output.' }
        if (-not $proc.HasExited) { $proc.Kill() }
    } else {
        Warn 'MCP binary exited within 3s of startup; enable EVERYTHING_MCP_LOG=debug to diagnose.'
    }
    Remove-Item $inFile, $outFile -Force -ErrorAction SilentlyContinue
}

$openCodeEntry = Join-Path $openCodePluginRoot 'dist\index.js'
if (Test-Path -LiteralPath $openCodeEntry) {
    Pass "OpenCode plugin is installed in '$openCodePluginRoot'."
} else {
    Warn "OpenCode plugin was not found at '$openCodePluginRoot'. Run .\scripts\install.ps1 or use -SkipOpenCode only if OpenCode is not needed."
}

$userBinary = [Environment]::GetEnvironmentVariable('INSTANT_FS_MCP_BINARY', 'User')
if ($userBinary -eq $stableBinary) {
    Pass 'OpenCode is configured to use the stable installed binary.'
} elseif ($userBinary) {
    Warn "INSTANT_FS_MCP_BINARY points to '$userBinary' instead of the stable installed binary."
} elseif ([Environment]::GetEnvironmentVariable('EVERYTHING_MCP_BINARY', 'User') -eq $stableBinary) {
    Pass 'OpenCode uses the legacy EVERYTHING_MCP_BINARY env var (still supported).'
} else {
    Warn 'INSTANT_FS_MCP_BINARY is not set for OpenCode.'
}

if (Test-Path -LiteralPath $claudeConfig) {
    try {
        $claude = Get-Content -LiteralPath $claudeConfig -Raw | ConvertFrom-Json
        if ($claude.PSObject.Properties['mcpServers'].Value.PSObject.Properties['instant-file-search'] -or $claude.PSObject.Properties['mcpServers'].Value.PSObject.Properties['everything']) {
            Pass 'Claude Desktop MCP server is configured.'
        } else { Warn "Claude Desktop config exists but '$serverName' is not configured." }
    } catch { Warn "Claude Desktop config could not be parsed: $claudeConfig" }
} else {
    Warn "Claude Desktop config was not found at '$claudeConfig'."
}

Write-Host "`nSummary: $failures failure(s), $warnings warning(s)." -ForegroundColor Cyan
if ($failures -gt 0) { exit 1 }
exit 0
