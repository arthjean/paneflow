[CmdletBinding()]
param(
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$crate = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../crates/paneflow-highlights'))
$manifestPath = Join-Path $crate 'c/MANIFEST.toml'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "Missing $manifestPath" }
$manifestText = [IO.File]::ReadAllText($manifestPath)
$utf8 = [Text.UTF8Encoding]::new($false)

function Get-Pin([string]$Section, [string]$Key) {
    $current = ''
    foreach ($line in ($manifestText -split "`n")) {
        $line = $line.TrimEnd("`r")
        if ($line.StartsWith('[[')) { $current = '#files'; continue }
        if ($line.StartsWith('[')) { $current = $line.Substring(1, $line.Length - 2); continue }
        if ($current -eq $Section) {
            $match = [regex]::Match($line, '^(\S+) = "(.*)"$')
            if ($match.Success -and $match.Groups[1].Value -eq $Key) { return $match.Groups[2].Value }
        }
    }
    $label = if ($Section) { "$Section.$Key" } else { $Key }
    throw "MANIFEST.toml has no $label"
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Assert-Sha256([string]$Label, [string]$Path, [string]$Expected) {
    $found = Get-Sha256 $Path
    if ($found -ne $Expected) {
        [Console]::Error.WriteLine("Hash mismatch for ${Label}: expected $Expected, found $found")
        exit 1
    }
}

function Save-Download([string]$Url, [string]$Path, [string]$Label) {
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Path -MaximumRetryCount 3
    } catch {
        [Console]::Error.WriteLine("Download failed for ${Label}: $Url")
        exit 1
    }
}

function Write-Lf([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, $utf8)
}

function Invoke-Tar([string[]]$Arguments) {
    & tar @Arguments
    if ($LASTEXITCODE -ne 0) { throw "tar $($Arguments -join ' ') failed" }
}

$name = Get-Pin '' 'name'
$version = Get-Pin '' 'version'
$packageUrl = Get-Pin 'package' 'url'
$integrity = Get-Pin 'package' 'integrity'
$wasmMember = Get-Pin 'package' 'wasm'
$wasmSha256 = Get-Pin 'package' 'wasm_sha256'
$wabtVersion = Get-Pin 'wabt' 'version'
$wabtUrl = Get-Pin 'wabt' 'url'
$simdeCommit = Get-Pin 'simde' 'commit'
$simdeHeader = Get-Pin 'simde' 'header'

