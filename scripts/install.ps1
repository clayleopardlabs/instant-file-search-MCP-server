[CmdletBinding()]
param(
    [ValidateSet('codex', 'opencode', 'claude', 'hermes', 'dsh', 'all')]
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
    # The Fallback Engine (Everything) is an optional add-on. The native indexer
    # is the primary engine, so bundling a second one is only worth it for parity
    # testing or as a safety net, and it adds an MIT-licensed third-party binary
    # to every install.
    [switch]$IncludeFallbackEngine,
    [switch]$SkipCodex,
    [switch]$SkipOpenCode,
    [switch]$SkipClaude,
    [switch]$SkipHermes,
    [switch]$SkipDsh,
    [string]$DshProfile,
    [string[]]$DshPresets,
    [switch]$SelfTest,
    [switch]$NonInteractive,
    [switch]$Uninstall,
    [switch]$PruneOldVersions,
    [int]$NativeWaitSec = 180,
    [int]$ElevationTimeoutSec = 120,
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

# Set when the install is functionally complete but a step still needs attention
# (for example the native indexer service needs an elevated run).
$incomplete = $false

# Keep an existing choice during upgrades unless the caller supplies a new one.
$statePath = Join-Path $InstallRoot 'current.json'
if ((-not $IndexMode) -and (Test-Path -LiteralPath $statePath)) {
    try { $IndexMode = (Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json).index_mode } catch {}
}
if ((-not $ContentMode) -and (Test-Path -LiteralPath $statePath)) {
    try { $ContentMode = (Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json).content_mode } catch {}
}
if (-not $IndexMode) { $IndexMode = 'disk' }
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

# Read/write user text files through .NET with an explicit UTF-8 decode. Windows
# PowerShell 5.1 falls back to the ANSI code page for Get-Content -Raw when a
# file has no BOM, which silently corrupts non-ASCII text (the Cordis preset
# patches contain em dashes and CJK) on the way back out. Read once here and
# write with the same BOM state the file had.
function Read-TextFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        return [System.Text.Encoding]::UTF8.GetString($bytes, 3, $bytes.Length - 3)
    }
    return [System.Text.Encoding]::UTF8.GetString($bytes)
}

function Test-TextFileHasBom([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    return ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
}

function Write-TextFile([string]$Path, [string]$Text, [bool]$WithBom = $false) {
    $encoding = New-Object System.Text.UTF8Encoding($WithBom)
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}
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

# --- DeepSeek Harness -------------------------------------------------------
# DSH composes one Cordis plugin tree per profile; the user-owned layer is
# `cordis.patch.yml` under <DSH_HOME>\profiles\<profile>\. The file is a flat
# top-level YAML array: `- insert:` entries carry rows, and
# `@deepseek-ai/dsh-agent-preset` entries carry a `plugins:` list that is a
# separate agent scope. A row registering our MCP client can therefore live at
# the root (directly under a top-level insert entry) or inside a preset, and a
# profile may contain many rows for the same server name.
#
# This section parses that shape with line arithmetic rather than a YAML parser,
# because the one-liner install ships only this file. Every edit stays
# scope-correct: all existing rows are repointed to the installed binary, a
# missing root row is restored, and preset rows are only added on request.
# The running harness hot-applies this file, so a successful edit does not need
# a restart; `mcp__instant__*` tools appear on the next agent turn.

function Get-DshHome {
    if ($env:DSH_HOME) { return $env:DSH_HOME }
    return (Join-Path $env:USERPROFILE '.dsh')
}

function Get-DshProfileCandidates {
    $profilesRoot = Join-Path (Get-DshHome) 'profiles'
    if (-not (Test-Path -LiteralPath $profilesRoot)) { return @() }
    return @(Get-ChildItem -LiteralPath $profilesRoot -Directory -ErrorAction SilentlyContinue |
        ForEach-Object {
            $candidatePatch = Join-Path $_.FullName 'cordis.patch.yml'
            [pscustomobject]@{
                Name     = $_.Name
                Dir      = $_.FullName
                Patch    = $candidatePatch
                HasPatch = (Test-Path -LiteralPath $candidatePatch)
            }
        })
}

function Find-RunningDshProfile {
    # The command line of a running host is the best hint when the caller did not
    # inherit DSH_PROFILE (a plain PowerShell, or an agent shell started outside
    # DSH). Only an explicit --profile flag can be trusted here.
    foreach ($proc in @(Get-CimInstance Win32_Process -Filter "Name='node.exe'" -ErrorAction SilentlyContinue)) {
        $cmd = $proc.CommandLine
        if (-not $cmd) { continue }
        if ($cmd -notmatch '[\\/]dsh[\\/]' -or $cmd -notmatch 'bin\.js') { continue }
        if ($cmd -match '--profile[\s=]+["'']?(?<profile>[A-Za-z0-9._-]+)') { return $Matches['profile'] }
    }
    return $null
}

function Test-DshHostRunning {
    foreach ($proc in @(Get-CimInstance Win32_Process -Filter "Name='node.exe'" -ErrorAction SilentlyContinue)) {
        $cmd = $proc.CommandLine
        if ($cmd -and $cmd -match '[\\/]dsh[\\/]' -and $cmd -match 'bin\.js') { return $true }
    }
    return $false
}

# Resolve the one profile to edit. Never guess between several profiles: editing
# the wrong profile looks like a successful install and silently configures an
# app the user did not select.
function Resolve-DshProfile {
    param([string]$Profile)

    $candidates = @(Get-DshProfileCandidates)
    if ($candidates.Count -eq 0) { return $null }

    $requested = $Profile
    $source = '-DshProfile'
    if (-not $requested) {
        $requested = $env:DSH_PROFILE
        $source = 'DSH_PROFILE'
    }
    if (-not $requested) {
        $requested = Find-RunningDshProfile
        $source = 'the running harness'
    }

    if ($requested) {
        $matches = @($candidates | Where-Object { $_.Name -eq $requested })
        if ($matches.Count -eq 0) {
            $known = @($candidates | Where-Object HasPatch | ForEach-Object Name) -join ', '
            throw ("DeepSeek Harness profile '$requested' (from $source) was not found. " +
                "Profiles with a cordis.patch.yml: $known. Pass -DshProfile <name>.")
        }
        if (-not $matches[0].HasPatch) {
            throw "DeepSeek Harness profile '$requested' exists but has no cordis.patch.yml, so there is nothing to edit."
        }
        return $matches[0]
    }

    $withPatch = @($candidates | Where-Object HasPatch)
    if ($withPatch.Count -eq 1) { return $withPatch[0] }
    if ($withPatch.Count -eq 0) { return $null }
    throw ("Multiple DeepSeek Harness profiles were found (" + (($withPatch | ForEach-Object Name) -join ', ') +
        "). Set DSH_PROFILE or pass -DshProfile <name> so the installer does not edit the wrong profile.")
}

function Test-DshInstalled {
    return (@(Get-DshProfileCandidates | Where-Object HasPatch).Count -gt 0)
}

function Get-DshBinaryPath {
    return ($stableBinary -replace '\\', '/')
}

# Parse the patch into top-level entries, rows, presets, and our server rows.
# A row is any `- id:` line below a top-level entry. Its range ends at the next
# row with the same or a lower indent, so nested plugin rows stay inside their
# parent row.
# Keep each line's own terminator: Cordis patches written by different tools are
# often mixed LF/CRLF, and normalising them would rewrite the whole file.
function Split-TextIntoLines {
    param([string]$Text)
    $parts = [regex]::Split($Text, "(\r?\n)")
    $lineList = [System.Collections.Generic.List[string]]::new()
    $endingList = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -lt $parts.Count; $i += 2) {
        $lineList.Add($parts[$i])
        if ($i + 1 -lt $parts.Count) { $endingList.Add($parts[$i + 1]) } else { $endingList.Add('') }
    }
    return [pscustomobject]@{ Lines = [string[]]$lineList; Endings = [string[]]$endingList }
}

function Join-TextFromLines {
    param([string[]]$Lines, [string[]]$Endings)
    $sb = New-Object System.Text.StringBuilder
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        [void]$sb.Append($Lines[$i])
        $ending = if ($i -lt $Endings.Count) { $Endings[$i] } else { '' }
        [void]$sb.Append($ending)
    }
    return $sb.ToString()
}

function Get-DominantNewline {
    param([string[]]$Endings)
    $crlf = 0
    $lf = 0
    foreach ($ending in $Endings) {
        if ($ending -eq "`r`n") { $crlf++ }
        elseif ($ending -eq "`n") { $lf++ }
    }
    if ($lf -gt $crlf) { return "`n" }
    return "`r`n"
}

