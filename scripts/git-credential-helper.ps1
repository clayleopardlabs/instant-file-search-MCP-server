param()

$ErrorActionPreference = "Stop"

$fields = @{}
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line -or $line.Length -eq 0) {
    break
  }

  $parts = $line.Split("=", 2)
  if ($parts.Count -eq 2) {
    $fields[$parts[0]] = $parts[1]
  }
}

if ($fields["host"] -ne "github.com") {
  exit 0
}

$token = $env:GITHUB_TOKEN
if ([string]::IsNullOrWhiteSpace($token)) {
  $token = $env:GH_TOKEN
}

if ([string]::IsNullOrWhiteSpace($token)) {
  exit 1
}

Write-Output "username=x-access-token"
Write-Output "password=$token"
