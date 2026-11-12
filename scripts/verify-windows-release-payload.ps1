[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Target
)

$ErrorActionPreference = 'Stop'

$report = 'target\libghostty-windows-release\provenance.json'
$exe = "target\$Target\release\paneflow.exe"
pwsh -NoProfile -File ./scripts/verify-libghostty-windows.ps1 `
  -Target $Target `
  -Binary $exe `
  -ReportPath $report `
  -AllowGeneratedIcons
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$provenance = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
if ($provenance.linkage -ne 'static-candidate' -or
    $provenance.runtime_smoke_verified -ne $false -or
    [string]::IsNullOrWhiteSpace($provenance.binary_sha256)) {
    throw 'libghostty release provenance did not produce the expected pre-smoke static candidate'
}
$serialized = Get-Content -LiteralPath $report -Raw
if ($serialized -match '(?i)[a-z]:\\') {
    throw 'libghostty release provenance must not contain runner-local absolute paths'
}
