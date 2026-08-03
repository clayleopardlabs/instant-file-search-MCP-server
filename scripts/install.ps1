[CmdletBinding()]
param(
    [ValidateSet('codex', 'opencode', 'claude', 'all')]
    [string[]]$Clients,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\instant-file-search'),
    [string]$ReleaseBase = 'https://github.com/clayleopardlabs/instant-file-search-MCP-server/releases/latest/download',
    [string]$ServerBinary,
    [string]$IndexerBinary,
    [string]$VendorDir,
    [string]$ExpectedSha256,
    [switch]$SkipDownload,
    [switch]$SkipElevation,
    [switch]$SkipCodex,
    [switch]$SkipOpenCode,
    [switch]$SkipClaude,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$serverName = 'instant-file-search'
$binaryName = 'instant-file-search-mcp-server.exe'
$indexerName = 'instant-file-search-indexer.exe'
$serviceName = 'instant-file-search-indexer'
$doctorName = 'doctor.ps1'
$repoRoot = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { $null }
$isCheckout = $repoRoot -and (Test-Path -LiteralPath (Join-Path $repoRoot 'Cargo.toml'))
$stableBinary = Join-Path $InstallRoot $binaryName
$stableIndexer = Join-Path $InstallRoot $indexerName
$openCodeConfigDir = Join-Path $env:USERPROFILE '.config\opencode'
$openCodePluginRoot = Join-Path $openCodeConfigDir 'plugins\instant-file-search-mcp-plugin'
$claudeConfig = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'

function Write-Step([string]$Message) { Write-Host "`n==> $Message" -ForegroundColor Cyan }
function Write-Action([string]$Message) { if ($DryRun) { Write-Host "DRY RUN: $Message" -ForegroundColor Yellow } }

function Test-Elevated {
    return ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-ClaudeInstalled {
    return (Test-Path -LiteralPath $claudeConfig) -or
        (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Programs\Claude\Claude.exe')) -or
        (Get-Process -Name Claude -ErrorAction SilentlyContinue) -or
        (Get-Command claude -ErrorAction SilentlyContinue)
}

function Get-DetectedClients {
    $found = @()
    if ((Get-Command codex -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.codex\config.toml'))) { $found += 'codex' }
    if ((Get-Command opencode -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath $openCodeConfigDir)) { $found += 'opencode' }
    if (Test-ClaudeInstalled) { $found += 'claude' }
    return $found
}

function Select-InstallClients {
    $detected = @(Get-DetectedClients)
    if ($Clients -and ($Clients -contains 'all')) { return $detected }
    if ($Clients) { return @($Clients | Where-Object { $_ -ne 'all' } | Select-Object -Unique) }

    # If only one client is present, set it up without prompting - simplest path
    # for a first-time user.
    if ($detected.Count -eq 1) {
        Write-Host "Detected $($detected[0]) only - configuring it automatically." -ForegroundColor Green
        return $detected
    }

    Write-Host "Detected clients: $(if($detected){$detected -join ', '}else{'none'})" -ForegroundColor Green
    Write-Host 'Choose clients to configure: [A]ll detected, or enter a comma-separated list: codex, opencode, claude'
    $answer = Read-Host 'Selection (default A)'
    if ([string]::IsNullOrWhiteSpace($answer)) { return $detected }
    $choice = $answer.Trim().ToLowerInvariant()
    if ($choice -eq 'a') { return $detected }
    if ($choice -eq 'n') { return @() }
    $selected = @($choice -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $invalid = @($selected | Where-Object { $_ -notin @('codex', 'opencode', 'claude') })
    if ($invalid) { throw "Unknown client(s): $($invalid -join ', '). Use codex, opencode, claude, or all detected." }
    return @($selected | Select-Object -Unique)
}

function Get-ReleaseAsset([string]$Name, [string]$Dest) {
    $url = "$ReleaseBase/$Name"
    Write-Host "   Downloading $Name..." -ForegroundColor Gray
    $parent = Split-Path -Parent $Dest
    if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    Invoke-WebRequest -Uri $url -OutFile $Dest -UseBasicParsing
    if (-not (Test-Path -LiteralPath $Dest)) { throw "Download failed: $url" }
    Write-Host "   Saved $Dest" -ForegroundColor Green
}

function Resolve-ServerBinary {
    if ($ServerBinary) {
        if (-not (Test-Path -LiteralPath $ServerBinary)) { throw "Server binary not found: $ServerBinary" }
        return $ServerBinary
    }
    if ($isCheckout) {
        $local = Join-Path $repoRoot "target\release\$binaryName"
        if (Test-Path -LiteralPath $local) { return $local }
    }
    if ($SkipDownload) { throw "Server binary not found and -SkipDownload was set. Pass -ServerBinary or remove -SkipDownload." }
    New-Item -ItemType Directory -Path $InstallRoot -Force | Out-Null
    Get-ReleaseAsset $binaryName $stableBinary
    if ($ExpectedSha256) {
        $actual = (Get-FileHash -LiteralPath $stableBinary -Algorithm SHA256).Hash
        if ($actual -ne $ExpectedSha256.ToUpperInvariant()) {
            Remove-Item -LiteralPath $stableBinary -Force
            throw "SHA-256 verification failed for '$binaryName'. Expected '$ExpectedSha256', got '$actual'."
        }
        Write-Host "   SHA-256 verified." -ForegroundColor Green
    }
    return $stableBinary
}

function Resolve-IndexerBinary {
    if ($IndexerBinary) {
        if (-not (Test-Path -LiteralPath $IndexerBinary)) { throw "Indexer binary not found: $IndexerBinary" }
        return $IndexerBinary
    }
    if ($isCheckout) {
        $local = Join-Path $repoRoot "target\release\$indexerName"
        if (Test-Path -LiteralPath $local) { return $local }
    }
    if ($SkipDownload) { return $null }
    New-Item -ItemType Directory -Path $InstallRoot -Force | Out-Null
    Get-ReleaseAsset $indexerName $stableIndexer
    return $stableIndexer
}

function Deploy-BundledEngine {
    $bundleDir = Join-Path $InstallRoot 'everything'
    $zipName = 'instant-file-search-fallback-engine-1.5.0.1418b.zip'
    $iniName = 'instant-file-search-fallback-engine-1.5.0.1418b.ini'
    $licenseName = 'LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt'

    $vendorSource = $VendorDir
    if (-not $vendorSource -and $isCheckout) {
        $candidate = Join-Path $repoRoot 'vendor\everything'
        if (Test-Path -LiteralPath $candidate) { $vendorSource = $candidate }
    }

    if ($vendorSource -and (Test-Path -LiteralPath (Join-Path $vendorSource $zipName))) {
        New-Item -ItemType Directory -Path $bundleDir -Force | Out-Null
        Expand-Archive -LiteralPath (Join-Path $vendorSource $zipName) -DestinationPath $bundleDir -Force
        Copy-Item -LiteralPath (Join-Path $vendorSource $iniName) -Destination $bundleDir -Force
        Copy-Item -LiteralPath (Join-Path $vendorSource $licenseName) -Destination $InstallRoot -Force
        Write-Host "PASS: Bundled Fallback Engine deployed to '$bundleDir'." -ForegroundColor Green
        return
    }

    if ($SkipDownload) {
        Write-Host 'WARN: fallback engine not available (no local vendor, -SkipDownload set). The MCP will use an installed Everything if present.' -ForegroundColor Yellow
        return
    }

    New-Item -ItemType Directory -Path $bundleDir -Force | Out-Null
    $tmpZip = Join-Path $env:TEMP $zipName
    try {
        Get-ReleaseAsset $zipName $tmpZip
        Expand-Archive -LiteralPath $tmpZip -DestinationPath $bundleDir -Force
        Remove-Item -LiteralPath $tmpZip -Force -ErrorAction SilentlyContinue
        Get-ReleaseAsset $iniName (Join-Path $bundleDir $iniName)
        Get-ReleaseAsset $licenseName (Join-Path $InstallRoot $licenseName)
        Write-Host "PASS: Fallback Engine downloaded to '$bundleDir'." -ForegroundColor Green
    } catch {
        Remove-Item -LiteralPath $tmpZip -Force -ErrorAction SilentlyContinue
        Write-Host "WARN: could not deploy the bundled Fallback Engine ($($_.Exception.Message)). The MCP will use an installed Everything if one is present; otherwise searches will report an engine error until this is resolved." -ForegroundColor Yellow
    }
}

function Backup-Config([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $backup = "$Path.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    Copy-Item -LiteralPath $Path -Destination $backup -Force
    Write-Host "Backup created: $backup" -ForegroundColor DarkGray
}

# JSONC-aware parse: strips // and /* */ comments and trailing commas
# while staying string-aware (so URLs and `"rm -rf /*"` survive).
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
            # skip the comma if the next non-whitespace char is } or ]
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
        catch { throw "Could not parse '$Path' as JSON. A backup was not changed; edit JSON/JSONC comments first or use the client-specific manual configuration." }
    }
}

function Ensure-Property($Object, [string]$Name, $Value) {
    $property = $Object.PSObject.Properties[$Name]
    if ($property) { $Object.$Name = $Value }
    else { $Object | Add-Member -NotePropertyName $Name -NotePropertyValue $Value }
}

function Write-JsonConfig([string]$Path, $Config) {
    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    Backup-Config $Path
    $Config | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $Path -Encoding UTF8
}

# Use the existing config file (opencode.jsonc preferred over opencode.json),
# falling back to creating opencode.json for a fresh install.
function Resolve-OpenCodeConfig {
    $json = Join-Path $openCodeConfigDir 'opencode.json'
    $jsonc = Join-Path $openCodeConfigDir 'opencode.jsonc'
    if (Test-Path -LiteralPath $jsonc) { return $jsonc }
    if (Test-Path -LiteralPath $json) { return $json }
    return $json
}

# If we switched to opencode.jsonc but a previous install left an opencode.json
# whose only entry was ours, remove it so the config isn't split across files.
function Remove-OrphanOpenCodeJson([string]$ActiveConfig) {
    $json = Join-Path $openCodeConfigDir 'opencode.json'
    $jsonc = Join-Path $openCodeConfigDir 'opencode.jsonc'
    if ($ActiveConfig -ne $jsonc -or -not (Test-Path -LiteralPath $json)) { return }
    try {
        $cfg = Read-JsonConfig $json
        $mcp = $cfg.PSObject.Properties['mcp'].Value
        $keys = @($mcp.PSObject.Properties.Name)
        if ($keys.Count -eq 1 -and $keys[0] -eq $serverName) {
            Backup-Config $json
            Remove-Item -LiteralPath $json -Force
            Write-Host "Removed orphaned config '$json' (its only entry was '$serverName', now merged into '$jsonc')." -ForegroundColor Gray
        }
    } catch { /* leave it alone */ }
}

function Install-Codex {
    Write-Step 'Configuring Codex'
    $codex = Get-Command codex -ErrorAction SilentlyContinue
    if (-not $codex) { Write-Host 'WARN: codex.exe was not found on PATH; skipping Codex registration.' -ForegroundColor Yellow; return }
    if ($DryRun) { Write-Action "codex mcp add $serverName -- '$stableBinary'"; return }

    $listOutput = (& $codex.Source mcp list 2>&1 | Out-String)
    if ($listOutput -match '(?im)\beverything\b' -or $listOutput -match '(?im)\binstant-file-search\b') {
        $helpOutput = (& $codex.Source mcp --help 2>&1 | Out-String)
        if ($helpOutput -match '(?im)\bremove\b') {
            & $codex.Source mcp remove $serverName
            if ($LASTEXITCODE -ne 0) { throw "Could not replace existing Codex MCP server '$serverName'." }
            & $codex.Source mcp add $serverName -- $stableBinary
        } else {
            Write-Host "Codex server '$serverName' already exists; leaving it unchanged because this Codex version lacks 'mcp remove'." -ForegroundColor Yellow
            return
        }
    } else {
        & $codex.Source mcp add $serverName -- $stableBinary
    }
    if ($LASTEXITCODE -ne 0) { throw 'Codex MCP registration failed.' }
    Write-Host 'PASS: Codex MCP server registered.' -ForegroundColor Green
}

function Install-OpenCode {
    Write-Step 'Configuring OpenCode'
    $openCodeConfig = Resolve-OpenCodeConfig
    $pluginSource = Join-Path $repoRoot 'plugin'
    $pluginDist = Join-Path $pluginSource 'dist'
    $localPlugin = $isCheckout -and (Test-Path -LiteralPath $pluginDist)
    $npm = Get-Command npm -ErrorAction SilentlyContinue

    if ($DryRun) {
        Write-Action "Install plugin files into '$openCodePluginRoot'"
        Write-Action "Set user INSTANT_FS_MCP_BINARY to '$stableBinary'"
        Write-Action "Add MCP server '$serverName' to '$openCodeConfig'"
        return
    }

    New-Item -ItemType Directory -Path $openCodePluginRoot -Force | Out-Null
    if ($localPlugin) {
        Copy-Item -LiteralPath $pluginDist -Destination $openCodePluginRoot -Recurse -Force
        Copy-Item -LiteralPath (Join-Path $pluginSource 'package.json') -Destination $openCodePluginRoot -Force
        $lockfile = Join-Path $pluginSource 'package-lock.json'
        if (Test-Path -LiteralPath $lockfile) { Copy-Item -LiteralPath $lockfile -Destination $openCodePluginRoot -Force }
        Write-Host "   Plugin files copied from checkout to '$openCodePluginRoot'." -ForegroundColor Gray
    } elseif (-not $SkipDownload) {
        try {
            Get-ReleaseAsset 'instant-file-search-mcp-plugin-index.js' (Join-Path $openCodePluginRoot 'dist\index.js')
            Get-ReleaseAsset 'instant-file-search-mcp-plugin-package.json' (Join-Path $openCodePluginRoot 'package.json')
            Get-ReleaseAsset 'instant-file-search-mcp-plugin-package-lock.json' (Join-Path $openCodePluginRoot 'package-lock.json')
        } catch {
            Write-Host "WARN: could not download the OpenCode plugin from the release ($($_.Exception.Message)); skipping the sub-agent adapter." -ForegroundColor Yellow
            Remove-Item -LiteralPath $openCodePluginRoot -Recurse -Force -ErrorAction SilentlyContinue
            New-Item -ItemType Directory -Path $openCodePluginRoot -Force | Out-Null
            $npm = $null
        }
    } else {
        Write-Host 'WARN: no local plugin build and -SkipDownload was set; skipping the OpenCode sub-agent adapter.' -ForegroundColor Yellow
        $npm = $null
    }

    if ($npm -and (Test-Path -LiteralPath (Join-Path $openCodePluginRoot 'package.json'))) {
        Push-Location $openCodePluginRoot
        try {
            & $npm.Source ci --omit=dev --ignore-scripts
            if ($LASTEXITCODE -ne 0) { throw "npm ci exited $LASTEXITCODE" }
            Write-Host "PASS: OpenCode plugin installed globally at '$openCodePluginRoot'." -ForegroundColor Green
        } catch {
            Write-Host "WARN: plugin dependency install failed ($($_.Exception.Message)). The MCP server still works for the main session; sub-agents may not have the tools until npm ci succeeds." -ForegroundColor Yellow
        } finally { Pop-Location }
    } elseif (Test-Path -LiteralPath (Join-Path $pluginSource 'node_modules')) {
        Copy-Item -LiteralPath (Join-Path $pluginSource 'node_modules') -Destination $openCodePluginRoot -Recurse -Force
        Write-Host "PASS: OpenCode plugin node_modules copied from checkout." -ForegroundColor Green
    } elseif (Test-Path -LiteralPath (Join-Path $openCodePluginRoot 'dist\index.js')) {
        Write-Host "NOTE: plugin dist present but dependencies not installed; sub-agent tools may not load until 'npm ci' runs in '$openCodePluginRoot'." -ForegroundColor Yellow
    }

    [Environment]::SetEnvironmentVariable('INSTANT_FS_MCP_BINARY', $stableBinary, 'User')
    $config = Read-JsonConfig $openCodeConfig
    $mcp = $config.PSObject.Properties['mcp'].Value
    if (-not $mcp) { $mcp = [pscustomobject]@{}; Ensure-Property $config 'mcp' $mcp }
    Ensure-Property $mcp $serverName ([pscustomobject]@{ command = @($stableBinary); enabled = $true })
    Write-JsonConfig $openCodeConfig $config
    Write-Host "PASS: OpenCode MCP server '$serverName' added to '$openCodeConfig'." -ForegroundColor Green
    Remove-OrphanOpenCodeJson $openCodeConfig
}

function Install-Claude {
    Write-Step 'Configuring Claude Desktop'
    if ($DryRun) { Write-Action "Add MCP server '$serverName' to '$claudeConfig'"; return }
    $config = Read-JsonConfig $claudeConfig
    $servers = $config.PSObject.Properties['mcpServers'].Value
    if (-not $servers) { $servers = [pscustomobject]@{}; Ensure-Property $config 'mcpServers' $servers }
    Ensure-Property $servers $serverName ([pscustomobject]@{ command = $stableBinary; args = @() })
    Write-JsonConfig $claudeConfig $config
    Write-Host "PASS: Claude Desktop MCP server configured in '$claudeConfig'." -ForegroundColor Green
}

function Install-NativeService {
    Write-Step 'Installing the native indexer service'
    $indexerDir = Join-Path $InstallRoot 'indexer'
    $serviceIndexer = Join-Path $indexerDir $indexerName
    $registerHelper = Join-Path $indexerDir 'register-indexer-service.ps1'

    $resolved = Resolve-IndexerBinary
    if (-not $resolved) {
        Write-Host 'WARN: no indexer binary available; the native engine will not be installed. Searches will use the Fallback Engine.' -ForegroundColor Yellow
        return
    }

    if ($DryRun) {
        Write-Action "Copy '$resolved' to '$serviceIndexer'"
        Write-Action "sc.exe create $serviceName binPath= `"$serviceIndexer service`" start= auto"
        return
    }

    New-Item -ItemType Directory -Path $indexerDir -Force | Out-Null
    try {
        Copy-Item -LiteralPath $resolved -Destination $serviceIndexer -Force
    } catch {
        Write-Host "WARN: could not replace '$serviceIndexer' (the running service may hold a lock). The existing indexer binary will be used; restart the service after re-registering." -ForegroundColor Yellow
    }

    $elevated = Test-Elevated

    # Write a tiny helper that performs the admin-only registration, so we can
    # run it elevated on demand without quoting gymnastics.
    $helper = @"
`$ErrorActionPreference = 'Stop'
`$serviceName = '$serviceName'
`$serviceIndexer = '$serviceIndexer'
`$existing = Get-Service -Name `$serviceName -ErrorAction SilentlyContinue
if (`$existing) {
  if (`$existing.Status -ne 'Stopped') { Stop-Service -Name `$serviceName -Force; Start-Sleep -Seconds 2 }
  & sc.exe delete `$serviceName | Out-Null
  Start-Sleep -Seconds 1
}
& sc.exe create `$serviceName binPath= "`$serviceIndexer service" start= auto | Out-Null
if (`$LASTEXITCODE -ne 0) { throw "sc.exe create failed for service `$serviceName." }
Start-Service -Name `$serviceName
"@
    Set-Content -LiteralPath $registerHelper -Value $helper -Encoding UTF8

    if ($elevated) {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $registerHelper
        if ($LASTEXITCODE -ne 0) { throw "Indexer service registration failed (exit $LASTEXITCODE)." }
        Write-Host "PASS: indexer service '$serviceName' installed and started (auto-start)." -ForegroundColor Green
        return
    }

    if ($SkipElevation) {
        Write-Host 'WARN: not running elevated and -SkipElevation was set, so the indexer service was NOT registered. Searches will use the Fallback Engine until you register it. When you are ready, run this elevated:' -ForegroundColor Yellow
        Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$registerHelper`"" -ForegroundColor Gray
        return
    }

    Write-Host 'The native indexer service needs administrator rights. A UAC prompt will appear - click Yes.' -ForegroundColor Yellow
    try {
        Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$registerHelper`"" -Wait -ErrorAction Stop
        $svc = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
        if ($svc) { Write-Host "PASS: indexer service '$serviceName' installed and started (auto-start)." -ForegroundColor Green }
        else { Write-Host "WARN: elevation did not produce the '$serviceName' service. Run the helper elevated manually:" -ForegroundColor Yellow; Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$registerHelper`"" -ForegroundColor Gray }
    } catch {
        Write-Host "WARN: elevation was cancelled or failed: $($_.Exception.Message)" -ForegroundColor Yellow
        Write-Host "Run this elevated when ready:" -ForegroundColor Yellow
        Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$registerHelper`"" -ForegroundColor Gray
    }
}

function Install-Doctor {
    # Make doctor.ps1 available even for one-liner installs (no checkout).
    if ($DryRun) { Write-Action "Install '$doctorName' into '$InstallRoot'"; return }
    New-Item -ItemType Directory -Path $InstallRoot -Force | Out-Null
    $docDest = Join-Path $InstallRoot $doctorName
    if ($isCheckout -and (Test-Path -LiteralPath (Join-Path $repoRoot "scripts\$doctorName"))) {
        Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\$doctorName") -Destination $docDest -Force
        Write-Host "PASS: diagnostics script installed at '$docDest'." -ForegroundColor Green
    } elseif (-not $SkipDownload) {
        try {
            Get-ReleaseAsset $doctorName $docDest
            Write-Host "PASS: diagnostics script installed at '$docDest'." -ForegroundColor Green
        } catch {
            Write-Host "WARN: could not download '$doctorName'; no diagnostics script installed." -ForegroundColor Yellow
        }
    }
}

function Test-Installation {
    Write-Step 'Verifying the installation'
    if (-not (Test-Path -LiteralPath $stableBinary)) {
        Write-Host "FAIL: MCP server binary not found at '$stableBinary'." -ForegroundColor Red
        return $false
    }
    $inFile = Join-Path $env:TEMP "installer-verify-in-$PID.txt"
    $outFile = Join-Path $env:TEMP "installer-verify-out-$PID.txt"
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"installer","version":"1.0"}}}' |
        Set-Content -LiteralPath $inFile -Encoding UTF8
    $proc = Start-Process -FilePath $stableBinary -RedirectStandardInput $inFile -RedirectStandardOutput $outFile -PassThru -NoNewWindow
    $alive = $proc.WaitForExit(4000)
    if ($alive) {
        $out = Get-Content -LiteralPath $outFile -Raw -ErrorAction SilentlyContinue
        if ($out -match 'jsonrpc') { Write-Host 'PASS: MCP server starts and answers initialize.' -ForegroundColor Green; $ok = $true }
        else { Write-Host 'WARN: MCP server started but produced no JSON-RPC output.' -ForegroundColor Yellow; $ok = $false }
        if (-not $proc.HasExited) { $proc.Kill() }
    } else {
        Write-Host 'WARN: MCP server exited within 4s of startup. Enable EVERYTHING_MCP_LOG=debug to diagnose.' -ForegroundColor Yellow
        $ok = $false
    }
    Remove-Item $inFile, $outFile -Force -ErrorAction SilentlyContinue
    return $ok
}

Write-Host 'Instant File Search MCP installer' -ForegroundColor Green
Write-Host "Install root: $InstallRoot" -ForegroundColor Gray
if ($isCheckout) { Write-Host 'Source: local checkout' -ForegroundColor Gray }
else { Write-Host "Source: GitHub release ($ReleaseBase)" -ForegroundColor Gray }

$selected = @(Select-InstallClients)
if ($SkipCodex) { $selected = @($selected | Where-Object { $_ -ne 'codex' }) }
if ($SkipOpenCode) { $selected = @($selected | Where-Object { $_ -ne 'opencode' }) }
if ($SkipClaude) { $selected = @($selected | Where-Object { $_ -ne 'claude' }) }

# Always install the server, engines, and diagnostics even when no supported
# MCP client was detected - a user may configure any MCP host manually.
if (-not $selected) {
    Write-Host 'No MCP clients detected (Codex, OpenCode, or Claude Desktop).' -ForegroundColor Yellow
    Write-Host 'The MCP server and engines will still be installed so you can configure any host manually.' -ForegroundColor Yellow
} else {
    Write-Host "Selected: $($selected -join ', ')" -ForegroundColor Green
}

Write-Step 'Obtaining the MCP server binary'
$serverSource = Resolve-ServerBinary
if (-not $DryRun -and $serverSource -ne $stableBinary) {
    New-Item -ItemType Directory -Path $InstallRoot -Force | Out-Null
    try {
        Copy-Item -LiteralPath $serverSource -Destination $stableBinary -Force
        Write-Host "PASS: MCP server installed at '$stableBinary'." -ForegroundColor Green
    } catch {
        Write-Host "WARN: could not replace '$stableBinary' (an MCP host may currently have it loaded). The existing binary will be used; restart your AI app and re-run this installer to update it." -ForegroundColor Yellow
    }
}

Write-Step 'Deploying the Fallback Engine'
if ($DryRun) { Write-Action "Deploy Fallback Engine into '$(Join-Path $InstallRoot 'everything')'" }
else { Deploy-BundledEngine }

foreach ($client in $selected) {
    switch ($client) {
        'codex' { Install-Codex }
        'opencode' { Install-OpenCode }
        'claude' { Install-Claude }
    }
}

Install-NativeService
Install-Doctor

$everything = Get-Process -Name Everything -ErrorAction SilentlyContinue
if ($everything) { Write-Host 'PASS: Fallback Engine is running.' -ForegroundColor Green }
elseif (Test-Path -LiteralPath (Join-Path $InstallRoot 'everything\Everything.exe')) {
    Write-Host 'PASS: Fallback Engine is not running, but the bundled engine will start automatically on first search.' -ForegroundColor Green
} else {
    Write-Host 'WARN: Fallback Engine is not running and no bundled engine was deployed.' -ForegroundColor Yellow
}

if (-not $DryRun) { Test-Installation | Out-Null }

Write-Host "`nInstalled binary: $stableBinary" -ForegroundColor Green
Write-Host "Diagnostics:      $((Join-Path $InstallRoot $doctorName))" -ForegroundColor Gray
if (-not $selected) {
    Write-Host '' -ForegroundColor Gray
    Write-Host 'No MCP client was auto-configured. To use this with your AI app, point it at the binary above.' -ForegroundColor Yellow
    Write-Host 'See the README section "Set up a single app yourself" for per-app config examples.' -ForegroundColor Yellow
}
if (-not $elevated -and (Get-Service -Name $serviceName -ErrorAction SilentlyContinue) -eq $null) {
    Write-Host 'NOTE: the native indexer service is not registered yet (needs admin). Searches work now via the Fallback Engine; to enable the fast native indexer, run the printed elevated command or re-run this installer elevated.' -ForegroundColor Yellow
}
Write-Host 'Restart your AI app (or start a new session) so it reloads the MCP configuration.'
