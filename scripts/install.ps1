[CmdletBinding()]
param(
    [ValidateSet('codex', 'opencode', 'claude', 'all')]
    [string[]]$Clients,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\EverythingMCP'),
    [string]$ReleaseBase = 'https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server/releases/latest/download',
    [string]$ServerBinary,
    [string]$IndexerBinary,
    [string]$VendorDir,
    [string]$ExpectedSha256,
    [switch]$SkipDownload,
    [switch]$SkipCodex,
    [switch]$SkipOpenCode,
    [switch]$SkipClaude,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$serverName = 'instant-file-search'
$binaryName = 'instant-file-search-mcp-server.exe'
$indexerName = 'instant-file-search-indexer.exe'
$repoRoot = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { $null }
$isCheckout = $repoRoot -and (Test-Path -LiteralPath (Join-Path $repoRoot 'Cargo.toml'))
$stableBinary = Join-Path $InstallRoot $binaryName
$stableIndexer = Join-Path $InstallRoot $indexerName
$openCodeConfigDir = Join-Path $env:USERPROFILE '.config\opencode'
$openCodeConfig = Join-Path $openCodeConfigDir 'opencode.json'
$openCodePluginRoot = Join-Path $openCodeConfigDir 'plugins\instant-file-search-mcp-plugin'
$claudeConfig = Join-Path $env:APPDATA 'Claude\claude_desktop_config.json'

function Write-Step([string]$Message) { Write-Host "`n==> $Message" -ForegroundColor Cyan }
function Write-Action([string]$Message) { if ($DryRun) { Write-Host "DRY RUN: $Message" -ForegroundColor Yellow } }

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

    Write-Host "Detected clients: $(if($detected){$detected -join ', '}else{'none'})" -ForegroundColor Green
    Write-Host 'Choose clients to configure: [A]ll detected, or enter a comma-separated list: codex, opencode, claude'
    $choice = (Read-Host 'Selection (default A)').Trim().ToLowerInvariant()
    if (-not $choice -or $choice -eq 'a') { return $detected }
    if ($choice -eq 'n') { return @() }
    $selected = @($choice -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $invalid = @($selected | Where-Object { $_ -notin @('codex', 'opencode', 'claude') })
    if ($invalid) { throw "Unknown client(s): $($invalid -join ', '). Use codex, opencode, claude, or all detected." }
    return @($selected | Select-Object -Unique)
}

function Get-ReleaseAsset([string]$Name, [string]$Dest) {
    $url = "$ReleaseBase/$Name"
    Write-Host "   Downloading $Name..." -ForegroundColor Gray
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
        Write-Host "PASS: Bundled Everything deployed to '$bundleDir'." -ForegroundColor Green
        return
    }

    if ($SkipDownload) {
        Write-Host 'WARN: bundled engine not available (no local vendor, -SkipDownload set). The MCP will use an installed Everything if present.' -ForegroundColor Yellow
        return
    }

    New-Item -ItemType Directory -Path $bundleDir -Force | Out-Null
    $tmpZip = Join-Path $env:TEMP $zipName
    Get-ReleaseAsset $zipName $tmpZip
    Expand-Archive -LiteralPath $tmpZip -DestinationPath $bundleDir -Force
    Remove-Item -LiteralPath $tmpZip -Force -ErrorAction SilentlyContinue
    Get-ReleaseAsset $iniName (Join-Path $bundleDir $iniName)
    Get-ReleaseAsset $licenseName (Join-Path $InstallRoot $licenseName)
    Write-Host "PASS: Bundled Everything downloaded to '$bundleDir'." -ForegroundColor Green
}

