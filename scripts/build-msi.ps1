[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Version
)

$ErrorActionPreference = 'Stop'

$version = $Version -replace '^v', ''
if ($version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$') {
    throw "Windows MSI ProductVersion must be valid semver, got '$version'"
}
cargo wix `
  --nocapture `
  --no-build `
  --package paneflow-app `
  --target x86_64-pc-windows-msvc `
  --install-version $version `
  --include packaging/wix/main.wxs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Get-ChildItem -LiteralPath target/wix
