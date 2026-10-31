<#
.SYNOPSIS
Installs, updates, or repoints the instant-file-search-indexer Windows service.

.DESCRIPTION
This is the admin-only half of scripts\install.ps1. It used to be generated at
run time from a here-string, which made it the only part of the installer that
could not be read, reviewed, or syntax-checked before executing it elevated.

The caller stops nothing: this script stops the service (waiting for it to
settle rather than trusting a single Stop-Service call), copies the indexer
binary into place, points the service at it, restores the mode environment
variables, and starts it again.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\register-indexer-service.ps1 `
    -SourceIndexer C:\dl\instant-file-search-indexer.exe `
    -ServiceIndexer C:\...\versions\1.0.7\indexer\instant-file-search-indexer.exe

.EXAMPLE
# Parameter and shape check with no admin rights and no side effects (CI).
pwsh -File .\register-indexer-service.ps1 -Validate -SourceIndexer x -ServiceIndexer y
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$SourceIndexer,
    [Parameter(Mandatory = $true)][string]$ServiceIndexer,
    [string]$ServiceName = 'instant-file-search-indexer',
    [ValidateSet('memory', 'disk')][string]$IndexMode = 'disk',
    [ValidateSet('auto', 'off', 'memory', 'disk')][string]$ContentMode = 'auto',
    [switch]$Validate
)

$ErrorActionPreference = 'Stop'

if ($Validate) {
    # No service contact, no elevation: lets CI prove the script parses and its
    # parameters bind, which the generated helper could never do.
    "validate: ok"
    "service:  $ServiceName"
    "source:   $SourceIndexer"
    "target:   $ServiceIndexer"
    "modes:    index=$IndexMode content=$ContentMode"
    exit 0
}

foreach ($path in @($SourceIndexer, $ServiceIndexer)) {
    if (-not (Test-Path -LiteralPath $path) -and -not (Test-Path -LiteralPath (Split-Path -Parent $path))) {
        throw "Neither '$path' nor its parent directory exists."
    }
}

# cmd.exe is used for sc.exe because PowerShell strips nested quotes from
# native command arguments.
$configCommand = 'sc.exe config "' + $ServiceName + '" binPath= "\"' + $ServiceIndexer + '\" service" start= auto'
$createCommand = 'sc.exe create "' + $ServiceName + '" binPath= "\"' + $ServiceIndexer + '\" service" start= auto'

$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    if ($existing.Status -ne 'Stopped') {
        # A stop that slightly exceeds the SCM window throws even though it
        # lands a moment later, which used to abort whole upgrades. Ask, then
        # wait for the state to settle rather than trusting one call.
        try { Stop-Service -Name $ServiceName -Force -ErrorAction Stop } catch { }
        $deadline = (Get-Date).AddSeconds(60)
        while ((Get-Service -Name $ServiceName -ErrorAction SilentlyContinue).Status -ne 'Stopped' -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 500
        }
        if ((Get-Service -Name $ServiceName -ErrorAction SilentlyContinue).Status -ne 'Stopped') {
            throw "Service $ServiceName did not stop within 60 seconds. Close the AI apps using it and run the installer again."
        }
    }
    # Copy after the stop: Windows holds a running image open.
    Copy-Item -LiteralPath $SourceIndexer -Destination $ServiceIndexer -Force
    & cmd.exe /d /c $configCommand | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe config failed for service $ServiceName." }
} else {
    Copy-Item -LiteralPath $SourceIndexer -Destination $ServiceIndexer -Force
    & cmd.exe /d /c $createCommand | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe create failed for service $ServiceName." }
}

$environment = @("INSTANT_FS_INDEX_MODE=$IndexMode", "INSTANT_FS_CONTENT_INDEX=$ContentMode")
$serviceKey = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
$oldEnvironment = @((Get-ItemProperty -Path $serviceKey -Name Environment -ErrorAction SilentlyContinue).Environment)
$preserved = @($oldEnvironment | Where-Object { $_ -and $_ -notmatch '^INSTANT_FS_(INDEX_MODE|CONTENT_INDEX)=' })
New-ItemProperty -Path $serviceKey -Name Environment -PropertyType MultiString -Value (@($preserved + $environment)) -Force | Out-Null

Start-Service -Name $ServiceName
$started = Get-Service -Name $ServiceName
$started.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(120))
Write-Host "Service '$ServiceName' is running '$ServiceIndexer'."
