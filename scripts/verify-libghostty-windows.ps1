[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Binary,
    [string]$PackageRoot,
    [string]$RuntimeEvidence,
    [string]$ReportPath,
    [switch]$AllowGeneratedIcons,
    [switch]$AllowReleaseAssets
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$ManifestPath = Join-Path $Root "native\libghostty\manifest.toml"

. "$PSScriptRoot/libghostty-manifest.ps1"

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-NormalizedTextSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    $text = [IO.File]::ReadAllText($Path).Replace("`r`n", "`n").Replace("`r", "`n")
    $encoding = [Text.UTF8Encoding]::new($false)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash($encoding.GetBytes($text)) |
            ForEach-Object { $_.ToString("x2") }) -join "")
    }
    finally {
        $sha.Dispose()
    }
}

function Assert-TextHash {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "missing $Label at $Path"
    }
    $actual = Get-NormalizedTextSha256 $Path
    if ($actual -ne $Expected) {
        throw "$Label hash drift: expected $Expected, got $actual"
    }
}

function Get-KeyValueFile {
    param([Parameter(Mandatory = $true)][string]$Path)

    $values = [ordered]@{}
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ($line -notmatch '^([^=]+)=(.*)$') {
            throw "invalid key/value evidence in $Path"
        }
        $values[$matches[1]] = $matches[2]
    }
    return $values
}

function Assert-RecordedValue {
    param(
        [Parameter(Mandatory = $true)][Collections.IDictionary]$Values,
        [Parameter(Mandatory = $true)][string]$Key,
        [Parameter(Mandatory = $true)][string]$Expected
    )

    if (-not $Values.Contains($Key) -or $Values[$Key] -ne $Expected) {
        $actual = if ($Values.Contains($Key)) { $Values[$Key] } else { "<missing>" }
        throw "build-info drift for $Key`: expected $Expected, got $actual"
    }
}