function Get-DshPatchInfo {
    param([string]$Text)

    $split = Split-TextIntoLines $Text
    $lines = [string[]]$split.Lines
    $entryStarts = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^-\s') { $entryStarts += $i }
    }

    $entries = @()
    for ($e = 0; $e -lt $entryStarts.Count; $e++) {
        $start = $entryStarts[$e]
        $end = if ($e + 1 -lt $entryStarts.Count) { $entryStarts[$e + 1] - 1 } else { $lines.Count - 1 }
        $entries += [pscustomobject]@{ Start = $start; End = $end }
    }

    $rows = @()
    foreach ($entry in $entries) {
        $rowStarts = @()
        for ($i = $entry.Start; $i -le $entry.End; $i++) {
            if ($lines[$i] -match '^(?<indent>\s*)- id:\s*(?<id>\S+)') {
                $rowStarts += [pscustomobject]@{ Line = $i; Indent = $Matches['indent'].Length; Id = $Matches['id'] }
            }
        }
        for ($r = 0; $r -lt $rowStarts.Count; $r++) {
            $rowStart = $rowStarts[$r]
            $rowEnd = $entry.End
            # Own body ends at the first nested row: a preset row's config and
            # plugins: key are its own, its child plugin rows are not.
            $ownEnd = $entry.End
            for ($n = $r + 1; $n -lt $rowStarts.Count; $n++) {
                if ($rowStarts[$n].Indent -le $rowStart.Indent) { $rowEnd = $rowStarts[$n].Line - 1; break }
                if ($ownEnd -eq $entry.End) { $ownEnd = $rowStarts[$n].Line - 1 }
            }
            if ($ownEnd -gt $rowEnd) { $ownEnd = $rowEnd }
            $ownBody = if ($ownEnd -ge $rowStart.Line) { @($lines[$rowStart.Line..$ownEnd]) } else { @() }
            $body = if ($rowEnd -ge $rowStart.Line) { @($lines[$rowStart.Line..$rowEnd]) } else { @() }
            $ownText = $ownBody -join "`n"
            $rowName = $null
            if ($ownText -match '(?m)^\s*name:\s*[''"]?([^''"\s]+)') { $rowName = $Matches[1] }
            $rows += [pscustomobject]@{
                Start   = $rowStart.Line
                End     = $rowEnd
                OwnEnd  = $ownEnd
                Indent  = $rowStart.Indent
                Id      = $rowStart.Id
                Name    = $rowName
                Text    = ($body -join "`n")
                OwnText = $ownText
            }
        }
    }

    $presets = @()
    foreach ($presetRow in $rows) {
        # Presets appear in two shapes: an inserted row naming the package, or a
        # top-level override (id preset-<name>) whose plugins list comes from the
        # row it overrides. Both are recognised by their plugins list + config id.
        $isPresetRow = ($presetRow.Name -eq '@deepseek-ai/dsh-agent-preset')
        if (-not $isPresetRow -and
            $presetRow.OwnText -match '(?m)^\s+plugins:\s*$' -and
            $presetRow.OwnText -match '(?m)^\s+id:\s*\S+') {
            $isPresetRow = $true
        }
        if (-not $isPresetRow) { continue }
        $presetId = $null
        if ($presetRow.OwnText -match '(?m)^\s+id:\s*[''"]?([^''"\s]+)') { $presetId = $Matches[1] }
        if (-not $presetId) { $presetId = $presetRow.Id }
        $presets += [pscustomobject]@{
            Id     = $presetId
            RowId  = $presetRow.Id
            Start  = $presetRow.Start
            End    = $presetRow.End
            Indent = $presetRow.Indent
        }
    }

    $serverRows = @()
    foreach ($row in $rows) {
        if ($row.OwnText -notmatch '(?m)^\s*serverName:\s*[''"]?(instant|instant-file-search)[''"]?\s*$') { continue }
        $ownerPreset = $null
        foreach ($preset in $presets) {
            if ($row.Start -gt $preset.Start -and $row.Start -le $preset.End) { $ownerPreset = $preset.Id }
        }
        $commandLines = @()
        $hasUrl = $false
        for ($i = $row.Start; $i -le $row.OwnEnd; $i++) {
            if ($lines[$i] -match '^\s*command:\s*.+$') { $commandLines += $i }
            if ($lines[$i] -match '^\s*url:\s*.+$') { $hasUrl = $true }
        }
        $serverRows += [pscustomobject]@{
            Start        = $row.Start
            End          = $row.End
            Indent       = $row.Indent
            Id           = $row.Id
            Scope        = if ($ownerPreset) { 'preset' } else { 'root' }
            PresetId     = $ownerPreset
            CommandLines = $commandLines
            HasUrl       = $hasUrl
        }
    }

    $presetMounts = @()
    foreach ($preset in $presets) {
        $presetMounts += [pscustomobject]@{
            Id      = $preset.Id
            Mounted = (@($serverRows | Where-Object { $_.PresetId -eq $preset.Id }).Count -gt 0)
        }
    }

    return [pscustomobject]@{
        Lines        = $lines
        Endings      = [string[]]$split.Endings
        Entries      = $entries
        Rows         = $rows
        Presets      = $presets
        PresetMounts = $presetMounts
        ServerRows   = $serverRows
        RootRows     = @($serverRows | Where-Object { $_.Scope -eq 'root' })
        PresetRows   = @($serverRows | Where-Object { $_.Scope -eq 'preset' })
    }
}

# Repoint every row that registers our server, wherever it lives. Rewriting only
# the first match used to leave a stale command behind in whichever scope came
# later in the file.
# Repoint every row that registers our server, wherever it lives. Rewriting only
# the first match used to leave a stale command behind in whichever scope came
# later in the file. Unchanged lines keep their original line endings.
function Set-DshServerCommand {
    param([string]$Text, [string]$CommandPath)

    $info = Get-DshPatchInfo $Text
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.AddRange([string[]]$info.Lines)

    $updated = 0
    $missing = @()
    $seen = @{}
    foreach ($row in $info.ServerRows) {
        if ($row.CommandLines.Count -eq 0) { $missing += $row; continue }
        $changedHere = $false
        foreach ($lineIndex in $row.CommandLines) {
            if ($seen.ContainsKey($lineIndex)) { continue }
            $seen[$lineIndex] = $true
            $indent = [regex]::Match($lines[$lineIndex], '^(\s*)').Groups[1].Value
            $desired = $indent + 'command: "' + $CommandPath + '"'
            if ($lines[$lineIndex] -ne $desired) {
                $lines[$lineIndex] = $desired
                $changedHere = $true
            }
        }
        if ($changedHere) { $updated++ }
    }

    $newText = if ($updated -gt 0) { Join-TextFromLines -Lines ([string[]]$lines) -Endings ([string[]]$info.Endings) } else { $Text }
    return [pscustomobject]@{
        Text               = $newText
        Updated            = $updated
        RowsWithoutCommand = @($missing)
    }
}

function New-DshClientRowLines {
    param([string]$CommandPath, [int]$Indent = 10, [string]$Id = 'mcp-instant-file-search')
    $pad = ' ' * $Indent
    $field = ' ' * ($Indent + 2)
    $cfg = ' ' * ($Indent + 4)
    return @(
        ($pad + '- id: ' + $Id)
        ($field + "name: '@deepseek-ai/dsh-mcp-client'")
        ($field + 'config:')
        ($cfg + 'serverName: ' + $serverName)
        ($cfg + 'transport: stdio')
        ($cfg + 'command: "' + $CommandPath + '"')
        ($cfg + 'failOnStartupError: true')
    )
}

function New-DshRootEntryLines {
    param([string]$CommandPath)
    return @('- insert:') + @(New-DshClientRowLines -CommandPath $CommandPath -Indent 4)
}

# Restore the root-scope row. It is what a plain (non-preset) agent sees; if a
# later edit or another tool moves the registration into presets only, the
# running session silently loses the tools until this row comes back.
# Restore the root-scope row. It is what a plain (non-preset) agent sees; if a
# later edit or another tool moves the registration into presets only, the
# running session silently loses the tools until this row comes back.
function Add-DshRootRow {
    param([string]$Text, [string]$CommandPath)
    $split = Split-TextIntoLines $Text
    $newline = Get-DominantNewline $split.Endings
    $entry = (New-DshRootEntryLines -CommandPath $CommandPath) -join $newline
    if ([string]::IsNullOrWhiteSpace($Text)) { return $entry + $newline }
    $trimmed = $Text.TrimEnd()
    return $trimmed + $newline + $newline + $entry + $newline
}

# A profile that loads the server from an installed bundle must not also get a
# patch row: two registrations of the same server name in one scope is a load
# error. Report the bundle that owns it instead of writing anything.
# Mount the server inside one agent preset by appending a child row to its
# plugins list. Only used when the caller asks for it with -DshPresets.
function Add-DshPresetRow {
    param([string]$Text, [string]$PresetId, [string]$CommandPath)

    $info = Get-DshPatchInfo $Text
    $preset = @($info.Presets | Where-Object { $_.Id -eq $PresetId -or $_.RowId -eq $PresetId })
    if ($preset.Count -eq 0) {
        return [pscustomobject]@{ Text = $Text; Added = $false; Reason = "preset '$PresetId' was not found in the profile patch" }
    }
    $preset = $preset[0]
    $mount = @($info.PresetMounts | Where-Object { $_.Id -eq $preset.Id })
    if ($mount.Count -gt 0 -and $mount[0].Mounted) {
        return [pscustomobject]@{ Text = $Text; Added = $false; Reason = "preset '$PresetId' already mounts $serverName" }
    }

    $split = Split-TextIntoLines $Text
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.AddRange([string[]]$split.Lines)
    $endings = [System.Collections.Generic.List[string]]::new()
    $endings.AddRange([string[]]$split.Endings)
    $newline = Get-DominantNewline ([string[]]$split.Endings)

    # Child indent: follow an existing plugin entry, else the plugins: key + 2.
    $childIndent = $null
    for ($i = $preset.Start + 1; $i -le $preset.End; $i++) {
        if ($lines[$i] -match '^(?<indent>\s+)- id:') { $childIndent = $Matches['indent'].Length; break }
    }
    if ($null -eq $childIndent) {
        for ($i = $preset.Start; $i -le $preset.End; $i++) {
            if ($lines[$i] -match '^(?<indent>\s+)plugins:\s*$') { $childIndent = $Matches['indent'].Length + 2; break }
        }
    }
    if ($null -eq $childIndent) {
        return [pscustomobject]@{ Text = $Text; Added = $false; Reason = "preset '$PresetId' has no plugins list" }
    }

    # Avoid a duplicate child id inside this preset scope.
    $existingIds = @($info.Rows | Where-Object { $_.Start -gt $preset.Start -and $_.Start -le $preset.End } | ForEach-Object { $_.Id })
    $rowId = 'mcp-instant-file-search'
    $suffix = 2
    while ($existingIds -contains $rowId) { $rowId = "mcp-instant-file-search-$suffix"; $suffix++ }

    $insertAt = $preset.End
    while ($insertAt -gt $preset.Start -and [string]::IsNullOrWhiteSpace($lines[$insertAt])) { $insertAt-- }
    $newLines = New-DshClientRowLines -CommandPath $CommandPath -Indent $childIndent -Id $rowId
    $lines.InsertRange($insertAt + 1, [string[]]$newLines)
    $newEndings = @()
    for ($i = 0; $i -lt $newLines.Count; $i++) { $newEndings += $newline }
    $endings.InsertRange($insertAt + 1, [string[]]$newEndings)
    if ($insertAt -lt $endings.Count -and [string]::IsNullOrEmpty($endings[$insertAt])) { $endings[$insertAt] = $newline }
    return [pscustomobject]@{ Text = (Join-TextFromLines -Lines ([string[]]$lines) -Endings ([string[]]$endings)); Added = $true; Reason = "mounted in preset '$PresetId'" }
}

