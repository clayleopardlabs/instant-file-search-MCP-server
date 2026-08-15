[CmdletBinding()]
param(
    [ValidateSet('codex', 'opencode', 'claude', 'hermes', 'all')]
    [string[]]$Clients,
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\instant-file-search'),
    [string]$ReleaseBase = 'https://github.com/clayleopardlabs/instant-file-search-MCP-server/releases/latest/download',
    [string]$ServerBinary,
    [string]$IndexerBinary,
    [string]$VendorDir,
    [string]$ExpectedSha256,
    [string]$Version,
    [ValidateSet('memory', 'disk')]
    [string]$IndexMode,
    [ValidateSet('auto', 'off', 'memory', 'disk')]
    [string]$ContentMode,
    [switch]$SkipDownload,
    [switch]$SkipElevation,
    [switch]$SkipCodex,
    [switch]$SkipOpenCode,
    [switch]$SkipClaude,
    [switch]$SkipHermes,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$serverName   = 'instant'
$binaryName = 'instant-file-search-mcp-server.exe'
$indexerName = 'instant-file-search-indexer.exe'
$serviceName = 'instant-file-search-indexer'
$doctorName = 'doctor.ps1'
$repoRoot = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { $null }
$isCheckout = $repoRoot -and (Test-Path -LiteralPath (Join-Path $repoRoot 'Cargo.toml'))
$versionRoot = $null
$stableBinary = $null
$stableIndexer = $null
$installVersion = $null
$downloadRoot = Join-Path (Join-Path $InstallRoot 'downloads') $PID

# Keep an existing choice during upgrades unless the caller supplies a new one.
$statePath = Join-Path $InstallRoot 'current.json'
if ((-not $IndexMode) -and (Test-Path -LiteralPath $statePath)) {
    try { $IndexMode = (Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json).index_mode } catch {}
}
if ((-not $ContentMode) -and (Test-Path -LiteralPath $statePath)) {
    try { $ContentMode = (Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json).content_mode } catch {}
}
if (-not $IndexMode) { $IndexMode = 'memory' }
if (-not $ContentMode) { $ContentMode = 'auto' }
if ($IndexMode -notin @('memory', 'disk')) { throw "IndexMode must be memory or disk." }
if ($ContentMode -notin @('auto', 'off', 'memory', 'disk')) { throw "ContentMode must be auto, off, memory, or disk." }

# Simulation mode: redirect all user-config writes into a sandbox and skip the
# global env-var write, so parallel/simulated installs never touch the real
# opencode config or the user's environment.
$simulateRoot = $env:INSTANT_FS_SIMULATE
if ($simulateRoot) {
    $openCodeConfigDir = Join-Path $simulateRoot '.config\opencode'
    $hermesConfig = Join-Path $simulateRoot 'hermes\config.yaml'
} else {
    $openCodeConfigDir = Join-Path $env:USERPROFILE '.config\opencode'
    $hermesHome = if ($env:HERMES_HOME) { $env:HERMES_HOME } else { Join-Path $env:LOCALAPPDATA 'hermes' }
    $hermesConfig = Join-Path $hermesHome 'config.yaml'
}
$openCodePluginRoot = Join-Path $openCodeConfigDir 'plugins\instant-file-search-mcp-plugin'
$claudeConfig = if ($simulateRoot) { Join-Path $simulateRoot 'claude-desktop-config.json' } else { Join-Path $env:APPDATA 'Claude\claude_desktop_config.json' }

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

function Find-HermesCli {
    $candidates = @()
    if ($env:HERMES_CLI_PATH) { $candidates += $env:HERMES_CLI_PATH }
    $onPath = Get-Command hermes -ErrorAction SilentlyContinue
    if ($onPath -and $onPath.Source) { $candidates += $onPath.Source }
    $candidates += (Join-Path $env:LOCALAPPDATA 'hermes\hermes-agent\venv\Scripts\hermes.exe')
    foreach ($candidate in ($candidates | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        if ($candidate -match '\\WindowsApps\\') { continue }
        return (Get-Item -LiteralPath $candidate)
    }
    return $null
}

function Test-HermesInstalled {
    return (Find-HermesCli) -or (Test-Path -LiteralPath $hermesConfig)
}

function Get-DetectedClients {
    $found = @()
    if ((Get-Command codex -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.codex\config.toml'))) { $found += 'codex' }
    if ((Get-Command opencode -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath $openCodeConfigDir)) { $found += 'opencode' }
    if (Test-ClaudeInstalled) { $found += 'claude' }
    if (Test-HermesInstalled) { $found += 'hermes' }
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
    Write-Host 'Choose clients to configure: [A]ll detected, or enter a comma-separated list: codex, opencode, claude, hermes'
    $answer = Read-Host 'Selection (default A)'
    if ([string]::IsNullOrWhiteSpace($answer)) { return $detected }
    $choice = $answer.Trim().ToLowerInvariant()
    if ($choice -eq 'a') { return $detected }
    if ($choice -eq 'n') { return @() }
    $selected = @($choice -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $invalid = @($selected | Where-Object { $_ -notin @('codex', 'opencode', 'claude', 'hermes') })
    if ($invalid) { throw "Unknown client(s): $($invalid -join ', '). Use codex, opencode, claude, hermes, or all detected." }
    return @($selected | Select-Object -Unique)
}

function Get-ReleaseAsset([string]$Name, [string]$Dest) {
    $url = "$ReleaseBase/$Name"
    Write-Host "   Downloading $Name..." -ForegroundColor Gray
    $parent = Split-Path -Parent $Dest
    if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $attempts = 0
    while ($true) {
        $attempts++
        # Remove any stale partial file from a prior interrupted run.
        if (Test-Path -LiteralPath $Dest) { Remove-Item -LiteralPath $Dest -Force -ErrorAction SilentlyContinue }
        try {
            Invoke-WebRequest -Uri $url -OutFile $Dest -UseBasicParsing -TimeoutSec 60
            break
        } catch {
            Remove-Item -LiteralPath $Dest -Force -ErrorAction SilentlyContinue
            if ($attempts -ge 3) { throw "Download failed for '$Name': $($_.Exception.Message)" }
            Write-Host "   Download attempt $attempts failed; retrying..." -ForegroundColor DarkYellow
            Start-Sleep -Seconds 2
        }
    }
    if (-not (Test-Path -LiteralPath $Dest)) { throw "Download failed: $url" }
    $len = (Get-Item -LiteralPath $Dest).Length
    if ($len -eq 0) {
        Remove-Item -LiteralPath $Dest -Force -ErrorAction SilentlyContinue
        throw "Download of '$Name' produced an empty file; aborting to avoid installing a corrupt asset."
    }
    Write-Host "   Saved $Dest ($([math]::Round($len / 1KB, 1)) KB)" -ForegroundColor Green
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
    New-Item -ItemType Directory -Path $downloadRoot -Force | Out-Null
    $downloaded = Join-Path $downloadRoot $binaryName
    Get-ReleaseAsset $binaryName $downloaded
    if ($ExpectedSha256) {
        $actual = (Get-FileHash -LiteralPath $downloaded -Algorithm SHA256).Hash
        if ($actual -ne $ExpectedSha256.ToUpperInvariant()) {
            Remove-Item -LiteralPath $downloaded -Force
            throw "SHA-256 verification failed for '$binaryName'. Expected '$ExpectedSha256', got '$actual'."
        }
        Write-Host "   SHA-256 verified." -ForegroundColor Green
    }
    return $downloaded
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
    New-Item -ItemType Directory -Path $downloadRoot -Force | Out-Null
    $downloaded = Join-Path $downloadRoot $indexerName
    Get-ReleaseAsset $indexerName $downloaded
    return $downloaded
}

function Get-InstallVersion([string]$ServerPath) {
    if ($Version) { return $Version }
    try {
        $out = (& $ServerPath --version 2>$null | Select-Object -First 1).Trim()
        if ($out -match '^instant-file-search-mcp-server\s+([0-9]+\.[0-9]+\.[0-9]+)') { return $Matches[1] }
    } catch {}
    if ($isCheckout) {
        $cargo = Join-Path $repoRoot 'Cargo.toml'
        $match = Select-String -LiteralPath $cargo -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($match) { return $match.Matches[0].Groups[1].Value }
    }
    return "legacy-$((Get-FileHash -LiteralPath $ServerPath -Algorithm SHA256).Hash.Substring(0, 12).ToLowerInvariant())"
}

function Initialize-VersionedInstall([string]$ServerPath) {
    $script:installVersion = Get-InstallVersion $ServerPath
    $script:versionRoot = Join-Path $InstallRoot (Join-Path 'versions' $script:installVersion)
    $script:stableBinary = Join-Path $script:versionRoot $binaryName
    $script:stableIndexer = Join-Path $script:versionRoot $indexerName
    if ($DryRun) { return }
    New-Item -ItemType Directory -Path $script:versionRoot -Force | Out-Null
}

function Write-InstallState([string]$ServiceBinary) {
    if ($DryRun) { return }
    $state = [ordered]@{
        version = $installVersion
        server_binary = $stableBinary
        indexer_binary = $ServiceBinary
        index_mode = $IndexMode
        content_mode = $ContentMode
        installed_at = (Get-Date).ToUniversalTime().ToString('o')
    } | ConvertTo-Json
    $temp = Join-Path $InstallRoot 'current.json.tmp'
    $state | Set-Content -LiteralPath $temp -Encoding UTF8
    Move-Item -LiteralPath $temp -Destination (Join-Path $InstallRoot 'current.json') -Force
}

function Deploy-BundledEngine {
    $bundleDir = Join-Path $versionRoot 'everything'
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
        Copy-Item -LiteralPath (Join-Path $vendorSource $licenseName) -Destination $versionRoot -Force
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
        Get-ReleaseAsset $licenseName (Join-Path $versionRoot $licenseName)
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

# Write the current MCP registration key over any legacy key (e.g. "everything"
# or "instant-file-search") in raw JSONC text, keeping comments and formatting.
# Existing installs migrate in place instead of accumulating a duplicate entry.
function Migrate-LegacyMcpKeys([string]$Raw) {
    foreach ($legacy in @('everything', 'instant-file-search')) {
        if ($legacy -eq $serverName) { continue }
        $pattern = '"' + [regex]::Escape($legacy) + '"(\s*:)'
        if ($Raw -match $pattern) {
            $Raw = [regex]::Replace($Raw, $pattern, ('"' + $serverName + '"$1'))
        }
    }
    return $Raw
}

# Add the MCP server entry to an opencode config while preserving the file's
# comments and formatting. If the entry already exists with the same command,
# the file is left untouched (no churn, no comment loss on reinstall).
#
# Returns $true if the file was written or was already up to date.
function Write-McpConfigPreserving([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        $cfg = [pscustomobject]@{ mcp = [pscustomobject]@{ $serverName = [pscustomobject]@{ type = 'local'; command = @($stableBinary); enabled = $true } } }
        Write-JsonConfig $Path $cfg
        return $true
    }

    $raw = Get-Content -LiteralPath $Path -Raw
    $migrated = Migrate-LegacyMcpKeys $raw
    if ($migrated -ne $raw) {
        # A legacy registration key exists. Rewrite the file with the current
        # key so the migration persists, then continue to refresh the entry.
        Backup-Config $Path
        Set-Content -LiteralPath $Path -Value $migrated -Encoding UTF8
        Write-Host "   MCP entry migrated to '$serverName' in '$Path'." -ForegroundColor Gray
        $raw = $migrated
    }
    $config = Read-JsonConfig $Path
    if ($null -eq $config) { return $false }

    $mcp = $config.PSObject.Properties['mcp'].Value
    $existing = $null
    if ($mcp) { $existing = $mcp.PSObject.Properties[$serverName].Value }
    if ($existing) {
        $cmd = @($existing.command)
        $entryType = $existing.PSObject.Properties['type'].Value
        if ($cmd -contains $stableBinary -and $entryType -eq 'local') {
            Write-Host "   MCP entry '$serverName' already present and current; leaving config untouched." -ForegroundColor Gray
            return $true
        }
    }

    # The entry is stale (points at an old binary path, or is missing the
    # required "type": "local") - replace it cleanly via the object model
    # rather than appending a duplicate.
    if ($existing) {
        Write-Host "   MCP entry '$serverName' points at an old path; updating it." -ForegroundColor Gray
        $cfg2 = $config
        Ensure-Property $mcp $serverName ([pscustomobject]@{ type = 'local'; command = @($stableBinary); enabled = $true })
        Write-JsonConfig $Path $cfg2
        return $true
    }

    # The entry is missing. Try a surgical, comment-preserving insert into the
    # top-level "mcp" object; fall back to a full rewrite otherwise.
    try {
        $inserted = Insert-McpEntryText $raw $stableBinary
        if ($inserted) {
            Backup-Config $Path
            Set-Content -LiteralPath $Path -Value $inserted -Encoding UTF8
            Write-Host "   MCP entry inserted into '$Path' (comments preserved)." -ForegroundColor Gray
            return $true
        }
    } catch { /* fall through to rewrite */ }

    Write-Host "   Config has comments we cannot preserve automatically; backing up and rewriting as plain JSON." -ForegroundColor DarkGray
    $cfg2 = $config
    if (-not $mcp) { $mcp = [pscustomobject]@{}; Ensure-Property $cfg2 'mcp' $mcp }
    Ensure-Property $mcp $serverName ([pscustomobject]@{ type = 'local'; command = @($stableBinary); enabled = $true })
    Write-JsonConfig $Path $cfg2
    return $true
}

# Insert the MCP entry into the top-level "mcp" block of a JSONC file by text,
# preserving all comments/formatting. Returns the new text, or $null if it
# cannot be done safely.
function Insert-McpEntryText([string]$Raw, [string]$BinaryPath) {
    # Find the top-level "mcp" key and the braces of its value object.
    $depth = 0; $inStr = $false; $inStr2 = $false
    $mcpStart = -1; $mcpEnd = -1; $braceStart = -1
    $esc = $false
    $n = $Raw.Length
    $i = 0
    # locate the "mcp" key at any depth (top-level is what we need, but jsonc is
    # typically top-level) - walk tokenizing
    $needle = '"mcp"'
    while ($i -lt $n) {
        $c = $Raw[$i]
        $next = if ($i + 1 -lt $n) { $Raw[$i + 1] } else { '' }
        if ($inStr) {
            if ($esc) { $esc = $false }
            elseif ($c -eq '\') { $esc = $true }
            elseif ($c -eq '"') { $inStr = $false }
        } elseif ($c -eq '"') { $inStr = $true }
        elseif ($c -eq '/' -and $next -eq '/') { while ($i -lt $n -and $Raw[$i] -ne "`n") { $i++ } }
        elseif ($c -eq '/' -and $next -eq '*') { $i += 2; while ($i -lt $n -and -not ($Raw[$i] -eq '*' -and $i + 1 -lt $n -and $Raw[$i + 1] -eq '/')) { $i++ }; if ($i -lt $n) { $i += 2 } }
        else {
            if ($Raw.Substring($i) -match '^\s*"mcp"') {
                $mcpStart = $i
                # find the value: skip to the opening brace
                $j = $i
                while ($j -lt $n) {
                    $cj = $Raw[$j]
                    if ($cj -eq '{') { $braceStart = $j; break }
                    $j++
                }
                break
            }
        }
        $i++
    }
    if ($braceStart -lt 0) { return $null }

    # find matching close brace (string/comment aware)
    $k = $braceStart; $d = 0; $inStr = $false; $esc = $false
    while ($k -lt $n) {
        $c = $Raw[$k]
        $next = if ($k + 1 -lt $n) { $Raw[$k + 1] } else { '' }
        if ($inStr) { if ($esc) { $esc = $false } elseif ($c -eq '\') { $esc = $true } elseif ($c -eq '"') { $inStr = $false } }
        elseif ($c -eq '"') { $inStr = $true }
        elseif ($c -eq '/' -and $next -eq '/') { while ($k -lt $n -and $Raw[$k] -ne "`n") { $k++ } }
        elseif ($c -eq '/' -and $next -eq '*') { $k += 2; while ($k -lt $n -and -not ($Raw[$k] -eq '*' -and $k + 1 -lt $n -and $Raw[$k + 1] -eq '/')) { $k++ }; if ($k -lt $n) { $k += 2 } }
        else {
            if ($c -eq '{') { $d++ }
            elseif ($c -eq '}') { $d--; if ($d -eq 0) { $mcpEnd = $k; break } }
        }
        $k++
    }
    if ($mcpEnd -lt 0) { return $null }

    $inner = $Raw.Substring($braceStart + 1, $mcpEnd - $braceStart - 1)
    $entry = "`n    `"$serverName`": {`n      `"type`": `"local`",`n      `"command`": [`"$BinaryPath`"],`n      `"enabled`": true`n    }`n  "
    if ([string]::IsNullOrWhiteSpace($inner)) {
        $newInner = $entry
    } else {
        $newInner = $inner.TrimEnd() + "," + $entry
    }
    return $Raw.Substring(0, $braceStart + 1) + $newInner + $Raw.Substring($mcpEnd)
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
        if ($keys.Count -eq 1 -and ($keys[0] -eq $serverName -or $keys[0] -eq 'everything' -or $keys[0] -eq 'instant-file-search')) {
            Backup-Config $json
            Remove-Item -LiteralPath $json -Force
            Write-Host "Removed orphaned config '$json' (its only entry was '$serverName', now merged into '$jsonc')." -ForegroundColor Gray
        }
    } catch { /* leave it alone */ }
}

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

function Install-Codex {
    Write-Step 'Configuring Codex'
    $codex = Find-CodexCli
    if (-not $codex) { Write-Host 'WARN: codex.exe was not found on PATH; skipping Codex registration.' -ForegroundColor Yellow; return }
    if ($DryRun) { Write-Action "codex mcp add $serverName -- '$stableBinary'"; return }

    $listOutput = (& $codex.FullName mcp list 2>&1 | Out-String)
    # The entry may be registered under the current name or a legacy name from
    # an older installer. Resolve the actual registered name (legacy first, so
    # the current name does not match as a prefix of "instant-file-search")
    # and remove that exact entry, then add the current name.
    $registeredName = $null
    if ($listOutput -match '(?im)\binstant-file-search\b') { $registeredName = 'instant-file-search' }
    elseif ($listOutput -match '(?im)\beverything\b') { $registeredName = 'everything' }
    elseif ($listOutput -match ('(?im)\b' + [regex]::Escape($serverName) + '\b')) { $registeredName = $serverName }
    if ($registeredName) {
        $helpOutput = (& $codex.FullName mcp --help 2>&1 | Out-String)
        if ($helpOutput -match '(?im)\bremove\b') {
            & $codex.FullName mcp remove $registeredName
            if ($LASTEXITCODE -ne 0) { throw "Could not replace existing Codex MCP server '$registeredName'." }
            & $codex.FullName mcp add $serverName -- $stableBinary
        } else {
            Write-Host "Codex server '$serverName' already exists (as '$registeredName'); leaving it unchanged because this Codex version lacks 'mcp remove'." -ForegroundColor Yellow
            return
        }
    } else {
        & $codex.FullName mcp add $serverName -- $stableBinary
    }
    if ($LASTEXITCODE -ne 0) { throw 'Codex MCP registration failed.' }
    Write-Host 'PASS: Codex MCP server registered.' -ForegroundColor Green
}

# Hermes stores MCP servers in YAML under mcp_servers.  Its `mcp add` command
# performs interactive discovery and tool selection, so the installer writes
# the documented stdio entry directly and leaves probing to Hermes at startup.
function Get-HermesMcpEntryLines([string]$BinaryPath) {
    $quoted = $BinaryPath.Replace("'", "''")
    return @(
        "  ${serverName}:",
        "    command: '$quoted'",
        '    enabled: true'
    )
}

function Write-HermesConfig {
    $desired = @(Get-HermesMcpEntryLines $stableBinary)
    $newline = if (Test-Path -LiteralPath $hermesConfig) {
        $existingRaw = Get-Content -LiteralPath $hermesConfig -Raw
        if ($existingRaw.Contains("`r`n")) { "`r`n" } else { "`n" }
    } else { "`r`n" }

    $raw = if (Test-Path -LiteralPath $hermesConfig) { Get-Content -LiteralPath $hermesConfig -Raw } else { '' }
    $lines = if ($raw) { @($raw -split "`r?`n") } else { @() }
    $mcpIndex = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^mcp_servers\s*:\s*(?:#.*)?$') { $mcpIndex = $i; break }
    }

    $changed = $false
    # Migrate a legacy entry key in place (never leave a duplicate entry).
    for ($i = 0; $i -lt $lines.Count; $i++) {
        foreach ($legacy in @('everything', 'instant-file-search')) {
            if ($legacy -eq $serverName) { continue }
            if ($lines[$i] -match ('^\s+' + [regex]::Escape($legacy) + '\s*:')) {
                $lines[$i] = $lines[$i] -replace [regex]::Escape($legacy), $serverName
                $changed = $true
                Write-Host "   Hermes MCP entry migrated from legacy key '$legacy' to '$serverName'." -ForegroundColor Gray
                break
            }
        }
    }
    if ($mcpIndex -lt 0) {
        $prefix = if ($raw -and -not $raw.EndsWith("`n")) { @('') } else { @() }
        $lines = @($lines + $prefix + @('mcp_servers:') + $desired + @(''))
        $changed = $true
    } elseif ($lines[$mcpIndex] -match '^mcp_servers\s*:\s*\S+') {
        # Replace an inline map (for example `mcp_servers: {}`) with the
        # documented block form so the entry remains readable and stable.
        $before = if ($mcpIndex -gt 0) { @($lines[0..($mcpIndex - 1)]) } else { @() }
        $after = if ($mcpIndex + 1 -lt $lines.Count) { @($lines[($mcpIndex + 1)..($lines.Count - 1)]) } else { @() }
        $lines = @($before + @('mcp_servers:') + $desired + $after)
        $changed = $true
    } else {
        $sectionEnd = $lines.Count
        for ($i = $mcpIndex + 1; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -match '^\S' -and $lines[$i] -notmatch '^\s*(#|$)') { $sectionEnd = $i; break }
        }

        $entryStart = -1
        for ($i = $mcpIndex + 1; $i -lt $sectionEnd; $i++) {
            if ($lines[$i] -match '^  ' + [regex]::Escape($serverName) + '\s*:') { $entryStart = $i; break }
        }

        if ($entryStart -ge 0) {
            $entryEnd = $sectionEnd
            for ($i = $entryStart + 1; $i -lt $sectionEnd; $i++) {
                if ($lines[$i] -match '^  \S' -and $lines[$i] -notmatch '^    ') { $entryEnd = $i; break }
            }
            $currentEntry = (@($lines[$entryStart..($entryEnd - 1)]) -join $newline).TrimEnd()
            $desiredEntry = $desired -join $newline
            if ($currentEntry -ne $desiredEntry) {
                $before = @($lines[0..($entryStart - 1)])
                $after = if ($entryEnd -lt $lines.Count) { @($lines[$entryEnd..($lines.Count - 1)]) } else { @() }
                $lines = @($before + $desired + $after)
                $changed = $true
            }
        } else {
            $before = if ($sectionEnd -gt 0) { @($lines[0..($sectionEnd - 1)]) } else { @() }
            $after = if ($sectionEnd -lt $lines.Count) { @($lines[$sectionEnd..($lines.Count - 1)]) } else { @() }
            $lines = @($before + $desired + $after)
            $changed = $true
        }
    }

    if ($changed) {
        $parent = Split-Path -Parent $hermesConfig
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
        Backup-Config $hermesConfig
        [System.IO.File]::WriteAllText($hermesConfig, ($lines -join $newline), [System.Text.UTF8Encoding]::new($false))
    }
    return $true
}

function Install-Hermes {
    Write-Step 'Configuring Hermes'
    if ($DryRun) { Write-Action "Add MCP server '$serverName' to '$hermesConfig'"; return }
    if (-not (Test-HermesInstalled)) {
        Write-Host 'WARN: Hermes was not found; skipping Hermes registration.' -ForegroundColor Yellow
        return
    }
    if (Write-HermesConfig) {
        Write-Host "PASS: Hermes MCP server configured in '$hermesConfig'." -ForegroundColor Green
    }
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
        Install-OmoMcpAccess
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
    } elseif (-not $npm) {
        Write-Host "WARN: npm was not found on PATH, so the OpenCode plugin dependencies could not be installed. The MCP server still works for the main session; sub-agents need node/npm + 'npm ci' in '$openCodePluginRoot'." -ForegroundColor Yellow
    }

    if ($simulateRoot) {
        Write-Host "   (simulation) skipping global INSTANT_FS_MCP_BINARY env write." -ForegroundColor Gray
    } else {
        [Environment]::SetEnvironmentVariable('INSTANT_FS_MCP_BINARY', $stableBinary, 'User')
    }
    $ok = Write-McpConfigPreserving $openCodeConfig
    if ($ok) { Write-Host "PASS: OpenCode MCP server '$serverName' configured in '$openCodeConfig'." -ForegroundColor Green }
    else { Write-Host "WARN: could not update '$openCodeConfig'. Add the MCP entry manually - see README \"Set up a single app yourself\"." -ForegroundColor Yellow }
    Remove-OrphanOpenCodeJson $openCodeConfig

    # Ensure oh-my-opencode-slim sub-agents can see the search tools.
    Install-OmoMcpAccess
}

function Install-Claude {
    Write-Step 'Configuring Claude Desktop'
    if ($DryRun) { Write-Action "Add MCP server '$serverName' to '$claudeConfig'"; return }
    $config = Read-JsonConfig $claudeConfig
    $servers = $config.PSObject.Properties['mcpServers'].Value
    if (-not $servers) { $servers = [pscustomobject]@{}; Ensure-Property $config 'mcpServers' $servers }
    # Migrate a legacy entry key in place (never leave a duplicate entry).
    foreach ($legacy in @('everything', 'instant-file-search')) {
        if ($legacy -eq $serverName) { continue }
        if ($servers.PSObject.Properties[$legacy]) {
            $servers.PSObject.Properties.Remove($legacy)
            Write-Host "   Claude Desktop entry migrated from legacy key '$legacy' to '$serverName'." -ForegroundColor Gray
        }
    }
    Ensure-Property $servers $serverName ([pscustomobject]@{ command = $stableBinary; args = @() })
    Write-JsonConfig $claudeConfig $config
    Write-Host "PASS: Claude Desktop MCP server configured in '$claudeConfig'." -ForegroundColor Green
}

# Detect oh-my-opencode-slim (OMO) and add instant-file-search to every
# sub-agent's mcps array so the search tools are visible to subagents.
# Orchestrators that already have mcps: ["*"] are left untouched.
function Install-OmoMcpAccess {
    Write-Step 'Configuring oh-my-opencode-slim sub-agent MCP access'
    $omoConfig = Join-Path $openCodeConfigDir 'oh-my-opencode-slim.json'
    $omoConfigJsonc = Join-Path $openCodeConfigDir 'oh-my-opencode-slim.jsonc'
    $omoPath = if (Test-Path -LiteralPath $omoConfigJsonc) { $omoConfigJsonc }
              elseif (Test-Path -LiteralPath $omoConfig) { $omoConfig }
              else { $null }

    if (-not $omoPath) {
        Write-Host '   oh-my-opencode-slim config not found; skipping.' -ForegroundColor Gray
        return
    }

    if ($DryRun) {
        Write-Action "Add '$serverName' to sub-agent mcps in '$omoPath'"
        return
    }

    try {
        $config = Read-JsonConfig $omoPath
    } catch {
        Write-Host "WARN: could not parse '$omoPath'; skipping OMO configuration." -ForegroundColor Yellow
        return
    }

    Backup-Config $omoPath
    $changed = $false
    $mcpEntry = $serverName

    # OMO config has a top-level 'presets' object, each containing agent
    # definitions with optional 'mcps' arrays.  We iterate every preset and
    # every agent, skipping orchestrators (they typically have ["*"]).
    $presets = $config.PSObject.Properties['presets'].Value
    if (-not $presets) {
        Write-Host '   No presets found in OMO config; skipping.' -ForegroundColor Gray
        return
    }

    foreach ($presetName in $presets.PSObject.Properties.Name) {
        $preset = $presets.PSObject.Properties[$presetName].Value
        if (-not $preset) { continue }

        foreach ($agentName in $preset.PSObject.Properties.Name) {
            $agent = $preset.PSObject.Properties[$agentName].Value
            if (-not $agent -or -not ($agent.PSObject.Properties['mcps'])) { continue }

            $mcps = @($agent.mcps)

            # Skip agents with wildcard mcps (orchestrators like ["*"])
            if ($mcps -contains '*') { continue }

            # Already present? Nothing to do.
            if ($mcps -contains $mcpEntry) {
                Write-Host "   preset '$presetName' / $agentName`: already has '$mcpEntry'" -ForegroundColor Gray
                continue
            }

            # Migrate a legacy mcps entry in place (never leave a duplicate).
            if ($mcps -contains 'instant-file-search' -or $mcps -contains 'everything') {
                $newMcps = @($mcps | Where-Object { $_ -ne 'instant-file-search' -and $_ -ne 'everything' }) + @($mcpEntry)
                $agent.mcps = $newMcps
                $changed = $true
                Write-Host "   preset '$presetName' / $agentName`: migrated '$mcpEntry' in mcps" -ForegroundColor Green
                continue
            }

            # Add the entry.
            $newMcps = @($mcps) + @($mcpEntry)
            $agent.mcps = $newMcps
            $changed = $true
            Write-Host "   preset '$presetName' / $agentName`: added '$mcpEntry' to mcps" -ForegroundColor Green
        }
    }

    if ($changed) {
        # Write back as JSON (ConvertTo-Json).  Use File.WriteAllText with
        # UTF-8 no-BOM to avoid the BOM that PowerShell 5.1 Set-Content adds,
        # which would break JSONC parsers on re-read.
        $json = $config | ConvertTo-Json -Depth 20
        $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
        [System.IO.File]::WriteAllText($omoPath, $json, $utf8NoBom)
        Write-Host "PASS: OMO sub-agent MCP access configured." -ForegroundColor Green
    } else {
        Write-Host "   All sub-agents already have $serverName; no changes needed." -ForegroundColor Gray
    }
}

function Install-NativeService {
    Write-Step 'Installing the native indexer service'
    $indexerDir = Join-Path $versionRoot 'indexer'
    $serviceIndexer = Join-Path $indexerDir $indexerName
    $registerHelper = Join-Path $indexerDir 'register-indexer-service.ps1'

    $resolved = Resolve-IndexerBinary
    if (-not $resolved) {
        Write-Host 'WARN: no indexer binary available; the native engine will not be installed. Searches will use the Fallback Engine.' -ForegroundColor Yellow
        return
    }

    if ($DryRun) {
        Write-Action "Configure service environment: INSTANT_FS_INDEX_MODE=$IndexMode, INSTANT_FS_CONTENT_INDEX=$ContentMode"
        Write-Action "Copy '$resolved' to '$serviceIndexer'"
        Write-Action "sc.exe create $serviceName binPath= `"$serviceIndexer service`" start= auto"
        return
    }

    New-Item -ItemType Directory -Path $indexerDir -Force | Out-Null

    # Escape single quotes in paths so the generated helper survives usernames
    # or install roots that contain an apostrophe.
    $escResolved = $resolved -replace "'", "''"
    $escServiceIndexer = $serviceIndexer -replace "'", "''"
    $escServiceName = $serviceName -replace "'", "''"
    $escIndexMode = $IndexMode -replace "'", "''"
    $escContentMode = $ContentMode -replace "'", "''"

# Write a tiny helper that performs the admin-only switch. The binary was
# copied after the service stops, so updates can replace an existing binary.
# Use cmd.exe for sc.exe because PowerShell strips nested quotes from native
# command arguments.
$helper = @"
`$ErrorActionPreference = 'Stop'
`$sourceIndexer = '$escResolved'
`$serviceName = '$escServiceName'
`$serviceIndexer = '$escServiceIndexer'
`$indexMode = '$escIndexMode'
`$contentMode = '$escContentMode'
`$serviceCommand = 'sc.exe config "' + `$serviceName + '" binPath= "\"' + `$serviceIndexer + '\" service" start= auto'
`$existing = Get-Service -Name `$serviceName -ErrorAction SilentlyContinue
if (`$existing) {
  if (`$existing.Status -ne 'Stopped') {
    Stop-Service -Name `$serviceName -Force
    `$existing.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Stopped, [TimeSpan]::FromSeconds(30))
  }
  Copy-Item -LiteralPath `$sourceIndexer -Destination `$serviceIndexer -Force
  & cmd.exe /d /c `$serviceCommand | Out-Null
  if (`$LASTEXITCODE -ne 0) { throw "sc.exe config failed for service `$serviceName." }
} else {
  Copy-Item -LiteralPath `$sourceIndexer -Destination `$serviceIndexer -Force
  `$serviceCommand = 'sc.exe create "' + `$serviceName + '" binPath= "\"' + `$serviceIndexer + '\" service" start= auto'
  & cmd.exe /d /c `$serviceCommand | Out-Null
  if (`$LASTEXITCODE -ne 0) { throw "sc.exe create failed for service `$serviceName." }
}
`$environment = @("INSTANT_FS_INDEX_MODE=`$indexMode", "INSTANT_FS_CONTENT_INDEX=`$contentMode")
`$serviceKey = "HKLM:\SYSTEM\CurrentControlSet\Services\`$serviceName"
`$oldEnvironment = @((Get-ItemProperty -Path `$serviceKey -Name Environment -ErrorAction SilentlyContinue).Environment)
`$preserved = @(`$oldEnvironment | Where-Object { `$_ -and `$_ -notmatch '^INSTANT_FS_(INDEX_MODE|CONTENT_INDEX)=' })
New-ItemProperty -Path `$serviceKey -Name Environment -PropertyType MultiString -Value (@(`$preserved + `$environment)) -Force | Out-Null
Start-Service -Name `$serviceName
`$started = Get-Service -Name `$serviceName
`$started.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(120))
"@
    Set-Content -LiteralPath $registerHelper -Value $helper -Encoding UTF8

    if ($elevated) {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $registerHelper
        if ($LASTEXITCODE -ne 0) { throw "Indexer service registration failed (exit $LASTEXITCODE)." }
        $svc = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
        if ($svc -and $svc.Status -eq 'Running') {
            Write-Host "PASS: indexer service '$serviceName' installed and started (auto-start)." -ForegroundColor Green
        } else {
            throw "Indexer service '$serviceName' was registered but is not running."
        }
        return
    }

    if ($SkipElevation) {
        Write-Host 'WARN: not running elevated and -SkipElevation was set, so the indexer service was NOT registered. Searches will use the Fallback Engine until you register it. When you are ready, run this elevated:' -ForegroundColor Yellow
        Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$registerHelper`"" -ForegroundColor Gray
        return
    }

    Write-Host 'The native indexer service needs administrator rights. A UAC prompt will appear - click Yes.' -ForegroundColor Yellow
    try {
        $elevatedProcess = Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$registerHelper`"" -Wait -PassThru -ErrorAction Stop
        if ($elevatedProcess.ExitCode -ne 0) {
            throw "elevated service helper exited with code $($elevatedProcess.ExitCode)."
        }
        $deadline = (Get-Date).AddSeconds(120)
        do {
            $svc = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
            if ($svc -and $svc.Status -eq 'Running') { break }
            Start-Sleep -Milliseconds 500
        } while ((Get-Date) -lt $deadline)
        if ($svc -and $svc.Status -eq 'Running') {
            Write-Host "PASS: indexer service '$serviceName' installed and started (auto-start)." -ForegroundColor Green
        } else {
            Write-Host "WARN: elevation did not leave the '$serviceName' service running. Run the helper elevated manually:" -ForegroundColor Yellow
            Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$registerHelper`"" -ForegroundColor Gray
        }
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
    try {
        $proc = Start-Process -FilePath $stableBinary -RedirectStandardInput $inFile -RedirectStandardOutput $outFile -PassThru -NoNewWindow -ErrorAction Stop
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
    } catch {
        Write-Host "WARN: could not start the MCP server for verification ($($_.Exception.Message))." -ForegroundColor Yellow
        $ok = $false
    }
    Remove-Item $inFile, $outFile -Force -ErrorAction SilentlyContinue
    return $ok
}

Write-Host 'Instant File Search MCP installer' -ForegroundColor Green
Write-Host "Install root: $InstallRoot" -ForegroundColor Gray
if ($isCheckout) { Write-Host 'Source: local checkout' -ForegroundColor Gray }
else { Write-Host "Source: GitHub release ($ReleaseBase)" -ForegroundColor Gray }

$elevated = Test-Elevated

$selected = @(Select-InstallClients)
if ($SkipCodex) { $selected = @($selected | Where-Object { $_ -ne 'codex' }) }
if ($SkipOpenCode) { $selected = @($selected | Where-Object { $_ -ne 'opencode' }) }
if ($SkipClaude) { $selected = @($selected | Where-Object { $_ -ne 'claude' }) }
if ($SkipHermes) { $selected = @($selected | Where-Object { $_ -ne 'hermes' }) }

# Always install the server, engines, and diagnostics even when no supported
# MCP client was detected - a user may configure any MCP host manually.
if (-not $selected) {
    Write-Host 'No MCP clients detected (Codex, OpenCode, Claude Desktop, or Hermes).' -ForegroundColor Yellow
    Write-Host 'The MCP server and engines will still be installed so you can configure any host manually.' -ForegroundColor Yellow
} else {
    Write-Host "Selected: $($selected -join ', ')" -ForegroundColor Green
}

Write-Step 'Obtaining the MCP server binary'
$serverSource = Resolve-ServerBinary
Initialize-VersionedInstall $serverSource
if (-not $DryRun -and $serverSource -ne $stableBinary) {
    Copy-Item -LiteralPath $serverSource -Destination $stableBinary -Force
    Write-Host "PASS: MCP server $installVersion installed at '$stableBinary'." -ForegroundColor Green
}

Write-Step 'Deploying the Fallback Engine'
if ($DryRun) { Write-Action "Deploy Fallback Engine into '$(Join-Path $versionRoot 'everything')'" }
else { Deploy-BundledEngine }

foreach ($client in $selected) {
    switch ($client) {
        'codex' { Install-Codex }
        'opencode' { Install-OpenCode }
        'claude' { Install-Claude }
        'hermes' { Install-Hermes }
    }
}

Install-NativeService
Write-InstallState (Join-Path (Join-Path $versionRoot 'indexer') $indexerName)
Install-Doctor

$everything = Get-Process -Name Everything -ErrorAction SilentlyContinue
if ($everything) { Write-Host 'PASS: Fallback Engine is running.' -ForegroundColor Green }
elseif (Test-Path -LiteralPath (Join-Path $versionRoot 'everything\Everything.exe')) {
    Write-Host 'PASS: Fallback Engine is not running, but the bundled engine will start automatically on first search.' -ForegroundColor Green
} else {
    Write-Host 'WARN: Fallback Engine is not running and no bundled engine was deployed.' -ForegroundColor Yellow
}

$verifyOk = $true
if (-not $DryRun) { $verifyOk = Test-Installation }

Write-Host "`nInstalled version: $installVersion" -ForegroundColor Green
Write-Host "Installed binary:  $stableBinary" -ForegroundColor Green
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

if (-not $verifyOk) {
    Write-Host "`nThe installer finished, but the post-install verification failed. Run the diagnostics script for details:" -ForegroundColor Red
    Write-Host "  $((Join-Path $InstallRoot $doctorName))" -ForegroundColor Gray
    exit 1
}
exit 0
