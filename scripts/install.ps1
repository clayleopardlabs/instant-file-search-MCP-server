param(
  [ValidateSet("OpenCode", "Codex", "Both")]
  [string]$Client = "OpenCode",
  [string]$InstallDir,
  [string]$BinaryUrl = "https://github.com/clayleopardlabs/instantaneous-windows-file-search-mcp-server/releases/latest/download/instantaneous-windows-file-search-mcp-server.exe",
  [string]$ExpectedSha256
)

$ErrorActionPreference = "Stop"

$openCodeInstallDir = Join-Path $env:USERPROFILE ".config\opencode\tools"
$neutralInstallDir = Join-Path $env:LOCALAPPDATA "ClayLeopardLabs\instantaneous-file-search"
$serverName = "instantaneous-file-search"

$installForOpenCode = $Client -in @("OpenCode", "Both")
$installForCodex = $Client -in @("Codex", "Both")

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
  # Keep the existing OpenCode location unless a non-OpenCode client is selected.
  $InstallDir = if ($installForCodex) { $neutralInstallDir } else { $openCodeInstallDir }
}

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

if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256)) {
  $actualSha256 = (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash
  if ($actualSha256 -ne $ExpectedSha256.ToUpperInvariant()) {
    Remove-Item -LiteralPath $out -Force
    throw "SHA-256 verification failed. Expected '$ExpectedSha256', got '$actualSha256'."
  }
  Write-Host "   SHA-256 verified." -ForegroundColor Green
}

Write-Host "   Binary saved to $out" -ForegroundColor Green

if ($installForCodex) {
  $codex = Get-Command codex.exe -ErrorAction SilentlyContinue
  if (-not $codex) {
    $codex = Get-Command codex -ErrorAction SilentlyContinue
  }

  if ($codex) {
    Write-Host ":: Registering MCP server with Codex..." -ForegroundColor Cyan
    & $codex.Source mcp add $serverName -- $out
    if ($LASTEXITCODE -ne 0) {
      Write-Warning "Codex registration failed. Register it manually with: codex mcp add $serverName -- \"$out\""
    } else {
      Write-Host "   Codex registration complete." -ForegroundColor Green
    }
  } else {
    Write-Warning "Codex was not found on PATH. Register it later with: codex mcp add $serverName -- \"$out\""
  }
}

Write-Host ""
Write-Host "Installation complete." -ForegroundColor Green
Write-Host "Binary: $out" -ForegroundColor Gray

if ($installForOpenCode) {
  Write-Host "Add this to your opencode.json mcp section:" -ForegroundColor Yellow
  Write-Host "  ""everything"": {" -ForegroundColor Gray
  Write-Host "    ""type"": ""local""," -ForegroundColor Gray
  Write-Host "    ""command"": [""$out""]," -ForegroundColor Gray
  Write-Host "    ""enabled"": true" -ForegroundColor Gray
  Write-Host "  }" -ForegroundColor Gray
}

if ($installForCodex) {
  Write-Host "Verify the Codex registration with: codex mcp list" -ForegroundColor Yellow
  Write-Host "Restart Codex or start a new task if the tools do not appear immediately." -ForegroundColor Gray
}
