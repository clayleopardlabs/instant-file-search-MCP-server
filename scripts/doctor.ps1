[CmdletBinding()]
param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\instant-file-search'),
    [switch]$RequireNative
)

$ErrorActionPreference = 'Continue'
$serverName = 'instant-file-search'
$binaryName = 'instant-file-search-mcp-server.exe'
$statePath = Join-Path $InstallRoot 'current.json'
$installState = $null
if (Test-Path -LiteralPath $statePath) {
    try { $installState = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json } catch {}
}
$stableBinary = if ($installState -and $installState.server_binary) { $installState.server_binary } else { Join-Path $InstallRoot $binaryName }
$versionRoot = if ($installState -and $installState.version) { Join-Path $InstallRoot (Join-Path 'versions' $installState.version) } else { $InstallRoot }
$openCodePluginRoot = Join-Path $env:USERPROFILE '.config\opencode\plugins\instant-file-search-mcp-plugin'
$openCodeJson = Join-Path $env:USERPROFILE '.config\opencode\opencode.json'
$openCodeJsonc = Join-Path $env:USERPROFILE '.config\opencode\opencode.jsonc'
$claudeConfig = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'
$failures = 0
$warnings = 0

function Pass([string]$Message) { Write-Host "PASS: $Message" -ForegroundColor Green }
function Warn([string]$Message) { $script:warnings++; Write-Host "WARN: $Message" -ForegroundColor Yellow }
function Fail([string]$Message) { $script:failures++; Write-Host "FAIL: $Message" -ForegroundColor Red }

