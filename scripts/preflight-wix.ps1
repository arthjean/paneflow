[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$wixRoots = @(
    'C:\Program Files (x86)\WiX Toolset v3.14\bin',
    'C:\Program Files (x86)\WiX Toolset v3.11\bin'
)
$wixBin = $wixRoots | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $wixBin) {
    throw "WiX v3 not found under any known path. cargo-wix 0.3.9 requires WiX v3 (candle.exe/light.exe). Runner image may have dropped the preinstalled toolset - add an explicit install step (choco install wixtoolset --version 3.14.1)."
}
if ($wixBin -match 'v3\.11') {
    Write-Warning "Falling back to WiX v3.11 - windows-2022 image normally ships v3.14. Verify runner-image changelog for a silent WiX downgrade."
}
Write-Output "Using WiX: $wixBin"
Add-Content -Path $env:GITHUB_PATH -Value $wixBin