function Find-DshBundleWithServer {
    param([string]$ProfileDir)
    $profilePackage = Join-Path $ProfileDir 'package.json'
    if (-not (Test-Path -LiteralPath $profilePackage)) { return $null }
    $bundles = @()
    try { $bundles = @((Read-TextFile $profilePackage | ConvertFrom-Json).dsh.profile.bundles) } catch { return $null }
    foreach ($bundle in $bundles) {
        if (-not $bundle) { continue }
        $bundlePatch = Join-Path $ProfileDir (Join-Path 'node_modules' (Join-Path $bundle 'cordis.patch.yml'))
        if (-not (Test-Path -LiteralPath $bundlePatch)) { continue }
        if ((Read-TextFile $bundlePatch) -match 'serverName:\s*[''"]?(instant|instant-file-search)') { return $bundle }
    }
    return $null
}

function Install-Dsh {
    Write-Step 'Configuring DeepSeek Harness'

    $target = $null
    try {
        $target = Resolve-DshProfile -Profile $DshProfile
    } catch {
        Write-Host "WARN: $($_.Exception.Message)" -ForegroundColor Yellow
        Write-Host "      DeepSeek Harness was left unchanged." -ForegroundColor Yellow
        $script:incomplete = $true
        return
    }
    if (-not $target) {
        Write-Host 'WARN: no DeepSeek Harness profile found (no cordis.patch.yml under the DSH home); skipping DSH registration.' -ForegroundColor Yellow
        return
    }

    $script:dshProfileName = $target.Name
    $patch = $target.Patch
    $text = Read-TextFile $patch
    $command = Get-DshBinaryPath
    $before = Get-DshPatchInfo $text
    $bundleOwner = Find-DshBundleWithServer -ProfileDir $target.Dir
    $presetGaps = @($before.PresetMounts | Where-Object { -not $_.Mounted } | ForEach-Object { $_.Id })

    if ($DryRun) {
        Write-Action "DeepSeek Harness profile '$($target.Name)' -> '$patch'"
        if ($before.RootRows.Count -gt 0) {
            Write-Action "Root row: present; would repoint to '$command'"
        } elseif ($bundleOwner) {
            Write-Action "Root row: absent, but bundle '$bundleOwner' already registers '$serverName' (nothing to write)"
        } else {
            Write-Action "Root row: missing; would append a root '- insert:' entry"
        }
        if ($before.PresetRows.Count -gt 0) { Write-Action "Preset rows: $($before.PresetRows.Count); would repoint each" }
        if ($presetGaps.Count -gt 0) { Write-Action "Presets without '$serverName': $($presetGaps -join ', ')" }
        return
    }

    if ($before.ServerRows.Count -eq 0 -and $bundleOwner) {
        Write-Host "NOTE: DeepSeek Harness loads '$serverName' from bundle '$bundleOwner'; nothing to write in '$patch'." -ForegroundColor Yellow
        Write-Host "      Update that bundle's cordis.patch.yml command to '$command' instead of adding a second entry." -ForegroundColor Yellow
        return
    }

    $original = $text
    $updated = 0
    $withoutCommand = @()
    if ($before.ServerRows.Count -gt 0) {
        $repoint = Set-DshServerCommand -Text $text -CommandPath $command
        $text = $repoint.Text
        $updated = $repoint.Updated
        $withoutCommand = @($repoint.RowsWithoutCommand)
    }
    $addedRoot = $false
    if ($before.RootRows.Count -eq 0) {
        $text = Add-DshRootRow -Text $text -CommandPath $command
        $addedRoot = $true
    }

    if ($text -ne $original) {
        Backup-Config $patch
        Write-TextFile -Path $patch -Text $text -WithBom (Test-TextFileHasBom $patch)
    }

    $after = Get-DshPatchInfo $text
    if ($text -ne $original) {
        $parts = @()
        if ($updated -gt 0) { $parts += "repointed $updated row(s)" }
        if ($addedRoot) { $parts += 'added the missing root row' }
        if ($parts.Count -eq 0) { $parts += 'normalised the patch text' }
        Write-Host ("PASS: DeepSeek Harness profile '$($target.Name)': " + ($parts -join '; ') + '.') -ForegroundColor Green
    } else {
        Write-Host "PASS: DeepSeek Harness profile '$($target.Name)' already registers '$serverName' with the current binary." -ForegroundColor Green
    }

    if ($withoutCommand.Count -gt 0) {
        Write-Host "WARN: $($withoutCommand.Count) row(s) declare '$serverName' without a 'command:' line (a URL transport, for example); left unchanged." -ForegroundColor Yellow
    }
    $remainingGaps = @($after.PresetMounts | Where-Object { -not $_.Mounted } | ForEach-Object { $_.Id })
    if ($remainingGaps.Count -gt 0) {
        Write-Host ("NOTE: this profile also defines agent presets that do not mount '$serverName': " + ($remainingGaps -join ', ') + '. Add them with -DshPresets <name> if those agents need file search.') -ForegroundColor Gray
    }
    if (Test-DshHostRunning) {
        Write-Host 'PASS: DeepSeek Harness is running and hot-applies this patch; the tools appear in the next agent turn (restart only if they do not).' -ForegroundColor Green
    } else {
        Write-Host 'NOTE: start DeepSeek Harness to load the new server.' -ForegroundColor Gray
    }
}