function Backup-Config([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $backup = "$Path.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    Copy-Item -LiteralPath $Path -Destination $backup -Force
    Write-Host "Backup created: $backup" -ForegroundColor DarkGray
}

function Read-JsonConfig([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return [pscustomobject]@{} }
    $raw = Get-Content -LiteralPath $Path -Raw
    if (-not $raw.Trim()) { return [pscustomobject]@{} }
    try { return ($raw | ConvertFrom-Json) }
    catch { throw "Could not parse '$Path' as JSON. A backup was not changed; edit JSON/JSONC comments first or use the client-specific manual configuration." }
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
    $pluginSource = Join-Path $repoRoot 'plugin'
    $pluginDist = Join-Path $pluginSource 'dist'
    $hasPlugin = $isCheckout -and (Test-Path -LiteralPath $pluginDist)

    if ($DryRun) {
        Write-Action "Install plugin files into '$openCodePluginRoot'"
        Write-Action "Set user INSTANT_FS_MCP_BINARY to '$stableBinary'"
        Write-Action "Add MCP server '$serverName' to '$openCodeConfig'"
        return
    }

    if ($hasPlugin) {
        New-Item -ItemType Directory -Path $openCodePluginRoot -Force | Out-Null
        Copy-Item -LiteralPath $pluginDist -Destination $openCodePluginRoot -Recurse -Force
        Copy-Item -LiteralPath (Join-Path $pluginSource 'package.json') -Destination $openCodePluginRoot -Force
        $lockfile = Join-Path $pluginSource 'package-lock.json'
        if (Test-Path -LiteralPath $lockfile) { Copy-Item -LiteralPath $lockfile -Destination $openCodePluginRoot -Force }

        $npm = Get-Command npm -ErrorAction SilentlyContinue
        if ($npm) {
            Push-Location $openCodePluginRoot
            try {
                & $npm.Source ci --omit=dev --ignore-scripts
                if ($LASTEXITCODE -ne 0) { throw 'npm dependency installation for the OpenCode plugin failed.' }
            } finally { Pop-Location }
        } elseif (Test-Path -LiteralPath (Join-Path $pluginSource 'node_modules')) {
            Copy-Item -LiteralPath (Join-Path $pluginSource 'node_modules') -Destination $openCodePluginRoot -Recurse -Force
        } else { Write-Host 'WARN: npm and plugin/node_modules were not found; OpenCode plugin dependencies are missing.' -ForegroundColor Yellow }
        Write-Host "PASS: OpenCode plugin installed globally at '$openCodePluginRoot'." -ForegroundColor Green
    } else {
        Write-Host 'NOTE: no plugin build found in this checkout; skipping the OpenCode sub-agent adapter. The MCP entry below still serves the main OpenCode session.' -ForegroundColor Yellow
    }

    [Environment]::SetEnvironmentVariable('INSTANT_FS_MCP_BINARY', $stableBinary, 'User')
    $config = Read-JsonConfig $openCodeConfig
    $mcp = $config.PSObject.Properties['mcp'].Value
    if (-not $mcp) { $mcp = [pscustomobject]@{}; Ensure-Property $config 'mcp' $mcp }
    Ensure-Property $mcp $serverName ([pscustomobject]@{ command = @($stableBinary); enabled = $true })
    Write-JsonConfig $openCodeConfig $config
    Write-Host "PASS: OpenCode MCP server '$serverName' added to '$openCodeConfig'." -ForegroundColor Green
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
    $serviceName = 'instant-file-search-indexer'

    $resolved = Resolve-IndexerBinary
    if (-not $resolved) {
        Write-Host 'WARN: no indexer binary available; the native engine will not be installed. Searches will use the bundled Everything engine.' -ForegroundColor Yellow
        return
    }

    if ($DryRun) {
        Write-Action "Copy '$resolved' to '$serviceIndexer'"
        Write-Action "sc.exe create $serviceName binPath= `"$serviceIndexer service`" start= auto"
        return
    }

    New-Item -ItemType Directory -Path $indexerDir -Force | Out-Null
    Copy-Item -LiteralPath $resolved -Destination $serviceIndexer -Force

    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $elevated) {
        Write-Host 'WARN: not running elevated — cannot register the indexer service. Re-run this installer from an elevated prompt (or register it manually):' -ForegroundColor Yellow
        Write-Host "  sc.exe create $serviceName binPath= `"$serviceIndexer service`" start= auto" -ForegroundColor DarkGray
        return
    }

    $existing = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($existing) {
        if ($existing.Status -ne 'Stopped') { Stop-Service -Name $serviceName -Force; Start-Sleep -Seconds 2 }
        & sc.exe delete $serviceName | Out-Null
        Start-Sleep -Seconds 1
    }
    & sc.exe create $serviceName binPath= "`"$serviceIndexer service`"" start= auto | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe create failed for service '$serviceName'." }
    Start-Service -Name $serviceName
    Write-Host "PASS: indexer service '$serviceName' installed and started (auto-start)." -ForegroundColor Green
}

Write-Host 'Instant File Search MCP installer' -ForegroundColor Green
Write-Host "Install root: $InstallRoot" -ForegroundColor Gray
if ($isCheckout) { Write-Host 'Source: local checkout' -ForegroundColor Gray }
else { Write-Host "Source: GitHub release ($ReleaseBase)" -ForegroundColor Gray }

$selected = @(Select-InstallClients)
if ($SkipCodex) { $selected = @($selected | Where-Object { $_ -ne 'codex' }) }
if ($SkipOpenCode) { $selected = @($selected | Where-Object { $_ -ne 'opencode' }) }
if ($SkipClaude) { $selected = @($selected | Where-Object { $_ -ne 'claude' }) }
if (-not $selected) { Write-Host 'No clients selected. Nothing was installed.' -ForegroundColor Yellow; exit 0 }
Write-Host "Selected: $($selected -join ', ')" -ForegroundColor Green

Write-Step 'Obtaining the MCP server binary'
$serverSource = Resolve-ServerBinary
if (-not $DryRun -and $serverSource -ne $stableBinary) {
    New-Item -ItemType Directory -Path $InstallRoot -Force | Out-Null
    Copy-Item -LiteralPath $serverSource -Destination $stableBinary -Force
    Write-Host "PASS: MCP server installed at '$stableBinary'." -ForegroundColor Green
}

Write-Step 'Deploying the bundled Everything engine'
if ($DryRun) { Write-Action "Deploy bundled engine into '$(Join-Path $InstallRoot 'everything')'" }
else { Deploy-BundledEngine }

foreach ($client in $selected) {
    switch ($client) {
        'codex' { Install-Codex }
        'opencode' { Install-OpenCode }
        'claude' { Install-Claude }
    }
}

Install-NativeService

$everything = Get-Process -Name Everything -ErrorAction SilentlyContinue
if ($everything) { Write-Host 'PASS: Everything is running.' -ForegroundColor Green }
elseif (Test-Path -LiteralPath (Join-Path $InstallRoot 'everything\Everything.exe')) {
    Write-Host 'PASS: Everything is not running, but the bundled engine will start automatically on first search.' -ForegroundColor Green
} else {
    Write-Host 'WARN: Everything is not running and no bundled engine was deployed.' -ForegroundColor Yellow
}
Write-Host "`nInstalled binary: $stableBinary" -ForegroundColor Green
Write-Host 'Restart selected clients so they reload the MCP configuration.'
Write-Host 'Run .\scripts\doctor.ps1 any time to diagnose the setup.'
