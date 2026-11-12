[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Tag
)

$ErrorActionPreference = 'Stop'

$version = $Tag -replace '^v', ''
if ($version -notmatch '^\d+\.\d+\.\d+([.-][0-9A-Za-z.-]+)?$') {
    throw "TAG '$Tag' does not strip to a valid semver (got '$version'). Refusing to glob for MSI."
}
$candidates = @(Get-ChildItem -Path 'target/wix' -Filter 'paneflow-*.msi' -File)
if ($candidates.Count -ne 1) {
    $names = if ($candidates.Count -gt 0) { ($candidates | ForEach-Object { $_.Name }) -join ', ' } else { '<none>' }
    throw "Expected exactly one MSI under target/wix/ after cargo wix, found $($candidates.Count): $names"
}
$msi = $candidates[0].FullName
Write-Output "Signing: $msi"
pwsh -NoProfile -File ./scripts/sign-windows.ps1 -InputFile "$msi"
if ($LASTEXITCODE -ne 0) { throw "Signing failed for $msi (exit $LASTEXITCODE)" }
