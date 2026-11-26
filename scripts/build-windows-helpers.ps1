[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Target
)

$ErrorActionPreference = 'Stop'

$cargo = if ([string]::IsNullOrEmpty($env:CARGO)) { 'cargo' } else { $env:CARGO }
& $cargo build `
  --profile release-min `
  --locked `
  --target $Target `
  --target-dir target/embed-build `
  -p paneflow-shim `
  -p paneflow-ai-hook
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