# Pure-function regression tests for the patch editor. CI runs
# `install.ps1 -SelfTest`; it never touches a real profile or service.
function Invoke-InstallerSelfTest {
    $script:selfTestFailures = 0

    function Assert-Equality($Actual, $Expected, [string]$What) {
        $a = if ($null -eq $Actual) { '' } else { "$Actual" }
        $e = if ($null -eq $Expected) { '' } else { "$Expected" }
        if ($a -ne $e) {
            $script:selfTestFailures++
            Write-Host ("  FAIL: $What`n        expected: $e`n        actual:   $a") -ForegroundColor Red
        } else {
            Write-Host "  PASS: $What" -ForegroundColor Green
        }
    }

    $rootOnly = @'
- id: ui-settings-general
  name: "@deepseek-ai/dsh-client-ui-settings-general"
  config:
    welcomeNoticeVersion: 2026-09-28.1
- insert:
    - id: mcp-instant-file-search
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: instant
        transport: stdio
        command: "C:/old/instant-file-search-mcp-server.exe"
        failOnStartupError: true
'@

    $presetOnly = @'
- insert:
    - id: preset-standard
      name: '@deepseek-ai/dsh-agent-preset'
      config:
        id: standard
        plugins:
          - id: persona
            name: '@deepseek-ai/dsh-persona'
          - id: mcp-instant-file-search
            name: '@deepseek-ai/dsh-mcp-client'
            config:
              serverName: "instant"
              transport: "stdio"
              command: "C:/old/instant-file-search-mcp-server.exe"
              failOnStartupError: true
- insert:
    - id: preset-reverse-engineering
      name: '@deepseek-ai/dsh-agent-preset'
      config:
        id: reverse-engineering
        plugins:
          - id: persona
            name: '@deepseek-ai/dsh-persona'
'@

    $urlRow = @'
- insert:
    - id: mcp-instant-file-search
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: instant
        transport: streamable-http
        url: "http://127.0.0.1:9999/mcp"
'@

    Write-Host 'Self-test: DSH patch editor' -ForegroundColor Cyan

    # 1. Root-only patch: one row, updated in place, no extra row added.
    $info = Get-DshPatchInfo $rootOnly
    Assert-Equality $info.ServerRows.Count 1 'root-only fixture has one server row'
    Assert-Equality $info.RootRows.Count 1 'root-only fixture row is root-scoped'
    Assert-Equality $info.Presets.Count 0 'root-only fixture has no presets'
    $edit = Set-DshServerCommand -Text $rootOnly -CommandPath 'C:/new/instant.exe'
    Assert-Equality $edit.Updated 1 'root-only fixture repoints one row'
    Assert-Equality (($edit.Text -match 'C:/new/instant\.exe').Count) 1 'root-only fixture writes the new command'
    $again = Set-DshServerCommand -Text $edit.Text -CommandPath 'C:/new/instant.exe'
    Assert-Equality $again.Updated 0 'root-only fixture is idempotent'

    # 2. Preset-only patch: nothing is root-scoped, and the root row is restored.
    $info = Get-DshPatchInfo $presetOnly
    Assert-Equality $info.ServerRows.Count 1 'preset-only fixture has one server row'
    Assert-Equality $info.RootRows.Count 0 'preset-only fixture has no root row'
    Assert-Equality $info.PresetRows.Count 1 'preset-only fixture row is preset-scoped'
    Assert-Equality $info.PresetRows[0].PresetId 'standard' 'preset-only fixture row belongs to the standard preset'
    $gaps = @($info.PresetMounts | Where-Object { -not $_.Mounted } | ForEach-Object { $_.Id })
    Assert-Equality ($gaps -join ',') 'reverse-engineering' 'preset-only fixture reports the unmounted preset'
    $healed = Add-DshRootRow -Text $presetOnly -CommandPath 'C:/new/instant.exe'
    $info = Get-DshPatchInfo $healed
    Assert-Equality $info.RootRows.Count 1 'root row is restored after healing'
    Assert-Equality $info.ServerRows.Count 2 'healing keeps the preset row and adds the root row'
    $repoint = Set-DshServerCommand -Text $healed -CommandPath 'C:/new/instant.exe'
    Assert-Equality $repoint.Updated 1 'healing repoints the stale preset row (the restored root row is already current)'
    Assert-Equality (([regex]::Matches($repoint.Text, 'C:/new/instant\.exe')).Count) 2 'every server row carries the new command'

    # 3. A row without command: line is reported, never rewritten.
    $edit = Set-DshServerCommand -Text $urlRow -CommandPath 'C:/new/instant.exe'
    Assert-Equality $edit.Updated 0 'URL-transport row is not rewritten'
    Assert-Equality $edit.RowsWithoutCommand.Count 1 'URL-transport row is reported as missing a command'
    Assert-Equality $edit.Text $urlRow 'URL-transport row text is unchanged'

    # 5. Other servers and unrelated text are not touched.
    $other = "- id: x`n  config:`n    serverName: somebody-else`n"
    $info = Get-DshPatchInfo $other
    Assert-Equality $info.ServerRows.Count 0 'unrelated serverName is ignored'
    $edit = Set-DshServerCommand -Text $other -CommandPath 'C:/new/instant.exe'
    Assert-Equality $edit.Text $other 'unrelated patch text is unchanged'

    # 5b. A preset declared as a top-level override (id preset-<name>) with a
    # plugins list must still be recognised, or its child row is mistaken for a
    # root row and the preset looks unmounted.
    $overridePreset = @(
        "- id: preset-standard"
        "  config:"
        "    id: standard"
        "    name: `"Standard`""
        "    plugins:"
        "      - id: persona"
        "        name: '@deepseek-ai/dsh-persona'"
        "      - id: mcp-instant-file-search"
        "        name: '@deepseek-ai/dsh-mcp-client'"
        "        config:"
        "          serverName: `"instant`""
        "          transport: stdio"
        "          command: `"C:/old/instant.exe`""
    ) -join "`r`n"
    $info = Get-DshPatchInfo $overridePreset
    Assert-Equality $info.Presets.Count 1 'top-level preset override is recognised'
    Assert-Equality $info.Presets[0].Id 'standard' 'override preset id is read from its config'
    Assert-Equality $info.RootRows.Count 0 'override preset child row is not root-scoped'
    Assert-Equality $info.PresetMounts[0].Mounted $true 'override preset reports its child row as mounted'

    # 6. UTF-8 round trip. The patch contains em dashes and CJK in the wild, and
    # Windows PowerShell 5.1 read them as ANSI before this helper existed.
    $tempFile = Join-Path $env:TEMP ("ifs-selftest-" + $PID + ".yml")
    try {
        $cjk = ([string][char]0x9884) + ([char]0x8BBE)
        $unicode = "id: test`r`nname: `"em dash " + ([char]0x2014) + " and CJK $cjk`"`r`n"
        Write-TextFile -Path $tempFile -Text $unicode -WithBom $false
        Assert-Equality (Test-TextFileHasBom $tempFile) $false 'UTF-8 writer omits the BOM by default'
        $roundTrip = Read-TextFile $tempFile
        Assert-Equality ($roundTrip.Contains([char]0x2014)) $true 'UTF-8 round trip keeps the em dash'
        Assert-Equality ($roundTrip.Contains($cjk)) $true 'UTF-8 round trip keeps CJK text'
        Write-TextFile -Path $tempFile -Text $unicode -WithBom $true
        Assert-Equality (Test-TextFileHasBom $tempFile) $true 'UTF-8 writer can preserve a BOM'
        Assert-Equality ((Read-TextFile $tempFile).Contains($cjk)) $true 'UTF-8 reader keeps content after a BOM'
    } finally {
        Remove-Item -LiteralPath $tempFile -Force -ErrorAction SilentlyContinue
    }
    # 7. Mixed LF/CRLF patches keep every line's terminator, so an unrelated
    # repoint does not rewrite (or re-encode) the whole file.
    $mixedLines = @(
        "- insert:`r`n"
        "    - id: mcp-instant-file-search`n"
        "      name: '@deepseek-ai/dsh-mcp-client'`n"
        "      config:`r`n"
        "        serverName: instant`n"
        "        command: `"C:/old/instant.exe`"`n"
    )
    $mixed = ($mixedLines -join '')
    $edit = Set-DshServerCommand -Text $mixed -CommandPath 'C:/new/instant.exe'
    Assert-Equality (([regex]::Matches($edit.Text, "`r`n")).Count) 2 'mixed patch keeps its CRLF endings'
    Assert-Equality (([regex]::Matches($edit.Text, "(?<!`r)`n")).Count) 4 'mixed patch keeps its LF endings'
    Assert-Equality ($edit.Text.Contains('C:/new/instant.exe')) $true 'mixed patch repoints the command'

    # 8. Preset mount: appended inside the right scope, and repeated calls do not duplicate.
    $mounted = Add-DshPresetRow -Text $presetOnly -PresetId 'reverse-engineering' -CommandPath 'C:/new/instant.exe'
    Assert-Equality $mounted.Added $true 'preset mount is added'
    $info = Get-DshPatchInfo $mounted.Text
    Assert-Equality (@($info.PresetRows | Where-Object { $_.PresetId -eq 'reverse-engineering' }).Count) 1 'mounted preset has exactly one row'
    Assert-Equality (($info.ServerRows | Where-Object { $_.Scope -eq 'root' }).Count) 0 'preset mount does not create a root row'
    $mountedAgain = Add-DshPresetRow -Text $mounted.Text -PresetId 'reverse-engineering' -CommandPath 'C:/new/instant.exe'
    Assert-Equality $mountedAgain.Added $false 'repeated preset mount is a no-op'
    Assert-Equality (([regex]::Matches($mountedAgain.Text, 'serverName:\s*[''"]?instant[''"]?')).Count) 2 'repeated preset mount does not duplicate the row'
    $mountNew = Add-DshPresetRow -Text $presetOnly -PresetId 'no-such-preset' -CommandPath 'C:/new/instant.exe'
    Assert-Equality $mountNew.Added $false 'unknown preset is reported, not created'

    # 9. CRLF-only patch keeps CRLF.
    $crlf = $rootOnly -replace "`n", "`r`n"
    $edit = Set-DshServerCommand -Text $crlf -CommandPath 'C:/new/instant.exe'
    Assert-Equality ($edit.Text.Contains("`r`n")) $true 'CRLF patch keeps CRLF line endings'
    Assert-Equality ($edit.Text.Contains("`n") -and (($edit.Text -replace "`r`n", '').Contains("`n"))) $false 'CRLF patch does not gain bare LF endings'

    # 10. Uninstall removes the registrations but keeps unrelated patch content.
    $removal = Remove-DshServerRows $healed
    Assert-Equality $removal.Removed 2 'uninstall removes both registrations'
    $info = Get-DshPatchInfo $removal.Text
    Assert-Equality $info.ServerRows.Count 0 'uninstall leaves no registration rows behind'
    Assert-Equality ($removal.Text.Contains('preset-reverse-engineering')) $true 'uninstall keeps unrelated preset definitions'
    Assert-Equality ($removal.Text.Contains('mcp-instant-file-search')) $false 'uninstall removes the instant row ids'
    if ($script:selfTestFailures -gt 0) {
        Write-Host "Self-test FAILED: $($script:selfTestFailures) failure(s)." -ForegroundColor Red
        return 1
    }
    Write-Host 'Self-test passed.' -ForegroundColor Green
    return 0
}

function Get-DetectedClients {
    $found = @()
    if ((Get-Command codex -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath (Join-Path $env:USERPROFILE '.codex\config.toml'))) { $found += 'codex' }
    if ((Get-Command opencode -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath $openCodeConfigDir)) { $found += 'opencode' }
    if (Test-ClaudeInstalled) { $found += 'claude' }
    if (Test-HermesInstalled) { $found += 'hermes' }
    if (Test-DshInstalled) { $found += 'dsh' }
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
    if ($NonInteractive) { throw 'No -Clients was supplied and this run is non-interactive. Pass -Clients dsh (or another comma-separated client list).' }
    Write-Host 'Choose clients to configure: [A]ll detected, or enter a comma-separated list: codex, opencode, claude, hermes, dsh'
    $answer = Read-Host 'Selection (default A)'
    if ([string]::IsNullOrWhiteSpace($answer)) { return $detected }
    $choice = $answer.Trim().ToLowerInvariant()
    if ($choice -eq 'a') { return $detected }
    if ($choice -eq 'n') { return @() }
    $selected = @($choice -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $invalid = @($selected | Where-Object { $_ -notin @('codex', 'opencode', 'claude', 'hermes', 'dsh') })
    if ($invalid) { throw "Unknown client(s): $($invalid -join ', '). Use codex, opencode, claude, hermes, dsh, or all detected." }
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

# Replace the installed server binary. On Windows a running client holds the
# current exe open, so a plain Copy-Item fails with a sharing violation (this
# aborted real upgrades). Stop the server processes - their host restarts them -
# and retry once before giving up with an actionable message.
function Install-ServerBinary([string]$Source, [string]$Destination) {
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
            return
        } catch {
            if ($attempt -ge 2) {
                throw ("Could not replace '$Destination': a running MCP server still holds the file. " +
                    "Close DeepSeek Harness (or whichever client runs this server) and run the installer again. " +
                    "Underlying error: $($_.Exception.Message)")
            }
            $running = @(Get-Process -Name 'instant-file-search-mcp-server' -ErrorAction SilentlyContinue)
            if ($running.Count -gt 0) {
                Write-Host "   Stopping $($running.Count) running MCP server process(es) so the binary can be replaced..." -ForegroundColor Gray
                $running | Stop-Process -Force -ErrorAction SilentlyContinue
                Start-Sleep -Milliseconds 800
            } else {
                Start-Sleep -Milliseconds 300
            }
        }
    }
}

function Get-BinaryVersion([string]$Path) {
    try {
        $out = (& $Path --version 2>$null | Select-Object -First 1)
        if ($out -and $out -match '^instant-file-search-mcp-server\s+([0-9]+\.[0-9]+\.[0-9]+)') { return $Matches[1] }
    } catch {}
    return $null
}

function Get-SourceVersion {
    if (-not $isCheckout) { return $null }
    $cargo = Join-Path $repoRoot 'Cargo.toml'
    if (-not (Test-Path -LiteralPath $cargo)) { return $null }
    $match = Select-String -LiteralPath $cargo -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($match) { return $match.Matches[0].Groups[1].Value }
    return $null
}

