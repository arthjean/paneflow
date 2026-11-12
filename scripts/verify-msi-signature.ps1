[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Tag
)

$ErrorActionPreference = 'Stop'

$version = $Tag -replace '^v', ''
if ($version -notmatch '^\d+\.\d+\.\d+([.-][0-9A-Za-z.-]+)?$') {
    throw "TAG '$Tag' does not strip to a valid semver (got '$version')."
}
$candidates = @(Get-ChildItem -Path 'target/wix' -Filter 'paneflow-*.msi' -File)
if ($candidates.Count -ne 1) {
    $names = if ($candidates.Count -gt 0) { ($candidates | ForEach-Object { $_.Name }) -join ', ' } else { '<none>' }
    throw "Expected exactly one MSI under target/wix/ after cargo wix, found $($candidates.Count): $names"
}
$msi = $candidates[0].FullName

$sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\bin'
$signtool = $null
if (Test-Path -LiteralPath $sdkRoot) {
    $signtool = Get-ChildItem -Path $sdkRoot -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\' } |
        Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0.0.0' } } -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if ([string]::IsNullOrEmpty($signtool)) {
    $onPath = Get-Command -Name signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $onPath) { $signtool = $onPath.Source }
}
if ([string]::IsNullOrEmpty($signtool)) {
    throw "signtool.exe not found."
}

Write-Output "Verifying: $msi"
$verifyOutput = & $signtool verify /pa /v "$msi" 2>&1
$verifyExit = $LASTEXITCODE
$verifyOutput | ForEach-Object { Write-Output $_ }
if ($verifyExit -ne 0) {
    throw "signtool verify exit code $verifyExit"
}
if (-not ($verifyOutput -match '^\s*Successfully verified')) {
    throw "signtool verify did not emit 'Successfully verified' - AC-6 sentinel missing."
}
Write-Output "AC-6 sentinel 'Successfully verified' present."
