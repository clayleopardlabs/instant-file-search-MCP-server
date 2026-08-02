param(
    [string]$Destination = ""
)

# Fetches and verifies the Everything portable zip used by the self-contained
# MCP bundle. Everything is MIT-licensed (see LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt), so
# redistribution with the license notice is permitted.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts/fetch-everything.ps1
# Defaults to writing into vendor\everything\ (next to this script).

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Destination) {
    $Destination = Join-Path (Split-Path -Parent $scriptDir) "vendor\everything"
}

$url = "https://www.voidtools.com/Everything-1.5.0.1418b.x64.zip"
$expectedSha256 = "2240F7055D772983DA5AD3A433DBB9250C501CCB3E835451F76D29FE121C1571"
$target = Join-Path $Destination "instant-file-search-fallback-engine-1.5.0.1418b.zip"

New-Item -ItemType Directory -Path $Destination -Force | Out-Null

Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $target -UseBasicParsing

$actual = (Get-FileHash -Path $target -Algorithm SHA256).Hash
if ($actual -ne $expectedSha256) {
    Remove-Item -Path $target -Force
    throw "SHA256 mismatch. Expected $expectedSha256, got $actual. " +
          "The upstream file changed - review before updating the pinned hash."
}

Write-Host "OK: $target ($actual)"
Write-Host "Remember: LICENSE-instant-file-search-fallback-engine-1.5.0.1418b.txt must be distributed alongside Everything.exe."
Write-Host "Saved as: instant-file-search-fallback-engine-1.5.0.1418b.zip"
