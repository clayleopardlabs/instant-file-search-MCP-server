param(
  [string]$InstallDir = "$env:USERPROFILE\.config\opencode\tools",
  [string]$BinaryUrl = "https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server/releases/latest/download/instantaneous-windows-file-search-mcp-server.exe"
)

$ErrorActionPreference = "Stop"

Write-Host ":: Installing Everything by Voidtools..." -ForegroundColor Cyan
try {
  winget install voidtools.Everything --accept-source-agreements --silent
  Write-Host "   Everything installed." -ForegroundColor Green
} catch {
  Write-Host "   Everything already installed or winget unavailable." -ForegroundColor Yellow
}

Write-Host ":: Downloading MCP server binary..." -ForegroundColor Cyan
if (-not (Test-Path $InstallDir)) {
  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}
$out = Join-Path $InstallDir "instant-search-mcp-server.exe"
Invoke-WebRequest -Uri $BinaryUrl -OutFile $out -UseBasicParsing
Write-Host "   Binary saved to $out" -ForegroundColor Green

Write-Host ""
Write-Host "Done. Add this to your opencode.json mcp section:" -ForegroundColor Yellow
Write-Host "  ""everything"": {" -ForegroundColor Gray
Write-Host "    ""type"": ""local""," -ForegroundColor Gray
Write-Host "    ""command"": [""$out""]," -ForegroundColor Gray
Write-Host "    ""enabled"": true" -ForegroundColor Gray
Write-Host "  }" -ForegroundColor Gray
