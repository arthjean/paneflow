#Requires -Version 7
param(
    [switch]$Release,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$AppArgs
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
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
