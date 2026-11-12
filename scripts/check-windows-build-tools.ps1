[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

foreach ($tool in @('cl.exe', 'link.exe')) {
    where.exe $tool
    if ($LASTEXITCODE -ne 0) {
        throw "$tool is not on PATH: run scripts/msvc-dev-env.ps1 first"
    }
}

$signtool = Get-Command -Name signtool.exe -ErrorAction SilentlyContinue
if ($null -eq $signtool) {
    $sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\bin'
    if (Test-Path -LiteralPath $sdkRoot) {
        $signtool = Get-ChildItem -Path $sdkRoot -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0.0.0' } } -Descending |
            Select-Object -First 1
    }
}
if ($null -eq $signtool) {
    throw "signtool.exe not found on PATH or under the Windows SDK."
}
$signtoolPath = if ($signtool.PSObject.Properties.Name -contains 'Source') { $signtool.Source } else { $signtool.FullName }
Write-Output "Using signtool: $signtoolPath"