$work = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    foreach ($directory in 'package', 'wabt', 'simde', 'licenses', 'out') {
        New-Item -ItemType Directory -Path (Join-Path $work $directory) | Out-Null
    }

    $tarball = Join-Path $work 'package/package.tgz'
    Save-Download $packageUrl $tarball "$name $version"
    $found = 'sha512-' + [Convert]::ToBase64String([Security.Cryptography.SHA512]::HashData([IO.File]::ReadAllBytes($tarball)))
    if ($found -ne $integrity) {
        [Console]::Error.WriteLine("Hash mismatch for $name $version tarball: expected $integrity, found $found")
        exit 1
    }
    Invoke-Tar @('-xzf', $tarball, '-C', (Join-Path $work 'package'))
    $packageRoot = Join-Path $work 'package/package'
    $packageVersion = (Get-Content -Raw -LiteralPath (Join-Path $packageRoot 'package.json') | ConvertFrom-Json).version
    if ($packageVersion -ne $version) { throw "Tarball holds $name $packageVersion, MANIFEST.toml pins $version" }
    $wasm = Join-Path $work "package/$wasmMember"
    Assert-Sha256 "$name $version highlights.wasm" $wasm $wasmSha256

    if ($env:WASM2C) {
        $wasm2c = $env:WASM2C
        $found = (& $wasm2c --version 2>$null | Out-String).Trim()
        if ($found -ne $wabtVersion) {
            $shown = if ($found) { $found } else { 'no version' }
            [Console]::Error.WriteLine("WASM2C must be wasm2c $wabtVersion, found $shown")
            exit 1
        }
        $resolved = (Get-Item -LiteralPath $wasm2c).ResolvedTarget
        if (-not $resolved) { $resolved = (Get-Item -LiteralPath $wasm2c).FullName }
        $wabtRoot = [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $resolved) '..'))
    } else {
        $architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        $wabtHost = if ($IsWindows -and $architecture -eq 'X64') { 'windows-x64' }
            elseif ($IsLinux -and $architecture -eq 'X64') { 'linux-x64' }
            elseif ($IsLinux -and $architecture -eq 'Arm64') { 'linux-arm64' }
            elseif ($IsMacOS -and $architecture -eq 'Arm64') { 'macos-arm64' }
            else { [Console]::Error.WriteLine("No WABT $wabtVersion binary for this host: set WASM2C"); exit 1 }
        $archive = "wabt-$wabtVersion-$wabtHost.tar.gz"
        $archivePath = Join-Path $work "wabt/$archive"
        Save-Download "$wabtUrl/$archive" $archivePath "WABT $wabtVersion ($wabtHost)"
        Save-Download "$wabtUrl/$archive.sha256" "$archivePath.sha256" "WABT $wabtVersion ($wabtHost) checksum"
        $published = ((Get-Content -Raw -LiteralPath "$archivePath.sha256").Trim() -split '\s+')[0]
        Assert-Sha256 "WABT $wabtVersion ($wabtHost), published checksum" $archivePath $published
        Assert-Sha256 "WABT $wabtVersion ($wabtHost)" $archivePath (Get-Pin 'wabt' "$($wabtHost.Replace('-', '_'))_sha256")
        Invoke-Tar @('-xzf', $archivePath, '-C', (Join-Path $work 'wabt'))
        $wabtRoot = Join-Path $work "wabt/wabt-$wabtVersion"
        $wasm2c = Join-Path $wabtRoot 'bin/wasm2c'
        if ($IsWindows) { $wasm2c += '.exe' }
        $found = (& $wasm2c --version | Out-String).Trim()
        if ($found -ne $wabtVersion) { throw "The WABT archive holds wasm2c $found, expected $wabtVersion" }
    }

    $simdeArchive = Join-Path $work 'simde/simde.tar.xz'
    Save-Download (Get-Pin 'simde' 'url') $simdeArchive "SIMDe $simdeCommit"
    Assert-Sha256 'SIMDe amalgamated archive' $simdeArchive (Get-Pin 'simde' 'sha256')
    Invoke-Tar @('-xJf', $simdeArchive, '-C', (Join-Path $work 'simde'), $simdeHeader)
    $simdePath = Join-Path $work "simde/$simdeHeader"
    Assert-Sha256 'SIMDe wasm/simd128.h' $simdePath (Get-Pin 'simde' 'header_sha256')
    if ((Get-Content -LiteralPath $simdePath -TotalCount 2)[1] -ne "/* $simdeCommit */") {
        throw "SIMDe wasm/simd128.h is not from commit $simdeCommit"
    }

    $wabtLicense = Join-Path $work 'licenses/wabt'
    Save-Download (Get-Pin 'wabt' 'license_url') $wabtLicense 'WABT license'
    Assert-Sha256 'WABT license' $wabtLicense (Get-Pin 'wabt' 'license_sha256')
    $simdeLicense = Join-Path $work 'licenses/simde'
    Save-Download (Get-Pin 'simde' 'license_url') $simdeLicense 'SIMDe license'
    Assert-Sha256 'SIMDe license' $simdeLicense (Get-Pin 'simde' 'license_sha256')

    $out = Join-Path $work 'out'
    foreach ($directory in 'c/generated', 'c/wasm2c', 'c/simde/wasm', 'src') {
        New-Item -ItemType Directory -Force -Path (Join-Path $out $directory) | Out-Null
    }
    & $wasm2c $wasm -n highlights -o (Join-Path $work 'highlights.c')
    if ($LASTEXITCODE -ne 0) { throw 'wasm2c failed' }
    $banner = "/* Generated by scripts/sync-pierre-highlights from $name $version (highlights.wasm sha256 $wasmSha256). Do not edit. */`n"
    foreach ($file in 'highlights.c', 'highlights.h') {
        $generated = [IO.File]::ReadAllText((Join-Path $work $file)).Replace("`r`n", "`n")
        Write-Lf (Join-Path $out "c/generated/$file") ($banner + $generated)
    }
    Copy-Item -LiteralPath (Join-Path $packageRoot 'LICENSE.md') -Destination (Join-Path $out 'c/generated/LICENSE')
    $runtime = [ordered]@{
        'include/wasm-rt.h' = 'wasm-rt.h'
        'share/wabt/wasm2c/wasm-rt-impl.h' = 'wasm-rt-impl.h'
        'share/wabt/wasm2c/wasm-rt-impl.c' = 'wasm-rt-impl.c'
        'share/wabt/wasm2c/wasm-rt-impl-tableops.inc' = 'wasm-rt-impl-tableops.inc'
        'share/wabt/wasm2c/wasm-rt-mem-impl.c' = 'wasm-rt-mem-impl.c'
        'share/wabt/wasm2c/wasm-rt-mem-impl-helper.inc' = 'wasm-rt-mem-impl-helper.inc'
    }
    foreach ($entry in $runtime.GetEnumerator()) {
        Copy-Item -LiteralPath (Join-Path $wabtRoot $entry.Key) -Destination (Join-Path $out "c/wasm2c/$($entry.Value)")
    }
    Copy-Item -LiteralPath $wabtLicense -Destination (Join-Path $out 'c/wasm2c/LICENSE')
    Copy-Item -LiteralPath $simdePath -Destination (Join-Path $out 'c/simde/wasm/simd128.h')
    Copy-Item -LiteralPath $simdeLicense -Destination (Join-Path $out 'c/simde/LICENSE')

    $notice = "Pierre highlights, the lexers of the npm package $name $version translated to C by wasm2c $wabtVersion.`n" +
        "Package: $packageUrl`n" +
        "Source: $(Get-Pin '' 'source_repository') ($(Get-Pin '' 'source_path'))`n`n" +
        "The generated code under generated/ is licensed under $(Get-Pin '' 'license'), the license of`n" +
        "the package it is generated from; generated/LICENSE holds its text.`n" +
        "The wasm2c runtime under wasm2c/ is part of WABT, licensed under Apache-2.0`n" +
        "(wasm2c/LICENSE). simde/wasm/simd128.h is SIMDe $(Get-Pin 'simde' 'version') at commit $simdeCommit,`n" +
        "licensed under MIT (simde/LICENSE).`n`n" +
        "The NOTICE.md of $name $version follows.`n`n"
    $noticeBytes = $utf8.GetBytes($notice) + [IO.File]::ReadAllBytes((Join-Path $packageRoot 'NOTICE.md'))
    [IO.File]::WriteAllBytes((Join-Path $out 'c/NOTICE'), $noticeBytes)

    $languageLines = [IO.File]::ReadAllLines((Join-Path $packageRoot 'dist/languages.js')) | Where-Object { $_.StartsWith("`t") }
    $tokenLines = [IO.File]::ReadAllLines((Join-Path $packageRoot 'dist/token-types.js')) | Where-Object { $_.StartsWith("`t`"") }
    $languageRows = foreach ($line in $languageLines) {
        $match = [regex]::Match($line, '^\t"?([^":]*)"?: ([0-9]+),?$')
        if (-not $match.Success) { throw "Could not read every language of $name/dist/languages.js" }
        "    (`"$($match.Groups[1].Value)`", $($match.Groups[2].Value)),"
    }
    $tokenRows = foreach ($line in $tokenLines) {
        $match = [regex]::Match($line, '^\t"([^"]*)",?$')
        if (-not $match.Success) { throw "Could not read every token type of $name/dist/token-types.js" }
        "    `"$($match.Groups[1].Value)`","
    }
    if (-not $languageRows -or -not $tokenRows) { throw "Could not read the tables of $name" }
    if ((($languageRows + $tokenRows) -join '').Contains('\')) { throw 'A language key or token type holds a backslash' }
    $tables = "pub(crate) const PACKAGE_VERSION: &str = `"$version`";`n" +
        "pub(crate) const WASM_SHA256: &str = `"$wasmSha256`";`n`n" +
        "pub(crate) const LANGUAGE_KEYS: &[(&str, u8)] = &[`n$($languageRows -join "`n")`n];`n`n" +
        "pub(crate) const TOKEN_TYPES: &[&str] = &[`n$($tokenRows -join "`n")`n];`n"
    Write-Lf (Join-Path $out 'src/tables.rs') $tables

    $files = @(
        'c/generated/highlights.c'
        'c/generated/highlights.h'
        'c/generated/LICENSE'
        'c/NOTICE'
        'c/wasm2c/wasm-rt.h'
        'c/wasm2c/wasm-rt-impl.h'
        'c/wasm2c/wasm-rt-impl.c'
        'c/wasm2c/wasm-rt-impl-tableops.inc'
        'c/wasm2c/wasm-rt-mem-impl.c'
        'c/wasm2c/wasm-rt-mem-impl-helper.inc'
        'c/wasm2c/LICENSE'
        'c/simde/wasm/simd128.h'
        'c/simde/LICENSE'
        'src/tables.rs'
    )
    $header = [Collections.Generic.List[string]]::new()
    $blank = 0
    foreach ($line in ($manifestText.Replace("`r`n", "`n").TrimEnd("`n") -split "`n")) {
        if ($line -eq '[[files]]') { break }
        if ($line -eq '') { $blank++; continue }
        while ($blank -gt 0) { $header.Add(''); $blank-- }
        $header.Add($line)
    }
    $manifest = [Text.StringBuilder]::new()
    foreach ($line in $header) { [void]$manifest.Append("$line`n") }
    foreach ($file in $files) {
        [void]$manifest.Append("`n[[files]]`npath = `"$file`"`nsha256 = `"$(Get-Sha256 (Join-Path $out $file))`"`n")
    }
    Write-Lf (Join-Path $out 'c/MANIFEST.toml') $manifest.ToString()

    $all = $files + 'c/MANIFEST.toml'
    if ($Check) {
        $failed = $false
        $listed = [regex]::Matches($manifestText, '(?m)^path = "(.*)"\r?$') | ForEach-Object { $_.Groups[1].Value }
        foreach ($file in $listed) {
            if (-not (Test-Path -LiteralPath (Join-Path $crate $file) -PathType Leaf)) {
                [Console]::Error.WriteLine("Missing vendored file: $file")
                $failed = $true
            }
        }
        foreach ($file in $all) {
            $target = Join-Path $crate $file
            if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
                if ($listed -notcontains $file) { [Console]::Error.WriteLine("Missing vendored file: $file") }
                $failed = $true
            } elseif ((Get-Sha256 $target) -ne (Get-Sha256 (Join-Path $out $file))) {
                [Console]::Error.WriteLine("Divergent vendored file: $file")
                $failed = $true
            }
        }
        if ($failed) { exit 1 }
        Write-Output "$name $version vendored files verified"
    } else {
        foreach ($file in $all) {
            $target = Join-Path $crate $file
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
            Copy-Item -LiteralPath (Join-Path $out $file) -Destination $target -Force
        }
        Write-Output "$name $version vendored files written"
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