function Resolve-ServerBinary {
    if ($ServerBinary) {
        if (-not (Test-Path -LiteralPath $ServerBinary)) { throw "Server binary not found: $ServerBinary" }
        return $ServerBinary
    }
    if ($isCheckout) {
        $local = Join-Path $repoRoot "target\release\$binaryName"
        if (Test-Path -LiteralPath $local) {
            # A stale target\release build used to be installed silently: the
            # installer took its version from the binary, so an old build could
            # overwrite a live versioned directory (or downgrade an existing
            # install) and every client would be repointed at it. Refuse when
            # the build does not match the tree it came from.
            $builtVersion = Get-BinaryVersion $local
            $sourceVersion = Get-SourceVersion
            if ($builtVersion -and $sourceVersion -and $builtVersion -ne $sourceVersion) {
                throw ("target\release\$binaryName reports version $builtVersion but Cargo.toml is $sourceVersion, so it is a stale build. " +
                    "Run 'cargo build --release', or pass -ServerBinary <path> to install a specific build, " +
                    "or run the published installer (irm .../scripts/install.ps1 | iex) to install the current release instead of a local build.")
            }
            if (-not $builtVersion) {
                Write-Host "   WARN: '$local' does not report a version; installing it as a $sourceVersion local build." -ForegroundColor Yellow
            }
            return $local
        }
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

function Resolve-ServiceHelper {
    # The admin-only half of the install lives in scripts\ (and ships as a
    # release asset) instead of being generated from a here-string: it can be
    # reviewed, syntax-checked, and exercised by CI without elevation.
    $helperName = 'register-indexer-service.ps1'
    if ($isCheckout) {
        $local = Join-Path $repoRoot "scripts\$helperName"
        if (Test-Path -LiteralPath $local) { return $local }
    }
    if ($SkipDownload) { throw "Service helper '$helperName' was not found and -SkipDownload was set." }
    New-Item -ItemType Directory -Path $downloadRoot -Force | Out-Null
    $downloaded = Join-Path $downloadRoot $helperName
    Get-ReleaseAsset $helperName $downloaded
    return $downloaded
}

function Resolve-IndexerBinary {
    if ($IndexerBinary) {
        if (-not (Test-Path -LiteralPath $IndexerBinary)) { throw "Indexer binary not found: $IndexerBinary" }
        return $IndexerBinary
    }
    if ($isCheckout) {
        $local = Join-Path $repoRoot "target\release\$indexerName"
        if (Test-Path -LiteralPath $local) {
            # Keep server and indexer from the same tree: pairing a release
            # server with a stale local indexer mismatches the IPC protocol.
            $builtVersion = $null
            try {
                $out = (& $local --version 2>$null | Select-Object -First 1)
                if ($out -and $out -match '^instant-file-search-indexer\s+([0-9]+\.[0-9]+\.[0-9]+)') { $builtVersion = $Matches[1] }
            } catch {}
            $sourceVersion = Get-SourceVersion
            if ($builtVersion -and $sourceVersion -and $builtVersion -ne $sourceVersion) {
                throw ("target\release\$indexerName reports version $builtVersion but Cargo.toml is $sourceVersion, so it is a stale build. " +
                    "Run 'cargo build --release', or pass -IndexerBinary <path> together with -ServerBinary.")
            }
            return $local
        }
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

function Test-FallbackEngineInstalled {
    # An Already-installed Everything means the user is relying on it today, so
    # an upgrade should not silently take it away.
    if (Get-Process -Name Everything -ErrorAction SilentlyContinue) { return $true }
    $roots = @($env:ProgramFiles)
    if (${env:ProgramFiles(x86)}) { $roots += ${env:ProgramFiles(x86)} }
    foreach ($root in $roots) {
        if ($root -and (Test-Path -LiteralPath (Join-Path $root 'Everything\Everything.exe'))) { return $true }
    }
    foreach ($key in @(
            'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Everything',
            'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Everything')) {
        if (Test-Path $key) { return $true }
    }
    return $false
}

function Resolve-FallbackEngineChoice {
    # $NoPrompt is passed for dry runs; $NonInteractive is the script-level flag.
    # Both must suppress the question, otherwise a scripted install blocks on it.
    param([switch]$NoPrompt)
    if ($IncludeFallbackEngine) { return 'include' }
    if (Test-FallbackEngineInstalled) {
        Write-Host 'INFO: Everything is already installed on this machine, so the Fallback Engine is included to keep that working.' -ForegroundColor Cyan
        return 'include'
    }
    if ($NoPrompt -or $NonInteractive) {
        Write-Host 'INFO: Fallback Engine not installed (optional; the native indexer handles all searches). Pass -IncludeFallbackEngine to add it.' -ForegroundColor Cyan
        return 'skip'
    }
    Write-Host ''
    Write-Host 'The native indexer is the primary engine. Everything is not required.'
    Write-Host 'The Fallback Engine is optional: it gives you a second engine to compare'
    Write-Host 'against for parity testing, or a backup if the native indexer fails.'
    $answer = Read-Host 'Include the optional Fallback Engine? [y/N]'
    if ($answer -match '^(y|yes)$') { return 'include' }
    return 'skip'
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
    $raw = Read-TextFile $Path
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
    $Config | ConvertTo-Json -Depth 20 | ForEach-Object { Write-TextFile -Path $Path -Text $_ }
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

    $raw = Read-TextFile $Path
    $migrated = Migrate-LegacyMcpKeys $raw
    if ($migrated -ne $raw) {
        # A legacy registration key exists. Rewrite the file with the current
        # key so the migration persists, then continue to refresh the entry.
        Backup-Config $Path
        Write-TextFile -Path $Path -Text $migrated
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
            Write-TextFile -Path $Path -Text $inserted
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
        $existingRaw = Read-TextFile $hermesConfig
        if ($existingRaw.Contains("`r`n")) { "`r`n" } else { "`n" }
    } else { "`r`n" }

    $raw = Read-TextFile $hermesConfig
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
        Write-TextFile -Path $hermesConfig -Text ($lines -join $newline) -WithBom (Test-TextFileHasBom $hermesConfig)
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
# Look for registrations that a *different* client still points at an old copy of
# the server. The installer only updates the clients it was asked to, and a stale
# Codex/Hermes/OpenCode/Claude entry then looks healthy from the outside.
function Get-StaleClientReferences {
    param([string]$ExpectedBinary)
    $stale = @()
    $expected = $ExpectedBinary -replace '\\', '/'
    $knownNames = @($serverName, 'everything', 'instant-file-search')

    $userBinary = [Environment]::GetEnvironmentVariable('INSTANT_FS_MCP_BINARY', 'User')
    if ($userBinary -and ($userBinary -replace '\\', '/') -ne $expected) {
        $stale += [pscustomobject]@{ Client = 'opencode'; Detail = "INSTANT_FS_MCP_BINARY points at '$userBinary'" }
    }

    $codexConfig = Join-Path $env:USERPROFILE '.codex\config.toml'
    if (Test-Path -LiteralPath $codexConfig) {
        $codexLines = [string[]]((Read-TextFile $codexConfig) -split "\r?\n")
        $currentName = $null
        for ($i = 0; $i -lt $codexLines.Count; $i++) {
            if ($codexLines[$i] -match '^\s*\[mcp_servers\.(?<name>[^\]]+)\]\s*$') { $currentName = $Matches['name']; continue }
            if ($codexLines[$i] -match '^\s*\[') { $currentName = $null; continue }
            if (-not $currentName -or ($knownNames -notcontains $currentName)) { continue }
            if ($codexLines[$i] -match '^\s*command\s*=\s*[''"]?(?<command>.+?)[''"]?\s*$') {
                $command = $Matches['command'].Trim().Trim('"').Trim("'")
                if ($currentName -ne $serverName) {
                    $stale += [pscustomobject]@{ Client = 'codex'; Detail = "legacy mcp_servers.$currentName section" }
                } elseif (($command -replace '\\', '/') -ne $expected) {
                    $stale += [pscustomobject]@{ Client = 'codex'; Detail = "mcp_servers.$currentName points at '$command'" }
                }
            }
        }
    }

    if (Test-Path -LiteralPath $hermesConfig) {
        $hermesLines = [string[]]((Read-TextFile $hermesConfig) -split "\r?\n")
        $hermesName = $null
        for ($i = 0; $i -lt $hermesLines.Count; $i++) {
            if ($hermesLines[$i] -match '^  (?<name>[A-Za-z0-9._-]+):\s*$') { $hermesName = $Matches['name']; continue }
            if ($hermesLines[$i] -match '^\S') { $hermesName = $null; continue }
            if (-not $hermesName -or ($knownNames -notcontains $hermesName)) { continue }
            if ($hermesLines[$i] -match '^\s+command:\s*[''"]?(?<command>.+?)[''"]?\s*$') {
                $command = $Matches['command'].Trim().Trim('"').Trim("'")
                if ($hermesName -ne $serverName) {
                    $stale += [pscustomobject]@{ Client = 'hermes'; Detail = "legacy mcp_servers.$hermesName entry" }
                } elseif (($command -replace '\\', '/') -ne $expected) {
                    $stale += [pscustomobject]@{ Client = 'hermes'; Detail = "mcp_servers.$hermesName points at '$command'" }
                }
                $hermesName = $null
            }
        }
    }

    $openConfig = Resolve-OpenCodeConfig
    if ($openConfig) {
        $openCfg = $null
        try { $openCfg = Read-JsonConfig $openConfig } catch {}
        $openMcp = if ($openCfg) { $openCfg.PSObject.Properties['mcp'].Value } else { $null }
        foreach ($name in $knownNames) {
            if (-not $openMcp -or -not $openMcp.PSObject.Properties[$name]) { continue }
            $entry = $openMcp.PSObject.Properties[$name].Value
            $command = if ($entry -and @($entry.command).Count -gt 0) { @($entry.command)[0] } else { $null }
            if ($name -ne $serverName) {
                $stale += [pscustomobject]@{ Client = 'opencode'; Detail = "legacy mcp.$name entry" }
            } elseif ($command -and ($command -replace '\\', '/') -ne $expected) {
                $stale += [pscustomobject]@{ Client = 'opencode'; Detail = "mcp.$name points at '$command'" }
            }
        }
    }

    if (Test-Path -LiteralPath $claudeConfig) {
        try {
            $claudeCfg = (Read-TextFile $claudeConfig) | ConvertFrom-Json
            $claudeServers = $claudeCfg.PSObject.Properties['mcpServers'].Value
            foreach ($name in $knownNames) {
                if (-not $claudeServers -or -not $claudeServers.PSObject.Properties[$name]) { continue }
                $entry = $claudeServers.PSObject.Properties[$name].Value
                $command = if ($entry -and $entry.PSObject.Properties['command']) { $entry.command } else { $null }
                if ($name -ne $serverName) {
                    $stale += [pscustomobject]@{ Client = 'claude'; Detail = "legacy mcpServers.$name entry" }
                } elseif ($command -and ($command -replace '\\', '/') -ne $expected) {
                    $stale += [pscustomobject]@{ Client = 'claude'; Detail = "mcpServers.$name points at '$command'" }
                }
            }
        } catch {}
    }

    return @($stale)
}

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
        if (Get-Service -Name $serviceName -ErrorAction SilentlyContinue) {
            Write-Action "Update existing service '$serviceName' to run '$serviceIndexer' (start= auto)"
        } elseif ($elevated) {
            Write-Action "sc.exe create $serviceName binPath= `"$serviceIndexer service`" start= auto"
        } else {
            Write-Action "Register service '$serviceName' (needs admin; a UAC prompt follows unless -SkipElevation or -NonInteractive is set)"
        }
        return
    }

    New-Item -ItemType Directory -Path $indexerDir -Force | Out-Null

    # The admin-only half lives in scripts\register-indexer-service.ps1 (shipped
    # as a release asset too) instead of a generated here-string: it can be
    # reviewed, syntax-checked, and bound by CI without elevation.
    $helperSource = Resolve-ServiceHelper
    if ($helperSource -ne $registerHelper) {
        Copy-Item -LiteralPath $helperSource -Destination $registerHelper -Force
    }
    $helperArgs = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$registerHelper`"",
        '-SourceIndexer', "`"$resolved`"",
        '-ServiceIndexer', "`"$serviceIndexer`"",
        '-ServiceName', "`"$serviceName`"",
        '-IndexMode', $IndexMode,
        '-ContentMode', $ContentMode
    )
    $helperCommand = 'powershell -NoProfile -ExecutionPolicy Bypass -File "' + $registerHelper + '" -SourceIndexer "' + $resolved + '" -ServiceIndexer "' + $serviceIndexer + '" -ServiceName "' + $serviceName + '" -IndexMode ' + $IndexMode + ' -ContentMode ' + $ContentMode

    if ($elevated) {
        & powershell.exe @helperArgs
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
        Write-Host "  $helperCommand" -ForegroundColor Gray
        $script:incomplete = $true
        return
    }

    if ($NonInteractive) {
        Write-Host 'WARN: not elevated and -NonInteractive was set, so the indexer service was NOT registered. Searches will use the Fallback Engine until it is registered. Run this elevated:' -ForegroundColor Yellow
        Write-Host "  $helperCommand" -ForegroundColor Gray
        $script:incomplete = $true
        return
    }

    Write-Host 'The native indexer service needs administrator rights. A UAC prompt will appear - click Yes.' -ForegroundColor Yellow
    try {
        $elevatedProcess = Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList $helperArgs -PassThru -ErrorAction Stop
        $timedOut = $false
        if ($ElevationTimeoutSec -gt 0) { $timedOut = -not $elevatedProcess.WaitForExit($ElevationTimeoutSec * 1000) }
        else { $elevatedProcess.WaitForExit() }
        if ($timedOut) {
            Write-Host "WARN: the UAC prompt was not answered within $ElevationTimeoutSec s. The installer continues without the native indexer; approve the prompt, or run the helper elevated:" -ForegroundColor Yellow
            Write-Host "  $helperCommand" -ForegroundColor Gray
            $script:incomplete = $true
            return
        }
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
            $script:incomplete = $true
            Write-Host "  $helperCommand" -ForegroundColor Gray
        }
    } catch {
        Write-Host "WARN: elevation was cancelled or failed: $($_.Exception.Message)" -ForegroundColor Yellow
        $script:incomplete = $true
        Write-Host "Run this elevated when ready:" -ForegroundColor Yellow
        Write-Host "  $helperCommand" -ForegroundColor Gray
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

# Minimal MCP client probe: initialize, tools/list, then one real tool call.
# Used by the install verification and exercised by the doctor-style checks.
# Minimal MCP client probe: initialize, tools/list, then one real tool call.
# The reader runs on its own runspace because Process.StandardOutput forbids
# concurrent ReadLineAsync calls, and a stale pending read would poison the
# next response (that made the search_status check flaky).
function Invoke-McpProbe {
    param([string]$ExePath, [int]$TimeoutMs = 25000)

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ExePath
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true

    $proc = $null
    $queue = $null
    $readerPs = $null
    $readerRs = $null
    try {
        $proc = [System.Diagnostics.Process]::Start($psi)
        $queue = New-Object 'System.Collections.Concurrent.BlockingCollection[string]'

        $readerRs = [runspacefactory]::CreateRunspace()
        $readerRs.Open()
        $readerPs = [powershell]::Create()
        $readerPs.Runspace = $readerRs
        [void]$readerPs.AddScript({
            param($stream, $target)
            try {
                while (($line = $stream.ReadLine()) -ne $null) { $target.Add($line) }
            } catch {}
            try { $target.CompleteAdding() } catch {}
        }).AddArgument($proc.StandardOutput).AddArgument($queue)
        [void]$readerPs.BeginInvoke()

        $deadline = (Get-Date).AddMilliseconds($TimeoutMs)

        $send = {
            param($payload)
            $proc.StandardInput.WriteLine(($payload | ConvertTo-Json -Compress -Depth 10))
            $proc.StandardInput.Flush()
        }
        $readMatching = {
            param([int]$id)
            while ((Get-Date) -lt $deadline) {
                $remaining = [int]($deadline - (Get-Date)).TotalMilliseconds
                if ($remaining -le 0) { return $null }
                $line = $null
                if (-not $queue.TryTake([ref]$line, $remaining)) { return $null }
                if ([string]::IsNullOrWhiteSpace($line)) { continue }
                try { $msg = $line | ConvertFrom-Json } catch { continue }
                if ($msg.id -eq $id) { return $msg }
            }
            return $null
        }

        & $send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2024-11-05'; capabilities = @{}; clientInfo = @{ name = 'installer'; version = '1.0' } } }
        $init = & $readMatching 1
        if (-not $init) { return $null }
        & $send @{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = @{} }
        & $send @{ jsonrpc = '2.0'; id = 2; method = 'tools/list'; params = @{} }
        $tools = & $readMatching 2
        & $send @{ jsonrpc = '2.0'; id = 3; method = 'tools/call'; params = @{ name = 'search_status'; arguments = @{} } }
        $status = & $readMatching 3

        $statusText = $null
        if ($status -and $status.result -and $status.result.content) { $statusText = $status.result.content[0].text }
        return [pscustomobject]@{
            ServerName = $init.result.serverInfo.name
            Version    = $init.result.serverInfo.version
            ToolNames  = @($tools.result.tools | ForEach-Object { $_.name })
            StatusText = $statusText
        }
    } catch {
        return $null
    } finally {
        if ($proc) {
            try { $proc.StandardInput.Close() } catch {}
            Start-Sleep -Milliseconds 200
            if (-not $proc.HasExited) { try { $proc.Kill() } catch {} }
        }
        if ($readerPs) { try { $readerPs.Dispose() } catch {} }
        if ($readerRs) { try { $readerRs.Dispose() } catch {} }
    }
}


# The indexer creates its named pipe only after the first index build, which can
# take minutes on large volumes. Poll instead of assuming the service start is enough.
function Wait-NativeIndexer {
    param([int]$TimeoutSec = 180)
    if ((Get-Service -Name $serviceName -ErrorAction SilentlyContinue).Status -ne 'Running') { return $false }
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $pipes = [System.IO.Directory]::GetFiles('\\.\pipe\')
            if (@($pipes | Where-Object { $_ -match 'instant-file-search-indexer' }).Count -gt 0) { return $true }
        } catch {}
        Start-Sleep -Seconds 2
    }
    return $false
}

function Test-Installation {
    Write-Step 'Verifying the installation'
    if (-not (Test-Path -LiteralPath $stableBinary)) {
        Write-Host "FAIL: MCP server binary not found at '$stableBinary'." -ForegroundColor Red
        return $false
    }
    $ok = $true
    $probe = Invoke-McpProbe -ExePath $stableBinary
    if (-not $probe) {
        Write-Host 'FAIL: the MCP server did not answer initialize/tools/list within the timeout.' -ForegroundColor Red
        return $false
    }
    Write-Host ("PASS: MCP server {0} answers initialize and lists {1} tool(s)." -f $probe.Version, $probe.ToolNames.Count) -ForegroundColor Green
    $expectedTools = @('find_files', 'count_files', 'search_status', 'recent_changes', 'aggregate_files')
    $missingTools = @($expectedTools | Where-Object { $probe.ToolNames -notcontains $_ })
    if ($missingTools.Count -gt 0) {
        Write-Host ("FAIL: tools/list is missing: " + ($missingTools -join ', ')) -ForegroundColor Red
        $ok = $false
    }

    $status = $null
    if ($probe.StatusText) {
        try { $status = $probe.StatusText | ConvertFrom-Json } catch {}
    }
    if ($status) {
        $nativeOk = [bool]$status.native.ok
        if ($nativeOk) {
            Write-Host ("PASS: native indexer is serving ({0:N0} files indexed)." -f [double]$status.native.detail.indexed) -ForegroundColor Green
        } else {
            Write-Host "WARN: native indexer is not serving yet ($($status.native.error)); searches answer from the Fallback Engine until it is ready." -ForegroundColor Yellow
        }
        if ($status.everything -and $status.everything.ok) {
            Write-Host 'PASS: Fallback Engine is connected.' -ForegroundColor Green
        } elseif (-not $nativeOk) {
            Write-Host 'FAIL: neither the native indexer nor the Fallback Engine is connected yet.' -ForegroundColor Red
            $ok = $false
        }
    } else {
        Write-Host 'WARN: search_status did not return a parsable result.' -ForegroundColor Yellow
    }

    # The profile patch only helps if the harness composes it. Reject a wrong
    # profile or a malformed patch here instead of leaving the user with a
    # successful install and no tools.
    $dshProfileName = $script:dshProfileName
    if (-not $dshProfileName) { $dshProfileName = $DshProfile }
    if ($dshProfileName -and (Get-Command dsh -ErrorAction SilentlyContinue)) {
        try {
            $dump = (& dsh --profile $dshProfileName --dump-config 2>$null | Out-String)
            if ($dump -match 'serverName:\s*[''"]?instant') {
                Write-Host "PASS: DeepSeek Harness profile '$dshProfileName' composes the '$serverName' server." -ForegroundColor Green
            } else {
                Write-Host "FAIL: DeepSeek Harness profile '$dshProfileName' did not compose the '$serverName' server. Re-run with -DshProfile <name>." -ForegroundColor Red
                $ok = $false
            }
        } catch {
            Write-Host "WARN: could not run 'dsh --dump-config' to verify the profile: $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }
    return $ok
}

# --- Uninstall --------------------------------------------------------------
# Removes the registrations and files this installer created. Client configs are
# edited in place with the same backups the install path takes.

function Remove-TomlMcpSection {
    param([string]$Text, [string]$Name)
    $pattern = '^\s*\[mcp_servers\.' + [regex]::Escape($Name) + '(\.[^\]]+)?\]\s*$'
    $lines = [string[]]($Text -split "\r?\n")
    $out = [System.Collections.Generic.List[string]]::new()
    $skip = $false
    foreach ($line in $lines) {
        if ($line -match '^\s*\[mcp_servers\.') {
            $skip = ($line -match $pattern)
            if ($skip) { continue }
        } elseif ($line -match '^\s*\[') {
            $skip = $false
        }
        if (-not $skip) { $out.Add($line) }
    }
    return ($out -join "`n")
}

function Remove-HermesMcpEntry {
    param([string]$Text, [string]$Name)
    $lines = [string[]]($Text -split "\r?\n")
    $out = [System.Collections.Generic.List[string]]::new()
    $skip = $false
    foreach ($line in $lines) {
        if ($line -match ('^  ' + [regex]::Escape($Name) + ':\s*$')) { $skip = $true; continue }
        if ($skip -and $line -match '^  \S') { $skip = $false }
        if (-not $skip) { $out.Add($line) }
    }
    return ($out -join "`n")
}

function Remove-JsonMcpKey {
    param([string]$Path, [string[]]$Names, [string]$Container)
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $cfg = $null
    try { $cfg = Read-JsonConfig $Path } catch { return $false }
    if (-not $cfg) { return $false }
    $containerObject = $cfg.PSObject.Properties[$Container].Value
    if (-not $containerObject) { return $false }
    $removed = $false
    foreach ($name in $Names) {
        if ($containerObject.PSObject.Properties[$name]) {
            $containerObject.PSObject.Properties.Remove($name)
            $removed = $true
        }
    }
    if ($removed) { Write-JsonConfig $Path $cfg }
    return $removed
}

# Remove every row that registers our server. A top-level `- insert:` entry that
# exists only for this server is removed whole; inside a preset only the child
# row goes, leaving the rest of the preset intact.
function Remove-DshServerRows {
    param([string]$Text)
    $info = Get-DshPatchInfo $Text
    if ($info.ServerRows.Count -eq 0) { return [pscustomobject]@{ Text = $Text; Removed = 0 } }

    $serverStarts = @($info.ServerRows | ForEach-Object { $_.Start })
    $ranges = @()
    foreach ($row in $info.ServerRows) {
        $entry = @($info.Entries | Where-Object { $row.Start -ge $_.Start -and $row.Start -le $_.End }) | Select-Object -First 1
        $otherRowsInEntry = @()
        if ($entry) {
            $otherRowsInEntry = @($info.Rows | Where-Object {
                $_.Start -ge $entry.Start -and $_.Start -le $entry.End -and ($serverStarts -notcontains $_.Start)
            })
        }
        if ($entry -and $otherRowsInEntry.Count -eq 0) {
            $ranges += [pscustomobject]@{ Start = $entry.Start; End = $entry.End }
        } else {
            $ranges += [pscustomobject]@{ Start = $row.Start; End = $row.End }
        }
    }

    $split = Split-TextIntoLines $Text
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.AddRange([string[]]$split.Lines)
    $endings = [System.Collections.Generic.List[string]]::new()
    $endings.AddRange([string[]]$split.Endings)
    $removedLines = 0
    foreach ($range in ($ranges | Sort-Object Start -Descending)) {
        $count = $range.End - $range.Start + 1
        if ($count -le 0) { continue }
        $lines.RemoveRange($range.Start, $count)
        $endings.RemoveRange($range.Start, $count)
        $removedLines += $count
    }
    # Keep the file newline-terminated if it was before.
    if ($lines.Count -gt 0 -and $endings.Count -gt 0 -and [string]::IsNullOrEmpty($endings[$endings.Count - 1])) {
        $endings[$endings.Count - 1] = Get-DominantNewline ([string[]]$split.Endings)
    }
    return [pscustomobject]@{ Text = (Join-TextFromLines -Lines ([string[]]$lines) -Endings ([string[]]$endings)); Removed = $info.ServerRows.Count }
}

function Invoke-Uninstall {
    Write-Host 'Instant File Search MCP uninstaller' -ForegroundColor Green
    Write-Host 'Close any AI app that uses this server (DeepSeek Harness, Codex, OpenCode, Claude, Hermes) first: a running MCP server process keeps its binary locked.' -ForegroundColor Gray
    $names = @($serverName, 'everything', 'instant-file-search')

    # 1) DeepSeek Harness registrations.
    if (Test-DshInstalled) {
        $target = $null
        try { $target = Resolve-DshProfile -Profile $DshProfile }
        catch { Write-Host "WARN: $($_.Exception.Message)" -ForegroundColor Yellow }
        if ($target) {
            if ($DryRun) {
                Write-Action "Remove '$serverName' registration row(s) from DeepSeek Harness profile '$($target.Name)'"
            } else {
                $text = Read-TextFile $target.Patch
                $result = Remove-DshServerRows $text
                if ($result.Removed -gt 0) {
                    Backup-Config $target.Patch
                    Write-TextFile -Path $target.Patch -Text $result.Text -WithBom (Test-TextFileHasBom $target.Patch)
                    Write-Host "PASS: removed $($result.Removed) DeepSeek Harness registration row(s) from '$($target.Patch)'." -ForegroundColor Green
                } else {
                    Write-Host 'DeepSeek Harness has no registration for this server; nothing to remove.' -ForegroundColor Gray
                }
            }
        }
    }

    # 2) User environment variables used by the OpenCode adapter.
    foreach ($variableName in @('INSTANT_FS_MCP_BINARY', 'EVERYTHING_MCP_BINARY')) {
        if ([Environment]::GetEnvironmentVariable($variableName, 'User')) {
            if ($DryRun) {
                Write-Action "Clear user environment variable $variableName"
            } else {
                [Environment]::SetEnvironmentVariable($variableName, $null, 'User')
                Write-Host "PASS: cleared $variableName." -ForegroundColor Green
            }
        }
    }

    # 3) Client configs.
    foreach ($client in @($UninstallClients)) {
        switch ($client) {
            'codex' {
                $codexConfig = Join-Path $env:USERPROFILE '.codex\config.toml'
                if (Test-Path -LiteralPath $codexConfig) {
                    if ($DryRun) { Write-Action "Remove the instant MCP sections from '$codexConfig'" }
                    else {
                        $codexText = Read-TextFile $codexConfig
                        $cleaned = $codexText
                        foreach ($name in $names) { $cleaned = Remove-TomlMcpSection $cleaned $name }
                        if ($cleaned -ne $codexText) {
                            Backup-Config $codexConfig
                            Write-TextFile -Path $codexConfig -Text $cleaned -WithBom (Test-TextFileHasBom $codexConfig)
                            Write-Host "PASS: removed '$serverName' from Codex config." -ForegroundColor Green
                        }
                    }
                }
            }
            'opencode' {
                $openConfig = Resolve-OpenCodeConfig
                if ($openConfig) {
                    if ($DryRun) { Write-Action "Remove '$serverName' (and legacy keys) from '$openConfig'" }
                    elseif (Remove-JsonMcpKey -Path $openConfig -Names $names -Container 'mcp') {
                        Write-Host "PASS: removed '$serverName' from '$openConfig'." -ForegroundColor Green
                    }
                }
            }
            'claude' {
                if (Test-Path -LiteralPath $claudeConfig) {
                    if ($DryRun) { Write-Action "Remove '$serverName' (and legacy keys) from '$claudeConfig'" }
                    elseif (Remove-JsonMcpKey -Path $claudeConfig -Names $names -Container 'mcpServers') {
                        Write-Host "PASS: removed '$serverName' from Claude Desktop config." -ForegroundColor Green
                    }
                }
            }
            'hermes' {
                if (Test-Path -LiteralPath $hermesConfig) {
                    if ($DryRun) { Write-Action "Remove '$serverName' (and legacy keys) from '$hermesConfig'" }
                    else {
                        $hermesText = Read-TextFile $hermesConfig
                        $cleaned = $hermesText
                        foreach ($name in $names) { $cleaned = Remove-HermesMcpEntry $cleaned $name }
                        if ($cleaned -ne $hermesText) {
                            Backup-Config $hermesConfig
                            Write-TextFile -Path $hermesConfig -Text $cleaned -WithBom (Test-TextFileHasBom $hermesConfig)
                            Write-Host "PASS: removed '$serverName' from Hermes config." -ForegroundColor Green
                        }
                    }
                }
            }
        }
    }

    # 4) The indexer service (admin).
    $service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($service) {
        $helperCandidates = @()
        if ($versionRoot) { $helperCandidates += (Join-Path (Join-Path $versionRoot 'indexer') 'register-indexer-service.ps1') }
        if ($repoRoot) { $helperCandidates += (Join-Path $repoRoot 'scripts\register-indexer-service.ps1') }
        $helper = @($helperCandidates | Where-Object { Test-Path -LiteralPath $_ }) | Select-Object -First 1
        if ($DryRun) {
            Write-Action "Stop and delete service '$serviceName'"
        } elseif ($NonInteractive -and -not $elevated) {
            Write-Host 'WARN: the indexer service needs administrator rights. Run this elevated:' -ForegroundColor Yellow
            if ($helper) { Write-Host "  powershell -NoProfile -ExecutionPolicy Bypass -File `"$helper`" -Uninstall -ServiceName $serviceName" -ForegroundColor Gray }
            else { Write-Host "  sc.exe delete $serviceName" -ForegroundColor Gray }
            $script:incomplete = $true
        } else {
            $helperArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$helper`"", '-Uninstall', '-ServiceName', $serviceName)
            $removeProcess = if ($elevated) {
                Start-Process -FilePath powershell.exe -ArgumentList $helperArgs -PassThru -NoNewWindow -Wait
            } else {
                Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList $helperArgs -PassThru
            }
            if ($elevated -and $removeProcess.ExitCode -ne 0) {
                Write-Host "WARN: service removal helper exited with code $($removeProcess.ExitCode)." -ForegroundColor Yellow
                $script:incomplete = $true
            }
        }
    }

    # 5) Installed files.
    if (Test-Path -LiteralPath $InstallRoot) {
        if ($DryRun) {
            Write-Action "Remove install directory '$InstallRoot'"
        } else {
            try {
                Remove-Item -LiteralPath $InstallRoot -Recurse -Force -ErrorAction Stop
                Write-Host "PASS: removed '$InstallRoot'." -ForegroundColor Green
            } catch {
                Write-Host "WARN: could not remove '$InstallRoot': $($_.Exception.Message). Close the AI app that runs the server and run -Uninstall again." -ForegroundColor Yellow
                $script:incomplete = $true
            }
        }
    }

    $dataDir = Join-Path $env:PROGRAMDATA 'ClayLeopardLabs\instant-file-search'
    if (Test-Path -LiteralPath $dataDir) {
        Write-Host "NOTE: the index database is still at '$dataDir'; remove it as administrator if you want that space back (sc.exe delete the service first if it is running)." -ForegroundColor Gray
    }

    if ($DryRun) {
        Write-Host "`nDRY RUN complete: nothing was removed." -ForegroundColor Yellow
        return 0
    }
    if ($script:incomplete) {
        Write-Host "`nUninstall finished, but the items above still need attention." -ForegroundColor Yellow
        return 2
    }
    Write-Host "`nUninstall complete." -ForegroundColor Green
    return 0
}

# Pure-function regression tests for the patch editor; never touches a real profile.
if ($SelfTest) { exit (Invoke-InstallerSelfTest) }

Write-Host 'Instant File Search MCP installer' -ForegroundColor Green
Write-Host "Install root: $InstallRoot" -ForegroundColor Gray
if ($isCheckout) { Write-Host 'Source: local checkout' -ForegroundColor Gray }
else { Write-Host "Source: GitHub release ($ReleaseBase)" -ForegroundColor Gray }

$elevated = Test-Elevated
if ($Uninstall) {
    $uninstallClients = if ($Clients -and ($Clients -notcontains 'all')) { @($Clients) } else { @(Get-DetectedClients) }
    if ($Clients -contains 'all') { $uninstallClients = @(Get-DetectedClients) }
    exit (Invoke-Uninstall -UninstallClients $uninstallClients)
}

$selected = @(Select-InstallClients)
if ($SkipCodex) { $selected = @($selected | Where-Object { $_ -ne 'codex' }) }
if ($SkipOpenCode) { $selected = @($selected | Where-Object { $_ -ne 'opencode' }) }
if ($SkipClaude) { $selected = @($selected | Where-Object { $_ -ne 'claude' }) }
if ($SkipHermes) { $selected = @($selected | Where-Object { $_ -ne 'hermes' }) }
if ($SkipDsh) { $selected = @($selected | Where-Object { $_ -ne 'dsh' }) }

# Always install the server, engines, and diagnostics even when no supported
# MCP client was detected - a user may configure any MCP host manually.
if (-not $selected) {
    Write-Host 'No MCP clients detected (Codex, OpenCode, Claude Desktop, Hermes, or DeepSeek Harness).' -ForegroundColor Yellow
    Write-Host 'The MCP server and engines will still be installed so you can configure any host manually.' -ForegroundColor Yellow
} else {
    Write-Host "Selected: $($selected -join ', ')" -ForegroundColor Green
}

Write-Step 'Obtaining the MCP server binary'
$serverSource = Resolve-ServerBinary
Initialize-VersionedInstall $serverSource
if ($DryRun) {
    Write-Action "Copy '$serverSource' to '$stableBinary'"
} elseif ($serverSource -eq $stableBinary) {
    Write-Host "PASS: MCP server $installVersion is already in place at '$stableBinary'." -ForegroundColor Green
} elseif (Test-Path -LiteralPath $stableBinary) {
    # A same-version reinstall (a rebuilt binary, or a plain re-run) must not fail
    # just because DeepSeek Harness respawns the MCP server as soon as it is
    # stopped, which keeps the destination exe locked. Identical bytes need no copy.
    $sameContent = $false
    try {
        $sourceHash = (Get-FileHash -LiteralPath $serverSource -Algorithm SHA256).Hash
        $targetHash = (Get-FileHash -LiteralPath $stableBinary -Algorithm SHA256).Hash
        $sameContent = ($sourceHash -eq $targetHash)
    } catch {}
    if ($sameContent) {
        Write-Host "PASS: MCP server $installVersion is already current at '$stableBinary' (identical binary; copy skipped while a client may hold it)." -ForegroundColor Green
    } else {
        Install-ServerBinary $serverSource $stableBinary
        Write-Host "PASS: MCP server $installVersion installed at '$stableBinary'." -ForegroundColor Green
    }
} else {
    Install-ServerBinary $serverSource $stableBinary
    Write-Host "PASS: MCP server $installVersion installed at '$stableBinary'." -ForegroundColor Green
}

Write-Step 'Deploying the Fallback Engine'
$fallbackChoice = Resolve-FallbackEngineChoice -NoPrompt:$DryRun
if ($DryRun) {
    if ($fallbackChoice -eq 'include') { Write-Action "Deploy the optional Fallback Engine into '$(Join-Path $versionRoot 'everything')'" }
    else { Write-Action 'Skip the optional Fallback Engine (pass -IncludeFallbackEngine to add it)' }
} elseif ($fallbackChoice -eq 'include') {
    Deploy-BundledEngine
} else {
    Write-Host 'INFO: Fallback Engine skipped. The native indexer handles all searches. Re-run with -IncludeFallbackEngine to add it.' -ForegroundColor Cyan
}

foreach ($client in $selected) {
    switch ($client) {
        'codex' { Install-Codex }
        'opencode' { Install-OpenCode }
        'claude' { Install-Claude }
        'hermes' { Install-Hermes }
        'dsh' { Install-Dsh }
    }
}


Install-NativeService

# A service that just started may still be building its first index; wait for
# the pipe so verification and the user both know when the native engine is live.
if (-not $DryRun -and $NativeWaitSec -gt 0) {
    $svcNow = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
    if ($svcNow -and $svcNow.Status -eq 'Running') {
        Write-Host "Waiting up to $NativeWaitSec s for the native indexer to start serving (the first build after an upgrade can take minutes)..." -ForegroundColor Gray
        if (Wait-NativeIndexer -TimeoutSec $NativeWaitSec) {
            Write-Host 'PASS: native indexer is accepting connections.' -ForegroundColor Green
        } else {
            Write-Host 'NOTE: the native indexer is still building its index; searches answer from the Fallback Engine until it is ready.' -ForegroundColor Gray
        }
    }
}

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

# Keep the plain "stable install" copy current: docs/build.md and the OpenCode
# plugin's resolution order both name it, and it used to stay frozen at whatever
# the first install left there. Then drop this run's downloads so repeated
# installs stop accumulating scratch - a handful of upgrades left ~70 MB behind.
if (-not $DryRun) {
    $rootBinary = Join-Path $InstallRoot $binaryName
    if ($stableBinary -and (Test-Path -LiteralPath $stableBinary) -and $rootBinary -ne $stableBinary) {
        try {
            Copy-Item -LiteralPath $stableBinary -Destination $rootBinary -Force -ErrorAction Stop
            Write-Host "PASS: refreshed the stable copy at '$rootBinary'." -ForegroundColor Green
        } catch {
            Write-Host "WARN: could not refresh '$rootBinary' ($($_.Exception.Message)); a client may be running it." -ForegroundColor Yellow
        }
    }
    $downloadsDir = Join-Path $InstallRoot 'downloads'
    if (Test-Path -LiteralPath $downloadsDir) {
        # Each run drops its own directory under downloads\<pid>. A directory
        # whose PID is no longer running belongs to a finished install and can
        # go immediately; the age check only covers reused PIDs.
        Get-ChildItem -LiteralPath $downloadsDir -Directory -ErrorAction SilentlyContinue |
            Where-Object {
                if ($_.FullName -eq $downloadRoot) { return $true }
                $owner = 0
                $alive = [int]::TryParse($_.Name, [ref]$owner) -and (Get-Process -Id $owner -ErrorAction SilentlyContinue)
                (-not $alive) -or $_.LastWriteTime -lt (Get-Date).AddDays(-1)
            } |
            ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

if (-not $DryRun) {
    $staleRefs = @(Get-StaleClientReferences -ExpectedBinary $stableBinary)
    foreach ($staleRef in $staleRefs) {
        Write-Host "WARN: $($staleRef.Client): $($staleRef.Detail); re-run with -Clients $($staleRef.Client) to update it." -ForegroundColor Yellow
    }
    if ($PruneOldVersions) {
        $versionsDir = Join-Path $InstallRoot 'versions'
        if (Test-Path -LiteralPath $versionsDir) {
            $oldVersions = @(Get-ChildItem -LiteralPath $versionsDir -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne $installVersion })
            if ($oldVersions.Count -eq 0) {
                Write-Host 'PASS: no old version directories to prune.' -ForegroundColor Green
            } else {
                foreach ($oldVersion in $oldVersions) {
                    try {
                        Remove-Item -LiteralPath $oldVersion.FullName -Recurse -Force -ErrorAction Stop
                        Write-Host "PASS: pruned old version '$($oldVersion.Name)'." -ForegroundColor Green
                    } catch {
                        Write-Host "WARN: could not prune '$($oldVersion.Name)': $($_.Exception.Message)" -ForegroundColor Yellow
                    }
                }
            }
        }
    }
}
if ($DryRun -and $PruneOldVersions) {
    $oldVersions = @(Get-ChildItem -LiteralPath (Join-Path $InstallRoot 'versions') -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne $installVersion })
    foreach ($oldVersion in $oldVersions) { Write-Action "Prune old version directory '$($oldVersion.Name)'" }
}
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
if ($selected -contains 'dsh' -and (Test-DshHostRunning)) {
    Write-Host 'DeepSeek Harness is running: this profile patch is hot-applied, so the new tools appear on the next agent turn. Restart it only if they do not.'
} else {
    Write-Host 'Start (or restart) your AI app so it reloads the MCP configuration.'
}

if ($DryRun) {
    Write-Host "`nDRY RUN complete: no files, services, or client configs were changed." -ForegroundColor Yellow
    exit 0
}

if (-not $verifyOk) {
    Write-Host "`nThe installer finished, but the post-install verification failed. Run the diagnostics script for details:" -ForegroundColor Red
    Write-Host "  $((Join-Path $InstallRoot $doctorName))" -ForegroundColor Gray
    exit 1
}if ($incomplete) {
    Write-Host "`nInstallation is complete, but the native indexer service still needs one elevated run (see the command above). Searches work now through the Fallback Engine." -ForegroundColor Yellow
    exit 2
}
exit 0
