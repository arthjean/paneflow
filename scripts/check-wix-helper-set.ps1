[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Target
)

$ErrorActionPreference = 'Stop'

$helperDir = Join-Path (Join-Path 'target' $Target) 'release/paneflow-helpers'
if (-not (Test-Path -LiteralPath $helperDir -PathType Container)) {
    throw "Expected helper directory missing: $helperDir"
}

$ns = New-Object System.Xml.XmlNamespaceManager((New-Object System.Xml.NameTable))
$ns.AddNamespace('w', 'http://schemas.microsoft.com/wix/2006/wi')
$xml = New-Object System.Xml.XmlDocument
$xml.Load((Resolve-Path -LiteralPath 'packaging/wix/main.wxs').Path)
$wixHelpers = @(
    $xml.SelectNodes("//w:Component[@Id='HelperBinaries']/w:File", $ns) |
        ForEach-Object { $_.GetAttribute('Name') } |
        Sort-Object
)
$diskHelpers = @(
    Get-ChildItem -LiteralPath $helperDir -Filter '*.exe' -File |
        ForEach-Object { $_.Name } |
        Sort-Object
)
$diff = Compare-Object -ReferenceObject $wixHelpers -DifferenceObject $diskHelpers
if ($null -ne $diff) {
    $diff | Format-Table | Out-String | Write-Output
    throw "WiX HelperBinaries entries do not match prepared paneflow-helpers files."
}
