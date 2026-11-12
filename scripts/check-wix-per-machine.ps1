[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$ns = New-Object System.Xml.XmlNamespaceManager((New-Object System.Xml.NameTable))
$ns.AddNamespace('w', 'http://schemas.microsoft.com/wix/2006/wi')
$xml = New-Object System.Xml.XmlDocument
$xml.Load((Resolve-Path -LiteralPath 'packaging/wix/main.wxs').Path)
$package = $xml.SelectSingleNode('//w:Package', $ns)
if ($null -eq $package) {
    throw "WiX Package element not found."
}
if ($package.GetAttribute('InstallScope') -eq 'perMachine') {
    $hkcuKeyPaths = @($xml.SelectNodes("//w:Component[.//w:RegistryValue[@Root='HKCU' and @KeyPath='yes']]", $ns))
    $invalid = @($hkcuKeyPaths | Where-Object {
        $hasShortcut = $_.SelectNodes(".//w:Shortcut", $ns).Count -gt 0
        $hasMachineFile = $_.SelectNodes(".//w:File", $ns).Count -gt 0
        $hasMachineEnvironment = $_.SelectNodes(".//w:Environment", $ns).Count -gt 0
        $hasMachineRegistry = $_.SelectNodes(".//w:RegistryValue[@Root='HKLM']", $ns).Count -gt 0
        -not ($hasShortcut -and -not ($hasMachineFile -or $hasMachineEnvironment -or $hasMachineRegistry))
    })
    if ($invalid.Count -gt 0) {
        $ids = @($invalid | ForEach-Object { $_.Id }) -join ', '
        throw "perMachine MSI may use HKCU KeyPath only for shortcut-only components. Offending component(s): $ids"
    }
}