function Find-CodexCli {
    $candidates = @()
    if ($env:CODEX_CLI_PATH) { $candidates += $env:CODEX_CLI_PATH }
    $onPath = Get-Command codex -ErrorAction SilentlyContinue
    if ($onPath -and $onPath.Source) { $candidates += $onPath.Source }
    $bundledRoot = Join-Path $env:LOCALAPPDATA 'OpenAI\Codex\bin'
    if (Test-Path -LiteralPath $bundledRoot) {
        $candidates += @(Get-ChildItem -Path (Join-Path $bundledRoot '*\codex.exe') -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -ExpandProperty FullName)
    }
    foreach ($candidate in ($candidates | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        if ($candidate -match '\\WindowsApps\\') { continue }
        return (Get-Item -LiteralPath $candidate)
    }
    return $null
}

# JSONC-aware parse: strips // and /* */ comments and trailing commas
# while staying string-aware (so URLs and `"rm -rf /*`"` survive).
function ConvertFrom-JsonC([string]$Text) {
    $sb = New-Object System.Text.StringBuilder
    $n = $Text.Length
    $i = 0
    $inString = $false
    while ($i -lt $n) {
        $c = $Text[$i]
        $next = if ($i + 1 -lt $n) { $Text[$i + 1] } else { '' }
        if ($inString) {
            [void]$sb.Append($c)
            if ($c -eq '\' -and $next) { $i++; if ($i -lt $n) { [void]$sb.Append($Text[$i]) } }
            elseif ($c -eq '"') { $inString = $false }
            $i++
        } elseif ($c -eq '"') {
            $inString = $true
            [void]$sb.Append($c)
            $i++
        } elseif ($c -eq '/' -and $next -eq '/') {
            while ($i -lt $n -and $Text[$i] -ne "`n") { $i++ }
        } elseif ($c -eq '/' -and $next -eq '*') {
            $i += 2
            while ($i -lt $n -and -not ($Text[$i] -eq '*' -and $i + 1 -lt $n -and $Text[$i + 1] -eq '/')) { $i++ }
            if ($i -lt $n) { $i += 2 }
        } elseif ($c -eq ',') {
            $j = $i + 1
            while ($j -lt $n -and $Text[$j] -match '\s') { $j++ }
            if ($j -lt $n -and ($Text[$j] -eq '}' -or $Text[$j] -eq ']')) { $i++ }
            else { [void]$sb.Append($c); $i++ }
        } else {
            [void]$sb.Append($c)
            $i++
        }
    }
    return ($sb.ToString() | ConvertFrom-Json)
}

function Read-JsonConfig([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{} }
    $raw = Get-Content -LiteralPath $Path -Raw
    if (-not $raw.Trim()) { return [pscustomobject]@{} }
    try { return ($raw | ConvertFrom-Json) }
    catch {
        try { return (ConvertFrom-JsonC $raw) }
        catch { return $null }
    }
}

# Use the existing config file (opencode.jsonc preferred over opencode.json).
function Resolve-OpenCodeConfig {
    if (Test-Path -LiteralPath $openCodeJsonc) { return $openCodeJsonc }
    if (Test-Path -LiteralPath $openCodeJson) { return $openCodeJson }
    return $null
}

Write-Host 'Instant File Search MCP doctor' -ForegroundColor Cyan

if ($env:OS -eq 'Windows_NT') { Pass 'Running on Windows.' } else { Fail 'This server requires Windows.' }

if (Test-Path -LiteralPath $stableBinary) {
    $file = Get-Item -LiteralPath $stableBinary
    if ($file.Length -gt 0) { Pass "Installed binary exists ($([math]::Round($file.Length / 1MB, 1)) MB)." }
    else { Fail 'Installed binary is empty.' }
} else {
    Fail "Installed binary not found at '$stableBinary'. Run .\scripts\install.ps1."
}

if ($installState) {
    Pass "Active installed version is '$($installState.version)'."
} else {
    Warn 'No current.json deployment state found; treating this as a legacy in-place install.'
}

$codex = Find-CodexCli
if (-not $codex) {
    Fail 'codex.exe is not on PATH.'
} else {
    Pass "Codex found at '$($codex.FullName)'."
    $listOutput = (& $codex.FullName mcp list 2>&1 | Out-String)
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
    Pass 'Fallback Engine is running.'
} else {
    Warn 'Fallback Engine is not running. It will auto-start on first search (if a bundled or installed engine is available).'
}

$bundleDir = Join-Path $versionRoot 'everything'
$bundledEngine = Join-Path $bundleDir 'Everything.exe'
if (Test-Path -LiteralPath $bundledEngine) {
    Pass "Bundled Fallback Engine present ('$bundledEngine')."
} else {
    Warn "Bundled Fallback Engine missing ('$bundledEngine'). find_files will fall back to an installed Everything."
}
if (Test-Path -LiteralPath (Join-Path $bundleDir 'instant-file-search-fallback-engine-1.5.0.1418b.ini')) {
    Pass 'Bundled Fallback Engine ini config present.'
} else {
    Warn 'Bundled Fallback Engine ini is missing; the engine will use default settings.'
}
if (Test-Path -LiteralPath (Join-Path $versionRoot 'LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt')) {
    Pass 'Fallback Engine license notice present (required for redistribution).'
} else {
    Warn 'Fallback Engine license notice is missing.'
}

$indexerExe = if ($installState -and $installState.indexer_binary) { $installState.indexer_binary } else { Join-Path $InstallRoot 'indexer\instant-file-search-indexer.exe' }
$indexerSvc = Get-Service -Name 'instant-file-search-indexer' -ErrorAction SilentlyContinue
$indexerServiceConfig = Get-CimInstance -ClassName Win32_Service -Filter "Name='instant-file-search-indexer'" -ErrorAction SilentlyContinue
$servicePath = if ($indexerServiceConfig) { $indexerServiceConfig.PathName } else { $null }
if (-not $servicePath -and $indexerSvc) {
    $serviceQuery = (& sc.exe qc instant-file-search-indexer 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -and $serviceQuery -match '(?m)^\s*BINARY_PATH_NAME\s*:\s*(.+)$') {
        $servicePath = $Matches[1].Trim()
    }
}
if (Test-Path -LiteralPath $indexerExe) {
    Pass "Native indexer binary present ('$indexerExe')."
} else {
    if ($RequireNative) { Fail "Native indexer binary missing ('$indexerExe')." }
    else { Warn "Native indexer binary missing ('$indexerExe'). Searches will use the Everything fallback." }
}
if ($indexerSvc) {
    if ($indexerSvc.Status -eq 'Running') { Pass 'Native indexer service is running.' }
    elseif ($RequireNative) { Fail "Native indexer service exists but is '$($indexerSvc.Status)'. Start it with: sc.exe start instant-file-search-indexer" }
    else { Warn "Native indexer service exists but is '$($indexerSvc.Status)'. Start it with: sc.exe start instant-file-search-indexer" }
} else {
    $message = "Native indexer service is not installed. Run the installer elevated (or: sc.exe create instant-file-search-indexer binPath= `"$indexerExe service`" start= auto)."
    if ($RequireNative) { Fail $message } else { Warn $message }
}

if ($servicePath -and $servicePath -notmatch [regex]::Escape($indexerExe)) {
    $message = "Partial upgrade: service command does not point to the active indexer binary '$indexerExe'. Re-run the installer to switch it."
    if ($RequireNative) { Fail $message } else { Warn $message }
} elseif ($indexerSvc -and -not $servicePath) {
    $message = 'Could not read the native indexer service command, so its deployed version could not be verified.'
    if ($RequireNative) { Fail $message } else { Warn $message }
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
    try {
        $versionOut = (& $stableBinary --version 2>$null | Select-Object -First 1).Trim()
        if ($installState -and $versionOut -notmatch [regex]::Escape($installState.version)) { Warn "Partial upgrade: server reports '$versionOut', expected version '$($installState.version)'." }
        elseif ($versionOut) { Pass "MCP server version: $versionOut" }
    } catch { Warn 'Could not read MCP server version.' }
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

$activeConfig = Resolve-OpenCodeConfig
if ($activeConfig) {
    $cfg = Read-JsonConfig $activeConfig
    $mcp = if ($cfg) { $cfg.PSObject.Properties['mcp'].Value } else { $null }
    if ($mcp -and $mcp.PSObject.Properties['instant-file-search']) {
        Pass "OpenCode MCP server '$serverName' is configured in '$activeConfig'."
    } else {
        Warn "'$serverName' is not in the OpenCode config '$activeConfig'. Run .\scripts\install.ps1."
    }
} else {
    Warn "No opencode.json or opencode.jsonc found in '$env:USERPROFILE\.config\opencode'. Run .\scripts\install.ps1."
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