function Resolve-Dumpbin {
    $onPath = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    if ($null -ne $onPath) {
        return $onPath.Source
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw "dumpbin.exe is required, and vswhere.exe was not found"
    }
    $installation = (& $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath | Select-Object -First 1)
    if ([string]::IsNullOrWhiteSpace($installation)) {
        throw "Visual Studio C++ tools are required to inspect PE/COFF artifacts"
    }
    $candidate = Get-ChildItem -LiteralPath (Join-Path $installation "VC\Tools\MSVC") `
        -Recurse -Filter dumpbin.exe -File |
        Where-Object { $_.FullName -match '\\bin\\Hostx64\\x64\\dumpbin\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($null -eq $candidate) {
        throw "dumpbin.exe was not found under the selected Visual Studio installation"
    }
    return $candidate.FullName
}

function Get-PeImports {
    param(
        [Parameter(Mandatory = $true)][string]$Dumpbin,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $output = @(& $Dumpbin /dependents $Path 2>&1 | ForEach-Object { $_.ToString() })
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin could not inspect PE imports for $Path"
    }
    return @($output |
        Select-String -Pattern '^\s+([A-Za-z0-9._-]+\.dll)\s*$' |
        ForEach-Object { $_.Matches[0].Groups[1].Value.ToLowerInvariant() } |
        Sort-Object -Unique)
}

if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "unsupported libghostty Windows qualification target: $Target"
}

$SourceSha = Get-ManifestString "source_sha"
$GhosttyVersion = Get-ManifestString "ghostty_app_version"
$ZigVersion = Get-ManifestString "zig_version"
$ZigArchiveUrl = Get-ManifestString "windows_zig_archive_url"
$ZigArchiveSha = Get-ManifestString "windows_zig_archive_sha256"
$ZigExecutableSha = Get-ManifestString "windows_zig_executable_sha256"
$ZigSourceArchiveUrl = Get-ManifestString "windows_zig_source_archive_url"
$ZigSourceArchiveSha = Get-ManifestString "windows_zig_source_archive_sha256"
$ZigImageBase = Get-ManifestString "windows_zig_image_base"
$ZigDllCharacteristics = Get-ManifestString "windows_zig_dll_characteristics"
$HeadersNormalization = Get-ManifestString "windows_headers_normalization"
$BindingsSha = Get-ManifestString "bindings_sha256"
$NoticeSha = Get-ManifestString "notice_sha256"
$SbomSha = Get-ManifestString "sbom_sha256"
$ArchivePath = (Get-ManifestTargetString $Target "archive_path").Replace('/', '\')
$ArchiveSha = Get-ManifestTargetString $Target "archive_sha256"
$BuildInfoSha = Get-ManifestTargetString $Target "build_info_sha256"
$HeadersSha = Get-ManifestTargetString $Target "headers_index_sha256"
$SymbolsSha = Get-ManifestTargetString $Target "symbols_sha256"
$ZigTarget = Get-ManifestTargetString $Target "zig_target"
$Normalization = Get-ManifestTargetString $Target "archive_normalization"
$SimdText = (Get-ManifestTargetBoolean $Target "simd").ToString().ToLowerInvariant()
$SystemLibraries = @(Get-ManifestTargetStringArray $Target "system_libraries")
$SystemLibrariesText = @($SystemLibraries | ForEach-Object { "$_.lib" }) -join ','

$PreparedRoot = Join-Path $Root "native\libghostty\prebuilt\$Target"
$Archive = Join-Path $PreparedRoot $ArchivePath
$BuildInfo = Join-Path $PreparedRoot "build-info.txt"
$Headers = Join-Path $PreparedRoot "headers.sha256"
$Symbols = Join-Path $PreparedRoot "symbols.txt"
$PreparedBindings = Join-Path $PreparedRoot "bindings.rs"
$Header = Join-Path $PreparedRoot "include\ghostty\vt.h"
$CanonicalBindings = Join-Path $Root (Get-ManifestString "bindings_path")
$Notice = Join-Path $Root (Get-ManifestString "notice_path")
$Sbom = Join-Path $Root (Get-ManifestString "sbom_path")
foreach ($required in @($Archive, $BuildInfo, $Headers, $Symbols, $PreparedBindings, $Header, $CanonicalBindings, $Notice, $Sbom)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "missing reviewed libghostty Windows input: $required"
    }
}

if ((Get-Sha256 $Archive) -ne $ArchiveSha) {
    throw "Windows libghostty archive hash drift"
}
Assert-TextHash $BuildInfo $BuildInfoSha "Windows libghostty build-info"
Assert-TextHash $Headers $HeadersSha "Windows libghostty header inventory"
Assert-TextHash $Symbols $SymbolsSha "Windows libghostty symbol inventory"
Assert-TextHash $PreparedBindings $BindingsSha "prepared Windows libghostty bindings"
Assert-TextHash $CanonicalBindings $BindingsSha "canonical libghostty bindings"
Assert-TextHash $Header (Get-ManifestString "header_sha256") "Windows libghostty public header"
Assert-TextHash $Notice $NoticeSha "libghostty third-party notice"
Assert-TextHash $Sbom $SbomSha "libghostty CycloneDX SBOM"

$BuildValues = Get-KeyValueFile $BuildInfo
foreach ($expectation in @(
    @{ Key = "source_sha"; Value = $SourceSha },
    @{ Key = "zig_version"; Value = $ZigVersion },
    @{ Key = "zig_archive_url"; Value = $ZigArchiveUrl },
    @{ Key = "zig_archive_sha256"; Value = $ZigArchiveSha },
    @{ Key = "zig_executable_sha256"; Value = $ZigExecutableSha },
    @{ Key = "zig_source_archive_url"; Value = $ZigSourceArchiveUrl },
    @{ Key = "zig_source_archive_sha256"; Value = $ZigSourceArchiveSha },
    @{ Key = "zig_image_base"; Value = $ZigImageBase },
    @{ Key = "zig_dll_characteristics"; Value = $ZigDllCharacteristics },
    @{ Key = "headers_normalization"; Value = $HeadersNormalization },
    @{ Key = "rust_target"; Value = $Target },
    @{ Key = "zig_target"; Value = $ZigTarget },
    @{ Key = "simd"; Value = $SimdText },
    @{ Key = "archive_normalization"; Value = $Normalization },
    @{ Key = "archive_sha256"; Value = $ArchiveSha },
    @{ Key = "bindings_sha256"; Value = $BindingsSha },
    @{ Key = "headers_sha256"; Value = $HeadersSha },
    @{ Key = "symbols_sha256"; Value = $SymbolsSha },
    @{ Key = "msvc_toolset"; Value = (Get-ManifestString "windows_msvc_toolset") },
    @{ Key = "windows_sdk"; Value = (Get-ManifestString "windows_sdk") },
    @{ Key = "llvm_version"; Value = (Get-ManifestString "windows_llvm_version") },
    @{ Key = "crt"; Value = (Get-ManifestString "windows_crt") },
    @{ Key = "cxx_runtime"; Value = (Get-ManifestString "windows_cxx_runtime") },
    @{ Key = "system_libraries"; Value = $SystemLibrariesText }
)) {
    Assert-RecordedValue $BuildValues $expectation.Key $expectation.Value
}

$RequiredSymbols = @(
    "ghostty_build_info",
    "ghostty_free",
    "ghostty_key_encoder_encode",
    "ghostty_render_state_update",
    "ghostty_terminal_free",
    "ghostty_terminal_new",
    "ghostty_terminal_resize",
    "ghostty_terminal_vt_write"
)
$SymbolSet = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($symbol in Get-Content -LiteralPath $Symbols) {
    $null = $SymbolSet.Add($symbol)
}
foreach ($symbol in $RequiredSymbols) {
    if (-not $SymbolSet.Contains($symbol)) {
        throw "Windows libghostty symbol inventory is missing $symbol"
    }
}

$UnexpectedNativeDlls = @(Get-ChildItem -LiteralPath $PreparedRoot -Recurse -Filter "*ghostty*.dll" -File)
if ($UnexpectedNativeDlls.Count -ne 0) {
    throw "reviewed Windows input contains a forbidden Ghostty DLL"
}

$SbomDocument = Get-Content -LiteralPath $Sbom -Raw | ConvertFrom-Json
if ($SbomDocument.bomFormat -ne "CycloneDX" -or $SbomDocument.specVersion -ne "1.6") {
    throw "libghostty SBOM must remain CycloneDX 1.6"
}
$SbomText = Get-Content -LiteralPath $Sbom -Raw
$NoticeText = Get-Content -LiteralPath $Notice -Raw
$LicenseComponents = @(
    Get-Content -LiteralPath $ManifestPath |
        Select-String -Pattern '^component = "([^"]+)"$' |
        ForEach-Object { $_.Matches[0].Groups[1].Value }
)
foreach ($component in $LicenseComponents) {
    if (-not $NoticeText.Contains($component)) {
        throw "libghostty notice is missing licensed component $component"
    }
    if (-not $SbomText.Contains(('"name": "' + $component + '"'))) {
        throw "libghostty SBOM is missing licensed component $component"
    }
}
if (-not $SbomText.Contains($SourceSha) -or -not $SbomText.Contains($Target)) {
    throw "libghostty SBOM does not identify the pinned source and Windows target"
}

$Dumpbin = Resolve-Dumpbin
$ArchiveHeaders = @(& $Dumpbin /headers $Archive 2>&1 | ForEach-Object { $_.ToString() })
if ($LASTEXITCODE -ne 0 -or -not ($ArchiveHeaders -match '8664 machine \(x64\)')) {
    throw "Windows libghostty archive is not a valid x64 COFF archive"
}

$BinaryHash = $null
$BinaryImports = @()
$BinaryIdentityWitnesses = @()
if (-not [string]::IsNullOrWhiteSpace($Binary)) {
    $Binary = (Resolve-Path -LiteralPath $Binary).Path
    $BinaryHeaders = @(& $Dumpbin /headers $Binary 2>&1 | ForEach-Object { $_.ToString() })
    if ($LASTEXITCODE -ne 0 -or -not ($BinaryHeaders -match '8664 machine \(x64\)')) {
        throw "Paneflow release binary is not x64 PE/COFF"
    }
    $BinaryImports = @(Get-PeImports $Dumpbin $Binary)
    if ($BinaryImports -match '^ghostty.*\.dll$') {
        throw "Paneflow release binary imports a forbidden Ghostty DLL"
    }

    $ApprovedImports = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in @(
        "advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll", "combase.dll",
        "comctl32.dll", "crypt32.dll", "d3d11.dll", "d3dcompiler_47.dll",
        "dcomp.dll", "dwmapi.dll", "dwrite.dll", "dxgi.dll", "gdi32.dll",
        "icuuc.dll", "imm32.dll", "iphlpapi.dll", "kernel32.dll", "ntdll.dll",
        "ole32.dll", "oleaut32.dll", "shell32.dll", "uiautomationcore.dll",
        "user32.dll", "userenv.dll", "vcruntime140.dll", "winmm.dll",
        "wintrust.dll", "ws2_32.dll"
    )) {
        $null = $ApprovedImports.Add($name)
    }
    foreach ($import in $BinaryImports) {
        if (-not $ApprovedImports.Contains($import) -and
            $import -notmatch '^(api-ms-win|ext-ms-win)-.+\.dll$') {
            throw "Paneflow release binary imports unreviewed runtime $import"
        }
    }
    $BinaryAscii = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Binary))
    foreach ($witness in @($SourceSha, $GhosttyVersion)) {
        if (-not $BinaryAscii.Contains($witness)) {
            throw "Paneflow release binary is missing pinned libghostty identity witness $witness"
        }
        $BinaryIdentityWitnesses += $witness
    }
    $BinaryAscii = $null
    $BinaryHash = Get-Sha256 $Binary
}

$RuntimeSmokeVerified = $false
$RuntimeEvidenceSha = $null
if (-not [string]::IsNullOrWhiteSpace($RuntimeEvidence)) {
    if ([string]::IsNullOrWhiteSpace($Binary) -or $null -eq $BinaryHash) {
        throw "runtime evidence requires the exact PaneFlow binary it exercised"
    }
    $RuntimeEvidence = (Resolve-Path -LiteralPath $RuntimeEvidence).Path
    $RuntimeDocument = Get-Content -LiteralPath $RuntimeEvidence -Raw | ConvertFrom-Json
    foreach ($expectation in @(
        @{ Key = "binary_sha256"; Value = $BinaryHash },
        @{ Key = "source_sha"; Value = $SourceSha },
        @{ Key = "ghostty_version"; Value = $GhosttyVersion },
        @{ Key = "status"; Value = "passed" }
    )) {
        $property = $RuntimeDocument.PSObject.Properties[$expectation.Key]
        if ($null -eq $property -or [string]$property.Value -ne $expectation.Value) {
            $actual = if ($null -eq $property) { "<missing>" } else { [string]$property.Value }
            throw "runtime evidence drift for $($expectation.Key): expected $($expectation.Value), got $actual"
        }
    }
    $RuntimeSmokeVerified = $true
    $RuntimeEvidenceSha = Get-Sha256 $RuntimeEvidence
}

$PackageFiles = @()
if (-not [string]::IsNullOrWhiteSpace($PackageRoot)) {
    $PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
    $RequiredPackageFiles = @(
        "paneflow.exe",
        "LICENSE.txt",
        "THIRD_PARTY_NOTICES.md",
        "libghostty-sbom.cdx.json",
        "libghostty-manifest.toml",
        "libghostty-build-info.txt"
    )
    foreach ($relative in $RequiredPackageFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $PackageRoot $relative) -PathType Leaf)) {
            throw "installed MSI payload is missing $relative"
        }
    }
    $ForbiddenDlls = @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -Filter "*ghostty*.dll" -File)
    if ($ForbiddenDlls.Count -ne 0) {
        throw "installed MSI payload contains a forbidden Ghostty DLL"
    }
    $PackageFiles = @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
        ForEach-Object { [IO.Path]::GetRelativePath($PackageRoot, $_.FullName).Replace('\', '/') } |
        Sort-Object)
}

$Commit = (& git -C $Root rev-parse HEAD).Trim()
if ($Commit -notmatch '^[0-9a-f]{40}$') {
    throw "libghostty provenance requires a full checked-out Git commit SHA"
}
$WorktreePathspecs = @(
    ".",
    ":(exclude).ghostty-source",
    ":(exclude).ghostty-source/**"
)
if ($AllowGeneratedIcons) {
    $WorktreePathspecs += @(
        ":(exclude)assets/PaneFlow.ico",
        ":(exclude)assets/icons/paneflow-16.png",
        ":(exclude)assets/icons/paneflow-24.png",
        ":(exclude)assets/icons/paneflow-32.png",
        ":(exclude)assets/icons/paneflow-48.png",
        ":(exclude)assets/icons/paneflow-64.png",
        ":(exclude)assets/icons/paneflow-128.png",
        ":(exclude)assets/icons/paneflow-256.png",
        ":(exclude)assets/icons/paneflow-512.png",
        ":(exclude)packaging/wix/paneflow.ico",
        ":(exclude)src-app/assets/icons/paneflow.png"
    )
}
if ($AllowReleaseAssets) {
    $WorktreePathspecs += ":(exclude)release-assets/**"
}
$WorktreeChanges = @(& git -C $Root status --porcelain --untracked-files=all -- @WorktreePathspecs)
if ($LASTEXITCODE -ne 0) {
    throw "could not inspect the libghostty provenance worktree"
}
if ($WorktreeChanges.Count -ne 0) {
    throw "libghostty provenance requires a clean worktree outside the explicit .ghostty-source checkout: $($WorktreeChanges -join ', ')"
}

$Report = [ordered]@{
    schema_version = 1
    commit = $Commit
    worktree_dirty = $false
    generated_icon_changes_allowed = [bool]$AllowGeneratedIcons
    release_asset_changes_allowed = [bool]$AllowReleaseAssets
    target = $Target
    source_sha = $SourceSha
    ghostty_version = $GhosttyVersion
    zig_version = $ZigVersion
    zig_codegen = [ordered]@{
        archive_url = $ZigArchiveUrl
        archive_sha256 = $ZigArchiveSha
        executable_sha256 = $ZigExecutableSha
        source_archive_url = $ZigSourceArchiveUrl
        source_archive_sha256 = $ZigSourceArchiveSha
        image_base = $ZigImageBase
        dll_characteristics = $ZigDllCharacteristics
    }
    headers_normalization = $HeadersNormalization
    archive_sha256 = $ArchiveSha
    manifest_sha256 = Get-NormalizedTextSha256 $ManifestPath
    notice_sha256 = $NoticeSha
    sbom_sha256 = $SbomSha
    symbol_count = [int]$BuildValues["symbol_count"]
    linkage = if ($RuntimeSmokeVerified) { "static" } else { "static-candidate" }
    runtime_smoke_verified = $RuntimeSmokeVerified
    runtime_evidence_sha256 = $RuntimeEvidenceSha
    binary_sha256 = $BinaryHash
    binary_imports = $BinaryImports
    binary_identity_witnesses = $BinaryIdentityWitnesses
    package_files = $PackageFiles
}

if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $parent = Split-Path $resolvedReport -Parent
    if ([string]::IsNullOrWhiteSpace($parent)) {
        throw "cannot resolve report parent for $ReportPath"
    }
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    $Report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $resolvedReport -Encoding utf8
}

Write-Host "verified libghostty Windows provenance, static linkage, notices, and SBOM"
