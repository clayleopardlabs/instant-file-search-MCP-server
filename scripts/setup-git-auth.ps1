param(
  [string]$RepoRoot = (Split-Path $PSScriptRoot -Parent)
)

$ErrorActionPreference = "Stop"

$helperPath = Join-Path $RepoRoot "scripts\git-credential-helper.ps1"
if (-not (Test-Path -LiteralPath $helperPath)) {
  throw "Credential helper not found at '$helperPath'."
}

git -C $RepoRoot config --local http.sslBackend openssl

$helperCommand = "!powershell -NoProfile -ExecutionPolicy Bypass -File `"$helperPath`""
git -C $RepoRoot config --local credential.helper $helperCommand
git -C $RepoRoot config --local credential.useHttpPath true

Write-Host "Configured repo-local Git auth for GitHub PAT usage." -ForegroundColor Green
Write-Host "If GITHUB_TOKEN or GH_TOKEN is present, future pushes should use it without schannel." -ForegroundColor Yellow
