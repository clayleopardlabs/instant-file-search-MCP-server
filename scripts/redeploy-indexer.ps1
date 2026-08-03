param(
    [string]$Source
)
$logPath = Join-Path $env:TEMP 'redeploy-indexer.log'
function Log([string]$Msg) { $Msg | Add-Content -LiteralPath $logPath }
$ErrorActionPreference = 'Continue'
$serviceName = 'instant-file-search-indexer'
$src = if ($Source) { $Source } else {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    Join-Path $repoRoot 'target\release\instant-file-search-indexer.exe'
}
$dst = Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\instant-file-search\indexer\instant-file-search-indexer.exe'

if (-not (Test-Path -LiteralPath $src)) { Log "FAIL: source binary not found: $src"; exit 1 }
Log "src=$src"
Log "dst=$dst"

$existing = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($existing) {
  Log "service exists, status=$($existing.Status)"
  if ($existing.Status -ne 'Stopped') { Stop-Service -Name $serviceName -Force -ErrorAction Continue; Start-Sleep -Seconds 2 }
}
try {
  Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop
  Log "copied OK"
} catch {
  Log "copy FAILED: $($_.Exception.Message)"
  exit 1
}
if ($existing) {
  Start-Service -Name $serviceName -ErrorAction Continue
  Start-Sleep -Seconds 1
}
$check = Get-Item -LiteralPath $dst
Log ("deployed timestamp: " + $check.LastWriteTime)
