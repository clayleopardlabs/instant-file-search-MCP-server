param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath
)

$ErrorActionPreference = 'Stop'
$log = "C:\Windows\Temp\deploy-indexer.log"

"[$([DateTime]::Now.ToString('HH:mm:ss'))] Starting deploy from $BinaryPath" | Out-File $log -Encoding utf8

try {
    $serviceDir = Join-Path $env:LOCALAPPDATA 'ClayLeopardLabs\instant-file-search\indexer'
    $target = Join-Path $serviceDir 'instant-file-search-indexer.exe'

    "Service dir: $serviceDir" | Out-File $log -Append -Encoding utf8
    if (-not (Test-Path $serviceDir)) {
        throw "Service dir missing: $serviceDir"
    }

    "Stopping service instant-file-search-indexer..." | Out-File $log -Append -Encoding utf8
    Stop-Service -Name 'instant-file-search-indexer' -Force -ErrorAction Stop
    Start-Sleep -Seconds 2

    "Copying binary..." | Out-File $log -Append -Encoding utf8
    Copy-Item -Path $BinaryPath -Destination $target -Force

    "Starting service..." | Out-File $log -Append -Encoding utf8
    Start-Service -Name 'instant-file-search-indexer' -ErrorAction Stop

    $svc = Get-Service -Name 'instant-file-search-indexer'
    $stamp = (Get-Item $target).LastWriteTime.ToString('MM/dd HH:mm:ss')
    "Deploy OK. Service state: $($svc.Status). Binary: $stamp" | Out-File $log -Append -Encoding utf8
}
catch {
    "DEPLOY FAILED: $($_.Exception.Message)" | Out-File $log -Append -Encoding utf8
    throw
}
