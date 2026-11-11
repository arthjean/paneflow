#Requires -Version 7
<#
.SYNOPSIS
Loads the Visual Studio developer environment into the following workflow steps.

.DESCRIPTION
Finds the latest Visual Studio installation that carries the MSVC x86/x64 build tools, runs its
vcvarsall.bat for the requested architecture, and exports every variable that vcvarsall.bat adds
or changes. Each new PATH entry goes to $env:GITHUB_PATH, every other variable to $env:GITHUB_ENV.
Without those two files, the script prints the variables it would export.

.PARAMETER Arch
The vcvarsall.bat architecture argument: x64, x86, arm64, x86_arm64, or amd64_arm64.

.EXAMPLE
./scripts/msvc-dev-env.ps1 -Arch x64
#>
param(
    [ValidateSet('x64', 'x86', 'arm64', 'x86_arm64', 'amd64_arm64')]
    [string]$Arch = 'x64'
)

$ErrorActionPreference = 'Stop'

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw "vswhere.exe was not found at $vswhere"
}
$installations = @(& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if ($LASTEXITCODE -ne 0) {
    throw "vswhere.exe failed with exit code $LASTEXITCODE"
}
$installationPath = $installations | Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($installationPath)) {
    throw 'No Visual Studio installation with the MSVC x86/x64 build tools was found'
}
$vcvarsall = Join-Path $installationPath 'VC\Auxiliary\Build\vcvarsall.bat'
if (-not (Test-Path -LiteralPath $vcvarsall -PathType Leaf)) {
    throw "vcvarsall.bat was not found at $vcvarsall"
}

$before = @{}
foreach ($entry in [Environment]::GetEnvironmentVariables().GetEnumerator()) {
    $before[[string]$entry.Key] = [string]$entry.Value
}

$output = & cmd.exe /d /s /c "`"$vcvarsall`" $Arch >nul 2>&1 && set"
if ($LASTEXITCODE -ne 0) {
    throw "vcvarsall.bat $Arch failed with exit code $LASTEXITCODE"
}

$after = @{}
foreach ($line in $output) {
    if ($line -match '^([^=]+)=(.*)$') {
        $after[$Matches[1]] = $Matches[2]
    }
}
if (-not $after.ContainsKey('VCToolsInstallDir')) {
    throw "vcvarsall.bat $Arch did not set VCToolsInstallDir"
}

$separator = [IO.Path]::PathSeparator
$knownPath = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($dir in ([string]$before['Path']).Split($separator, [StringSplitOptions]::RemoveEmptyEntries)) {
    [void]$knownPath.Add($dir.TrimEnd('\'))
}
$newPath = [Collections.Generic.List[string]]::new()
foreach ($dir in ([string]$after['Path']).Split($separator, [StringSplitOptions]::RemoveEmptyEntries)) {
    if ($knownPath.Add($dir.TrimEnd('\'))) {
        $newPath.Add($dir)
    }
}

$exported = [Collections.Generic.List[string]]::new()
foreach ($name in ($after.Keys | Sort-Object)) {
    if ($name -ieq 'Path') {
        continue
    }
    $value = $after[$name]
    if ($before.ContainsKey($name) -and $before[$name] -ceq $value) {
        continue
    }
    if ($value.Contains("`n")) {
        throw "vcvarsall.bat set $name to a multi-line value, which GITHUB_ENV cannot carry"
    }
    $exported.Add("$name=$value")
}

$reversedPath = [string[]]$newPath.ToArray()
[Array]::Reverse($reversedPath)

if ($env:GITHUB_ENV -and $env:GITHUB_PATH) {
    $exported | Out-File -LiteralPath $env:GITHUB_ENV -Encoding utf8 -Append
    $reversedPath | Out-File -LiteralPath $env:GITHUB_PATH -Encoding utf8 -Append
}
else {
    $exported | ForEach-Object { Write-Output "env: $_" }
    $newPath | ForEach-Object { Write-Output "path: $_" }
}
Write-Host "MSVC $Arch environment from $installationPath`: $($exported.Count) variable(s), $($newPath.Count) PATH entr$(if ($newPath.Count -eq 1) { 'y' } else { 'ies' })"
