#Requires -Version 7
param(
    [switch]$Release,
    [string]$Tag = '',
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$AppArgs
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if ($PSBoundParameters.ContainsKey('Tag')) {
    $slug = ($Tag.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-')
    if (-not $slug) {
        [Console]::Error.WriteLine("dev.ps1: -Tag '$Tag' has no letter or digit")
        exit 2
    }
    $env:PANEFLOW_HOME = Join-Path $HOME ".paneflow-dev-$slug"
    [Console]::Error.WriteLine("dev.ps1: tag '$slug' runs on its own state home $env:PANEFLOW_HOME")
}
& (Join-Path $PSScriptRoot 'fetch-conpty.ps1')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$profileArgs = @()
$profileDir = 'debug'
if ($Release) {
    $profileArgs = @('--release')
    $profileDir = 'release'
}
cargo build -p paneflow-app -p paneflow-host --locked @profileArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& (Join-Path $root "target\$profileDir\paneflow.exe") @AppArgs
exit $LASTEXITCODE
