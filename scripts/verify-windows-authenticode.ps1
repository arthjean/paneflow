[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Target
)

$ErrorActionPreference = 'Stop'

$releaseDir = Join-Path 'target' "$Target/release"
$embedDir = Join-Path 'src-app/target/embed/bin' $Target
$helperDir = Join-Path $releaseDir 'paneflow-helpers'
$files = [System.Collections.Generic.List[string]]::new()
$files.Add((Join-Path $releaseDir 'paneflow.exe'))
$files.Add((Join-Path $releaseDir 'paneflow-host.exe'))
foreach ($name in @('paneflow-shim.exe', 'paneflow-ai-hook.exe', 'paneflow-mcp.exe')) {
    $files.Add((Join-Path $embedDir $name))
}
$packaged = @(Get-ChildItem -LiteralPath $helperDir -Filter '*.exe' -File -ErrorAction SilentlyContinue)
if ($packaged.Count -eq 0) {
    throw "No packaged helper executable found under $helperDir"
}
foreach ($helper in $packaged) { $files.Add($helper.FullName) }
$msis = @(Get-ChildItem -Path 'target/wix' -Filter 'paneflow-*.msi' -File)
if ($msis.Count -ne 1) {
    throw "Expected exactly one MSI under target/wix/, found $($msis.Count)"
}
$files.Add($msis[0].FullName)

$failures = [System.Collections.Generic.List[string]]::new()
foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        $failures.Add("$file (missing)")
        continue
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    Write-Output "$($signature.Status) $file"
    if ($signature.Status -ne 'Valid') {
        $failures.Add("$file ($($signature.Status): $($signature.StatusMessage))")
    }
}
if ($failures.Count -gt 0) {
    foreach ($failure in $failures) { Write-Output "::error::Not validly signed: $failure" }
    throw "$($failures.Count) of $($files.Count) Windows file(s) are not validly signed"
}
Write-Output "All $($files.Count) Windows executables and the MSI carry a valid Authenticode signature."
