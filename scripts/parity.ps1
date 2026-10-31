<#
.SYNOPSIS
Runs the differential parity battery: identical queries through the native
indexer pipe and the Everything engine, comparing counts.

.DESCRIPTION
The parity tests are #[ignore]d in normal runs because they need a live
indexer service and a reachable Everything engine, which CI cannot provide.
They are the only check that catches "the two engines disagree", so they belong
in a pre-release pass rather than in someone's memory.

This wrapper checks the preconditions first, so a failure reads as "the engine
is not up" instead of a wall of test output.

.PARAMETER Debug
Run the debug profile instead of release (much slower for the scan counts).

.EXAMPLE
.\scripts\parity.ps1
#>
[CmdletBinding()]
param(
    [switch]$Debug
)

$ErrorActionPreference = 'Stop'

if (-not $IsWindows -and $PSVersionTable.PSVersion.Major -ge 6) {
    throw 'The parity battery compares the native indexer with the Everything engine, which is Windows-only.'
}

$service = Get-Service -Name 'instant-file-search-indexer' -ErrorAction SilentlyContinue
if (-not $service -or $service.Status -ne 'Running') {
    throw "The 'instant-file-search-indexer' service is not running; the native half of the comparison needs it. Start it (elevated) and retry."
}

$pipeFound = [System.IO.Directory]::GetFiles('\\.\pipe\') -match 'instant-file-search'
if (-not $pipeFound) {
    throw 'The indexer pipe is not present yet: the service is probably still building its index after a start. Retry in a minute.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    $profileArg = if ($Debug) { @() } else { @('--release') }
    Write-Host 'Running the parity battery (native indexer vs Everything)...' -ForegroundColor Cyan
    & cargo test --package instant-file-search-mcp-server @profileArg -- --ignored parity --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Parity battery exited $LASTEXITCODE. Mismatches are reported rather than failed, so a non-zero exit means the harness itself broke."
    }
    Write-Host 'Parity battery completed. Review the per-query counts above for mismatches.' -ForegroundColor Green
}
finally {
    Pop-Location
}
