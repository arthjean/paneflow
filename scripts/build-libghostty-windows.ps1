[CmdletBinding()]
param(
    [string]$SourceDir = $env:PANEFLOW_GHOSTTY_SOURCE_DIR,
    [string]$OutputDir,
    [string]$Zig = "zig",
    [string]$EvidenceDir = $env:EVIDENCE_DIR,
    [switch]$VerifyReproducible
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Target = "x86_64-pc-windows-msvc"
$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$ManifestPath = Join-Path $Root "native\libghostty\manifest.toml"
$SmokeSource = Join-Path $Root "native\libghostty\windows-smoke.c"

. "$PSScriptRoot/libghostty-manifest.ps1"

function Get-Sha256 {
    param([string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-NormalizedTextSha256 {
    param([string]$Path)

    $text = [IO.File]::ReadAllText($Path).Replace("`r`n", "`n").Replace("`r", "`n")
    $encoding = New-Object System.Text.UTF8Encoding($false)
    $bytes = $encoding.GetBytes($text)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString("x2") }) -join "")
    }
    finally {
        $sha.Dispose()
    }
}

function Get-Pe64ImageMetadata {
    param([string]$Path)

    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $reader = [IO.BinaryReader]::new($stream, [Text.Encoding]::ASCII, $true)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) {
            throw "$Path is not a PE executable"
        }
        $stream.Position = 0x3c
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 0x100)) {
            throw "$Path has an invalid PE header offset"
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "$Path has an invalid PE signature"
        }
        if ($reader.ReadUInt16() -ne 0x8664) {
            throw "$Path is not an x64 PE executable"
        }
        $optionalHeader = $peOffset + 24
        $stream.Position = $optionalHeader
        if ($reader.ReadUInt16() -ne 0x020b) {
            throw "$Path is not a PE32+ executable"
        }
        $stream.Position = $optionalHeader + 0x18
        $imageBase = $reader.ReadUInt64()
        $dllCharacteristicsOffset = $optionalHeader + 0x46
        $stream.Position = $dllCharacteristicsOffset
        $dllCharacteristics = $reader.ReadUInt16()
        return [pscustomobject]@{
            ImageBase = "0x{0:x16}" -f $imageBase
            DllCharacteristics = "0x{0:x4}" -f $dllCharacteristics
            DllCharacteristicsValue = [uint16]$dllCharacteristics
            DllCharacteristicsOffset = [int64]$dllCharacteristicsOffset
        }
    }
    finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Write-Utf8Lines {
    param(
        [string]$Path,
        [string[]]$Lines
    )

    $content = ($Lines -join "`n") + "`n"
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, $content, $encoding)
}

function Sort-Ordinal {
    param([AllowEmptyCollection()][string[]]$Values)

    $sorted = [string[]]@($Values)
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    return $sorted
}

function Assert-NonRootDirectory {
    param(
        [string]$Path,
        [string]$Purpose
    )

    $resolved = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $root = [IO.Path]::GetPathRoot($resolved).TrimEnd('\', '/')
    if ([string]::IsNullOrWhiteSpace($resolved) -or $resolved -eq $root) {
        throw "refusing to use filesystem root for $Purpose`: $Path"
    }
}

function Import-MsvcEnvironment {
    param(
        [string]$MsvcToolset,
        [string]$WindowsSdk
    )

    if ($MsvcToolset -notmatch '^[0-9]+(?:\.[0-9]+)+$' -or $WindowsSdk -notmatch '^[0-9]+(?:\.[0-9]+)+$') {
        throw "manifest-pinned MSVC toolset and Windows SDK versions must contain only digits and dots"
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw "Visual Studio Installer vswhere.exe is required for the x64 MSVC build"
    }
    $installation = (& $vswhere -latest -products "*" -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
    if ([string]::IsNullOrWhiteSpace($installation)) {
        throw "Visual Studio 2022 with the x64 C++ toolchain is required"
    }
    $vsDevCmd = Join-Path $installation "Common7\Tools\VsDevCmd.bat"
    $vsDevCmdCommand = '"{0}" -no_logo -arch=x64 -host_arch=x64 -vcvars_ver={1} -winsdk={2} && set' -f $vsDevCmd, $MsvcToolset, $WindowsSdk
    $environment = @(& $env:ComSpec /d /s /c $vsDevCmdCommand 2>&1 | ForEach-Object { $_.ToString() })
    $vsDevCmdExitCode = $LASTEXITCODE
    if ($vsDevCmdExitCode -ne 0) {
        throw "VsDevCmd.bat failed to select manifest-pinned MSVC toolset $MsvcToolset and Windows SDK $WindowsSdk (exit code $vsDevCmdExitCode)`n$($environment -join "`n")"
    }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
        }
    }
    return $installation
}

function Resolve-Tool {
    param(
        [string]$Name,
        [string[]]$Candidates = @()
    )

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }
    foreach ($candidate in $Candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    throw "$Name is required to build libghostty for $Target"
}

function Normalize-CoffArchive {
    param(
        [string]$Archive,
        [string]$Destination,
        [string]$LlvmObjcopy,
        [string]$LlvmAr
    )

    $work = Join-Path (Split-Path -Parent $Destination) "normalize"
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    $archiveMembers = @(& $LlvmAr t $Archive)
    if ($LASTEXITCODE -ne 0 -or $archiveMembers.Count -eq 0) {
        throw "cannot enumerate COFF members in $Archive"
    }
    $names = @($archiveMembers | ForEach-Object { Split-Path $_ -Leaf })
    $nameSet = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $duplicates = [Collections.Generic.SortedSet[string]]::new([StringComparer]::Ordinal)
    foreach ($name in $names) {
        if (-not $nameSet.Add($name)) {
            $null = $duplicates.Add($name)
        }
    }
    if ($duplicates.Count -ne 0) {
        throw "COFF normalization found duplicate member names: $([string]::Join(', ', $duplicates))"
    }

    Push-Location $work
    try {
        $null = & $LlvmAr x $Archive
        if ($LASTEXITCODE -ne 0) {
            throw "llvm-ar failed to extract $Archive"
        }
    }
    finally {
        Pop-Location
    }

    $sourcePaths = Sort-Ordinal @(Get-ChildItem -LiteralPath $work -File -Filter "*.obj" | ForEach-Object { $_.FullName })
    $sources = @($sourcePaths | ForEach-Object { Get-Item -LiteralPath $_ })
    if ($sources.Count -ne $names.Count) {
        throw "COFF extraction produced $($sources.Count) objects, expected $($names.Count)"
    }
    $members = @()
    foreach ($source in $sources) {
        $normalized = "$($source.FullName).normalized"
        $null = & $LlvmObjcopy --strip-debug $source.FullName $normalized
        if ($LASTEXITCODE -ne 0) {
            throw "llvm-objcopy failed for $($source.FullName)"
        }
        $bytes = [IO.File]::ReadAllBytes($normalized)
        if ($bytes.Length -lt 8 -or $bytes[0] -ne 0x64 -or $bytes[1] -ne 0x86) {
            throw "$($source.Name) is not an x64 COFF object"
        }
        foreach ($index in 4..7) {
            $bytes[$index] = 0
        }
        [IO.File]::WriteAllBytes($source.FullName, $bytes)
        Remove-Item -LiteralPath $normalized -Force
        $members += $source.Name
    }

    Push-Location $work
    try {
        $null = & $LlvmAr rcD $Destination @members
        if ($LASTEXITCODE -ne 0) {
            throw "llvm-ar failed to create $Destination"
        }
    }
    finally {
        Pop-Location
    }
    Remove-Item -LiteralPath $work -Recurse -Force
}

function Write-Utf8Json {
    param(
        [string]$Path,
        [object]$Value
    )

    $json = ConvertTo-Json -InputObject $Value -Depth 8
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, $json + "`n", $encoding)
}

function Reset-EvidenceSubdirectory {
    param(
        [string]$EvidenceRoot,
        [string]$Name
    )

    $resolvedRoot = [IO.Path]::GetFullPath($EvidenceRoot).TrimEnd('\', '/')
    $destination = [IO.Path]::GetFullPath((Join-Path $resolvedRoot $Name))
    if (-not $destination.StartsWith($resolvedRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to reset evidence outside $resolvedRoot`: $destination"
    }
    if (Test-Path -LiteralPath $destination) {
        Remove-Item -LiteralPath $destination -Recurse -Force
    }
    New-Item -ItemType Directory -Path $destination | Out-Null
    return $destination
}

function Export-FinalArchiveMembers {
    param(
        [string]$Archive,
        [string]$Destination,
        [string]$InventoryPath,
        [string]$LlvmAr
    )

    $memberOutput = @(& $LlvmAr t $Archive)
    $memberExitCode = $LASTEXITCODE
    if ($memberExitCode -ne 0 -or $memberOutput.Count -eq 0) {
        throw "cannot enumerate final COFF members in $Archive"
    }
    $memberNames = @($memberOutput | ForEach-Object { $_.ToString() })
    $memberSet = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($memberName in $memberNames) {
        $leaf = Split-Path $memberName -Leaf
        if ([string]::IsNullOrWhiteSpace($leaf) -or $leaf -ne $memberName) {
            throw "final COFF archive contains an unsafe member name: $memberName"
        }
        if (-not $memberSet.Add($memberName)) {
            throw "final COFF archive contains a duplicate member name: $memberName"
        }
    }

    New-Item -ItemType Directory -Path $Destination | Out-Null
    Push-Location $Destination
    try {
        $null = & $LlvmAr x $Archive
        $extractExitCode = $LASTEXITCODE
        if ($extractExitCode -ne 0) {
            throw "cannot extract final COFF members from $Archive"
        }
    }
    finally {
        Pop-Location
    }

    $inventoryMembers = @()
    for ($index = 0; $index -lt $memberNames.Count; $index++) {
        $memberName = $memberNames[$index]
        $memberPath = Join-Path $Destination $memberName
        if (-not (Test-Path -LiteralPath $memberPath -PathType Leaf)) {
            throw "final COFF member was not extracted: $memberName"
        }
        $member = Get-Item -LiteralPath $memberPath
        $inventoryMembers += [pscustomobject][ordered]@{
            ordinal = $index + 1
            name = $memberName
            size = [int64]$member.Length
            sha256 = Get-Sha256 $member.FullName
        }
    }
    $archiveFile = Get-Item -LiteralPath $Archive
    Write-Utf8Json $InventoryPath ([ordered]@{
        schema_version = 1
        archive = $archiveFile.Name
        archive_size = [int64]$archiveFile.Length
        archive_sha256 = Get-Sha256 $archiveFile.FullName
        members = [object[]]$inventoryMembers
    })
}

function Export-BuildEvidence {
    param(
        [string]$Label,
        [string]$Prepared,
        [string]$EvidenceRoot,
        [string]$LlvmAr
    )

    $buildEvidence = Reset-EvidenceSubdirectory $EvidenceRoot $Label
    $preparedEvidence = Join-Path $buildEvidence "prepared"
    New-Item -ItemType Directory -Path $preparedEvidence | Out-Null
    Get-ChildItem -LiteralPath $Prepared -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $preparedEvidence -Recurse -Force
    }

    $buildRoot = Split-Path $Prepared -Parent
    $rawArchive = Join-Path (Join-Path $buildRoot "raw") $ArchivePath
    $finalArchive = Join-Path $Prepared $ArchivePath
    foreach ($archive in @($rawArchive, $finalArchive)) {
        if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
            throw "cannot collect reproducibility evidence because archive is missing: $archive"
        }
    }

    $rawEvidence = Join-Path $buildEvidence "archives\raw"
    New-Item -ItemType Directory -Force -Path $rawEvidence | Out-Null
    Copy-Item -LiteralPath $rawArchive -Destination (Join-Path $rawEvidence $ArchiveFileName)

    $membersEvidence = Join-Path $buildEvidence "final-members"
    $inventoryPath = Join-Path $buildEvidence "final-members.json"
    Export-FinalArchiveMembers $finalArchive $membersEvidence $inventoryPath $LlvmAr
}

function Export-ReproducibilityEvidence {
    param(
        [string]$FirstPrepared,
        [string]$SecondPrepared,
        [string]$EvidenceRoot,
        [string[]]$ComparedPaths,
        [string]$LlvmAr
    )

    Assert-NonRootDirectory $EvidenceRoot "libghostty reproducibility evidence"
    New-Item -ItemType Directory -Force -Path $EvidenceRoot | Out-Null
    $comparisonPath = Join-Path $EvidenceRoot "comparison.json"
    if (Test-Path -LiteralPath $comparisonPath) {
        Remove-Item -LiteralPath $comparisonPath -Force
    }

    Export-BuildEvidence "build-1" $FirstPrepared $EvidenceRoot $LlvmAr
    Export-BuildEvidence "build-2" $SecondPrepared $EvidenceRoot $LlvmAr

    $comparisons = @(
        foreach ($relative in $ComparedPaths) {
            $left = Get-Item -LiteralPath (Join-Path $FirstPrepared $relative)
            $right = Get-Item -LiteralPath (Join-Path $SecondPrepared $relative)
            $leftSha = Get-Sha256 $left.FullName
            $rightSha = Get-Sha256 $right.FullName
            [pscustomobject][ordered]@{
                path = $relative.Replace('\', '/')
                left_size = [int64]$left.Length
                left_sha256 = $leftSha
                right_size = [int64]$right.Length
                right_sha256 = $rightSha
                equal = $leftSha -eq $rightSha
            }
        }
    )
    Write-Utf8Json $comparisonPath ([ordered]@{
        schema_version = 1
        target = $Target
        source_sha = $SourceSha
        source_date_epoch = $SourceDateEpoch
        source_patch = [ordered]@{
            path = $SourcePatchPath
            sha256 = $SourcePatchSha
            target = $SourcePatchTarget
            input_sha256 = $SourcePatchInputSha
            output_sha256 = $SourcePatchOutputSha
        }
        toolchain = [ordered]@{
            zig_version = $ZigVersion
            zig_archive_url = $ZigArchiveUrl
            zig_archive_sha256 = $ZigArchiveSha
            zig_executable_sha256 = $ZigExecutableSha
            zig_image_base = $ZigImageBase
            zig_dll_characteristics = $ZigDllCharacteristics
            msvc_toolset = $MsvcToolset
            windows_sdk = $WindowsSdk
            llvm_version = $LlvmVersion
        }
        build = [ordered]@{
            mode = $BuildMode
            seed = $BuildSeed
            jobs = $BuildJobs
            canonical_source_path = $CanonicalSourcePath
            canonical_cache_path = "$CanonicalSourcePath/.paneflow-zig-cache"
            canonical_prefix_path = "$CanonicalSourcePath/.paneflow-zig-output"
            archive_normalization = $Normalization
        }
        left = "build-1"
        right = "build-2"
        files = [object[]]$comparisons
    })
    return $comparisons
}

if ([string]::IsNullOrWhiteSpace($SourceDir)) {
    throw "PANEFLOW_GHOSTTY_SOURCE_DIR or -SourceDir must point to the pinned Ghostty checkout"
}
$SourceDir = (Resolve-Path -LiteralPath $SourceDir).Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $Root "target\libghostty\$Target"
}
$OutputDir = [IO.Path]::GetFullPath($OutputDir)

$SourceSha = Get-ManifestString "source_sha"
$ZigVersion = Get-ManifestString "zig_version"
$ZigArchiveUrl = Get-ManifestString "windows_zig_archive_url"
$ZigArchiveSha = Get-ManifestString "windows_zig_archive_sha256"
$ZigExecutableSha = Get-ManifestString "windows_zig_executable_sha256"
$ZigImageBase = Get-ManifestString "windows_zig_image_base"
$ZigDllCharacteristics = Get-ManifestString "windows_zig_dll_characteristics"
$SourcePatchPath = Get-ManifestString "windows_source_patch_path"
$SourcePatchSha = Get-ManifestString "windows_source_patch_sha256"
$SourcePatchTarget = Get-ManifestString "windows_source_patch_target"
$SourcePatchInputSha = Get-ManifestString "windows_source_patch_input_sha256"
$SourcePatchOutputSha = Get-ManifestString "windows_source_patch_output_sha256"
$HeaderPath = Get-ManifestString "header_path"
$HeaderSha = Get-ManifestString "header_sha256"
$BindingsPath = Get-ManifestString "bindings_path"
$BindingsSha = Get-ManifestString "bindings_sha256"
$BuildMode = Get-ManifestString "build_mode"
$BuildSeed = Get-ManifestString "windows_build_seed"
$BuildJobs = Get-ManifestString "windows_build_jobs"
$SourceDateEpoch = Get-ManifestString "windows_source_date_epoch"
$CanonicalSourcePath = Get-ManifestString "windows_canonical_source_path"
$MsvcToolset = Get-ManifestString "windows_msvc_toolset"
$WindowsSdk = Get-ManifestString "windows_sdk"
$LlvmVersion = Get-ManifestString "windows_llvm_version"
$Crt = Get-ManifestString "windows_crt"
$CxxRuntime = Get-ManifestString "windows_cxx_runtime"
$ArchivePath = (Get-ManifestTargetString $Target "archive_path").Replace('/', '\')
$ArchiveFileName = Split-Path $ArchivePath -Leaf
$ZigTarget = Get-ManifestTargetString $Target "zig_target"
$Simd = Get-ManifestTargetBoolean $Target "simd"
$SimdText = $Simd.ToString().ToLowerInvariant()
$SystemLibraries = @(Get-ManifestTargetStringArray $Target "system_libraries")
$SystemLibraryArgs = @($SystemLibraries | ForEach-Object { "$_.lib" })
$SystemLibrariesText = $SystemLibraryArgs -join ','
$Normalization = Get-ManifestTargetString $Target "archive_normalization"
$ExpectedArchiveSha = Get-ManifestTargetString $Target "archive_sha256"
$ExpectedBuildInfoSha = Get-ManifestTargetString $Target "build_info_sha256"
$ExpectedHeadersIndexSha = Get-ManifestTargetString $Target "headers_index_sha256"
$ExpectedSymbolsSha = Get-ManifestTargetString $Target "symbols_sha256"

$insideWorkTree = (& git -C $SourceDir rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $insideWorkTree -ne "true") {
    throw "$SourceDir is not a Ghostty Git checkout"
}
$actualSha = (& git -C $SourceDir rev-parse HEAD).Trim()
if ($actualSha -ne $SourceSha) {
    throw "Ghostty source mismatch: expected $SourceSha, got $actualSha"
}
$sourceStatus = @(& git -C $SourceDir status --porcelain --untracked-files=all)
if ($sourceStatus.Count -ne 0) {
    throw "Ghostty source must be a clean checkout of $SourceSha"
}
$actualEpoch = (& git -C $SourceDir show -s --format=%ct HEAD).Trim()
if ($actualEpoch -ne $SourceDateEpoch) {
    throw "Ghostty commit epoch mismatch: expected $SourceDateEpoch, got $actualEpoch"
}

$zigCommand = Get-Command $Zig -ErrorAction SilentlyContinue
if ($null -eq $zigCommand -and (Test-Path -LiteralPath $Zig)) {
    $ZigPath = (Resolve-Path -LiteralPath $Zig).Path
}
elseif ($null -ne $zigCommand) {
    $ZigPath = $zigCommand.Source
}
else {
    throw "libghostty requires Zig $ZigVersion; pass -Zig or add it to PATH"
}
if ((Get-Sha256 $ZigPath) -ne $ZigExecutableSha) {
    throw "Zig $ZigVersion executable checksum mismatch: $ZigPath"
}
$zigMetadata = Get-Pe64ImageMetadata $ZigPath
if ($zigMetadata.ImageBase -ne $ZigImageBase -or
    $zigMetadata.DllCharacteristics -ne $ZigDllCharacteristics) {
    throw "Zig $ZigVersion PE metadata drift: expected image base $ZigImageBase and DLL characteristics $ZigDllCharacteristics, got $($zigMetadata.ImageBase) and $($zigMetadata.DllCharacteristics)"
}
$ZigLibDir = Join-Path (Split-Path $ZigPath -Parent) "lib"
if (-not (Test-Path -LiteralPath $ZigLibDir -PathType Container)) {
    throw "libghostty requires the Zig $ZigVersion library beside $ZigPath"
}
$actualZig = (& $ZigPath version).Trim()
if ($LASTEXITCODE -ne 0 -or $actualZig -ne $ZigVersion) {
    throw "libghostty requires Zig $ZigVersion, found $actualZig"
}

$SourcePatch = [IO.Path]::GetFullPath((Join-Path $Root $SourcePatchPath))
if (-not $SourcePatch.StartsWith($Root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $SourcePatch -PathType Leaf)) {
    throw "manifest-pinned Ghostty source patch must be a repository file: $SourcePatchPath"
}
if ((Get-NormalizedTextSha256 $SourcePatch) -ne $SourcePatchSha) {
    throw "Ghostty source patch checksum mismatch at $SourcePatch"
}
$patchStats = @(& git apply --unidiff-zero --numstat $SourcePatch 2>&1 | ForEach-Object { $_.ToString() })
if ($LASTEXITCODE -ne 0 -or $patchStats.Count -ne 1 -or
    $patchStats[0] -notmatch '^[0-9-]+\t[0-9-]+\t(.+)$' -or
    $matches[1].Replace('\', '/') -ne $SourcePatchTarget.Replace('\', '/')) {
    throw "Ghostty source patch must modify exactly $SourcePatchTarget"
}
$sourcePatchTargetPath = [IO.Path]::GetFullPath((Join-Path $SourceDir $SourcePatchTarget))
if (-not $sourcePatchTargetPath.StartsWith($SourceDir + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $sourcePatchTargetPath -PathType Leaf)) {
    throw "manifest-pinned Ghostty source patch target is invalid: $SourcePatchTarget"
}
if ((Get-NormalizedTextSha256 $sourcePatchTargetPath) -ne $SourcePatchInputSha) {
    throw "Ghostty source patch input checksum mismatch at $sourcePatchTargetPath"
}

$sourceHeader = Join-Path $SourceDir $HeaderPath
if ((Get-Sha256 $sourceHeader) -ne $HeaderSha) {
    throw "Ghostty header checksum mismatch at $sourceHeader"
}
$bindings = Join-Path $Root $BindingsPath
if ((Get-NormalizedTextSha256 $bindings) -ne $BindingsSha) {
    throw "Paneflow bindings checksum mismatch at $bindings"
}

$vsInstallation = Import-MsvcEnvironment -MsvcToolset $MsvcToolset -WindowsSdk $WindowsSdk
$llvmRoot = Join-Path $vsInstallation "VC\Tools\Llvm\x64\bin"
$clExe = Resolve-Tool "cl.exe"
$dumpbinExe = Resolve-Tool "dumpbin.exe"
$llvmObjcopy = Resolve-Tool "llvm-objcopy.exe" @((Join-Path $llvmRoot "llvm-objcopy.exe"))
$llvmAr = Resolve-Tool "llvm-ar.exe" @((Join-Path $llvmRoot "llvm-ar.exe"))
$llvmNm = Resolve-Tool "llvm-nm.exe" @((Join-Path $llvmRoot "llvm-nm.exe"))
$llvmReadobj = Resolve-Tool "llvm-readobj.exe" @((Join-Path $llvmRoot "llvm-readobj.exe"))
$tarExe = Resolve-Tool "tar.exe"

if ($env:VCToolsVersion.TrimEnd('\') -ne $MsvcToolset) {
    throw "libghostty requires MSVC toolset $MsvcToolset, found $($env:VCToolsVersion)"
}
if ($env:WindowsSDKVersion.TrimEnd('\') -ne $WindowsSdk) {
    throw "libghostty requires Windows SDK $WindowsSdk, found $($env:WindowsSDKVersion)"
}
$llvmVersionOutput = @(& $llvmObjcopy --version)
if ($LASTEXITCODE -ne 0 -or -not ($llvmVersionOutput -match "LLVM version $([regex]::Escape($LlvmVersion))")) {
    throw "libghostty requires LLVM normalization tools $LlvmVersion"
}

if ($Normalization -ne "pinned-formatter-patch+fixed-source-cache-prefix+zig-build-seed0-j1+llvm-objcopy-strip-debug+coff-timestamp-zero+llvm-ar-D+ordinal-order") {
    throw "unsupported Windows archive normalization: $Normalization"
}

$requiredSymbols = @(
    "ghostty_build_info",
    "ghostty_free",
    "ghostty_key_encoder_encode",
    "ghostty_render_state_update",
    "ghostty_terminal_free",
    "ghostty_terminal_new",
    "ghostty_terminal_resize",
    "ghostty_terminal_vt_write"
)
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("paneflow-libghostty-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) {
    $EvidenceDir = Join-Path $tempRoot "reproducibility-evidence"
}
else {
    $EvidenceDir = [IO.Path]::GetFullPath($EvidenceDir)
}
$succeeded = $false
$fixedSourceCreated = $false
$canonicalSource = [IO.Path]::GetFullPath($CanonicalSourcePath)
$sourceArchive = Join-Path $tempRoot "source.tar"
$buildSource = $null

function Initialize-CanonicalSource {
    $publicRoot = [IO.Path]::GetFullPath("C:\Users\Public")
    if (-not $canonicalSource.StartsWith($publicRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "canonical libghostty source must stay under $publicRoot"
    }
    if (Test-Path -LiteralPath $canonicalSource) {
        throw "canonical libghostty source path is already in use: $canonicalSource"
    }
    New-Item -ItemType Directory -Path $canonicalSource | Out-Null
    $script:fixedSourceCreated = $true
    $null = & git -C $SourceDir archive --format=tar -o $sourceArchive $SourceSha
    if ($LASTEXITCODE -ne 0) {
        throw "cannot export the pinned Ghostty source tree"
    }
    $null = & $tarExe -xf $sourceArchive -C $canonicalSource
    if ($LASTEXITCODE -ne 0) {
        throw "cannot extract the pinned Ghostty source tree"
    }
    if ((Get-Sha256 (Join-Path $canonicalSource $HeaderPath)) -ne $HeaderSha) {
        throw "canonical Ghostty export has an unexpected header checksum"
    }
    $canonicalPatchTarget = [IO.Path]::GetFullPath((Join-Path $canonicalSource $SourcePatchTarget))
    if (-not $canonicalPatchTarget.StartsWith($canonicalSource + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        (Get-NormalizedTextSha256 $canonicalPatchTarget) -ne $SourcePatchInputSha) {
        throw "canonical Ghostty export has an unexpected source patch input"
    }
    Push-Location $canonicalSource
    try {
        $patchCheck = @(& git apply --unidiff-zero --check --whitespace=error-all $SourcePatch 2>&1 | ForEach-Object { $_.ToString() })
        if ($LASTEXITCODE -ne 0) {
            throw "manifest-pinned Ghostty source patch does not apply cleanly`n$($patchCheck -join "`n")"
        }
        $patchOutput = @(& git apply --unidiff-zero --whitespace=error-all $SourcePatch 2>&1 | ForEach-Object { $_.ToString() })
        if ($LASTEXITCODE -ne 0) {
            throw "cannot apply manifest-pinned Ghostty source patch`n$($patchOutput -join "`n")"
        }
    }
    finally {
        Pop-Location
    }
    if ((Get-NormalizedTextSha256 $canonicalPatchTarget) -ne $SourcePatchOutputSha) {
        throw "canonical Ghostty source patch output checksum mismatch"
    }
    return $canonicalSource
}

function Invoke-NativeBuild {
    param([string]$Label)

    $buildRoot = Join-Path $tempRoot $Label
    $raw = Join-Path $buildRoot "raw"
    $prepared = Join-Path $buildRoot "prepared"
    $cacheRoot = Join-Path $buildSource ".paneflow-zig-cache"
    $zigPrefix = Join-Path $buildSource ".paneflow-zig-output"
    $globalCache = Join-Path $cacheRoot "global"
    $localCache = Join-Path $cacheRoot "local"
    foreach ($fixedPath in @($cacheRoot, $zigPrefix)) {
        if (Test-Path -LiteralPath $fixedPath) {
            throw "fixed reproducibility path is already in use: $fixedPath"
        }
    }
    New-Item -ItemType Directory -Force -Path $prepared, $globalCache, $localCache | Out-Null

    $oldGlobal = $env:ZIG_GLOBAL_CACHE_DIR
    $oldLocal = $env:ZIG_LOCAL_CACHE_DIR
    $oldEpoch = $env:SOURCE_DATE_EPOCH
    $oldNoColor = $env:NO_COLOR
    $env:ZIG_GLOBAL_CACHE_DIR = $globalCache
    $env:ZIG_LOCAL_CACHE_DIR = $localCache
    $env:SOURCE_DATE_EPOCH = $SourceDateEpoch
    $env:NO_COLOR = "1"
    Push-Location $buildSource
    try {
        $zigOutput = @(& $ZigPath build --zig-lib-dir $ZigLibDir --verbose --seed $BuildSeed "-j$BuildJobs" -Demit-lib-vt=true "-Dtarget=$ZigTarget" "-Doptimize=$BuildMode" "-Dsimd=$SimdText" --prefix $zigPrefix 2>&1 | ForEach-Object { $_.ToString() })
        $zigExitCode = $LASTEXITCODE
        if ($zigExitCode -ne 0) {
            throw "SIMD libghostty build failed with exit code $zigExitCode; raw reproducer is $buildRoot`n$($zigOutput -join "`n")"
        }
    }
    finally {
        Pop-Location
        $env:ZIG_GLOBAL_CACHE_DIR = $oldGlobal
        $env:ZIG_LOCAL_CACHE_DIR = $oldLocal
        $env:SOURCE_DATE_EPOCH = $oldEpoch
        $env:NO_COLOR = $oldNoColor
    }

    if (-not (Test-Path -LiteralPath $zigPrefix -PathType Container)) {
        throw "Zig did not emit the fixed install prefix $zigPrefix"
    }
    Move-Item -LiteralPath $zigPrefix -Destination $raw
    $rawArchive = Join-Path $raw $ArchivePath
    $rawInclude = Join-Path $raw "include"
    if (-not (Test-Path -LiteralPath $rawArchive)) {
        throw "missing SIMD static archive: $rawArchive"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $rawInclude "ghostty\vt.h"))) {
        throw "missing installed libghostty headers: $rawInclude"
    }
    $archive = Join-Path $prepared $ArchivePath
    $archiveDirectory = Split-Path $archive -Parent
    $includeDir = Join-Path $prepared "include"
    New-Item -ItemType Directory -Force -Path $archiveDirectory, $includeDir | Out-Null
    Normalize-CoffArchive $rawArchive $archive $llvmObjcopy $llvmAr
    Copy-Item -LiteralPath (Join-Path $rawInclude "ghostty") -Destination $includeDir -Recurse -Force
    $bindingText = [IO.File]::ReadAllText($bindings).Replace("`r`n", "`n").Replace("`r", "`n")
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText((Join-Path $prepared "bindings.rs"), $bindingText, $encoding)

    $headerLines = @()
    $headerPaths = Sort-Ordinal @(Get-ChildItem -LiteralPath $includeDir -Recurse -File | ForEach-Object { $_.FullName })
    foreach ($headerFile in $headerPaths) {
        $file = Get-Item -LiteralPath $headerFile
        $relative = $file.FullName.Substring($includeDir.Length).TrimStart('\', '/').Replace('\', '/')
        $headerLines += "$(Get-Sha256 $file.FullName)  $relative"
    }
    $headerIndex = Join-Path $prepared "headers.sha256"
    Write-Utf8Lines $headerIndex $headerLines

    $symbolOutput = @(& $llvmNm -j -g -U $archive)
    $symbolExitCode = $LASTEXITCODE
    if ($symbolExitCode -ne 0) {
        throw "cannot inspect symbols in $archive"
    }
    $symbolSet = [Collections.Generic.SortedSet[string]]::new([StringComparer]::Ordinal)
    foreach ($symbol in $symbolOutput) {
        if (-not [string]::IsNullOrWhiteSpace($symbol)) {
            $null = $symbolSet.Add($symbol)
        }
    }
    $symbols = [string[]]@($symbolSet)
    foreach ($symbol in $requiredSymbols) {
        if (-not $symbolSet.Contains($symbol)) {
            throw "SIMD archive is missing required symbol $symbol"
        }
    }
    $symbolFile = Join-Path $prepared "symbols.txt"
    Write-Utf8Lines $symbolFile $symbols

    $members = @(& $llvmAr t $archive)
    if ($LASTEXITCODE -ne 0 -or $members.Count -eq 0) {
        throw "cannot inspect archive members in $archive"
    }
    $headers = @(& $llvmReadobj --file-headers $archive)
    if ($LASTEXITCODE -ne 0) {
        throw "cannot inspect COFF architecture in $archive"
    }
    $machineCount = @($headers | Where-Object { $_ -match 'Machine: IMAGE_FILE_MACHINE_AMD64' }).Count
    if ($machineCount -ne $members.Count -or @($headers | Where-Object { $_ -match '^Format:' -and $_ -notmatch 'COFF-x86-64' }).Count -ne 0) {
        throw "archive for $Target contains a non-x64 COFF member: $archive"
    }
    $directives = @(& $llvmReadobj --coff-directives $archive)
    if ($LASTEXITCODE -ne 0 -or -not ($directives -match 'RuntimeLibrary=MT_StaticRelease') -or -not ($directives -match 'DEFAULTLIB:libcpmt.lib')) {
        throw "archive does not declare the reviewed MSVC static CRT model"
    }

    $smokeObject = Join-Path $buildRoot "windows-smoke.obj"
    $smokeExe = Join-Path $buildRoot "windows-smoke.exe"
    $clOutput = @(& $clExe /nologo /W4 /WX /std:c11 /MT "/I$includeDir" "/Fo$smokeObject" $SmokeSource $archive @SystemLibraryArgs /link "/out:$smokeExe" 2>&1)
    $clExitCode = $LASTEXITCODE
    if ($clExitCode -ne 0) {
        throw "MSVC static smoke failed to link; reproducer is $buildRoot`n$($clOutput -join "`n")"
    }
    $smokeOutput = @(& $smokeExe 2>&1)
    $smokeExitCode = $LASTEXITCODE
    if ($smokeExitCode -ne 0) {
        throw "MSVC static smoke failed at runtime with exit code $smokeExitCode`n$($smokeOutput -join "`n")"
    }
    $dependencies = @(& $dumpbinExe /nologo /dependents $smokeExe)
    if ($LASTEXITCODE -ne 0 -or $dependencies -match 'ghostty-vt\.dll') {
        throw "MSVC static smoke has a forbidden ghostty-vt.dll dependency"
    }

    $archiveSha = Get-Sha256 $archive
    $headerIndexSha = Get-Sha256 $headerIndex
    $symbolsSha = Get-Sha256 $symbolFile
    $buildInfo = Join-Path $prepared "build-info.txt"
    Write-Utf8Lines $buildInfo @(
        "source_sha=$SourceSha",
        "source_date_epoch=$SourceDateEpoch",
        "source_patch_path=$SourcePatchPath",
        "source_patch_sha256=$SourcePatchSha",
        "source_patch_target=$SourcePatchTarget",
        "source_patch_input_sha256=$SourcePatchInputSha",
        "source_patch_output_sha256=$SourcePatchOutputSha",
        "zig_version=$ZigVersion",
        "zig_archive_url=$ZigArchiveUrl",
        "zig_archive_sha256=$ZigArchiveSha",
        "zig_executable_sha256=$ZigExecutableSha",
        "zig_image_base=$ZigImageBase",
        "zig_dll_characteristics=$ZigDllCharacteristics",
        "header_sha256=$HeaderSha",
        "headers_sha256=$headerIndexSha",
        "bindings_sha256=$BindingsSha",
        "rust_target=$Target",
        "zig_target=$ZigTarget",
        "optimize=$BuildMode",
        "simd=$SimdText",
        "build_seed=$BuildSeed",
        "build_jobs=$BuildJobs",
        "canonical_source_path=$CanonicalSourcePath",
        "canonical_cache_path=$CanonicalSourcePath/.paneflow-zig-cache",
        "canonical_prefix_path=$CanonicalSourcePath/.paneflow-zig-output",
        "archive_normalization=$Normalization",
        "archive_sha256=$archiveSha",
        "symbol_count=$($symbols.Count)",
        "symbols_sha256=$symbolsSha",
        "archive_members=$($members -join ',')",
        "msvc_toolset=$MsvcToolset",
        "windows_sdk=$WindowsSdk",
        "llvm_version=$LlvmVersion",
        "crt=$Crt",
        "cxx_runtime=$CxxRuntime",
        "system_libraries=$SystemLibrariesText"
    )
    $resolvedCache = [IO.Path]::GetFullPath($cacheRoot)
    if ((Split-Path $resolvedCache -Parent) -ne $buildSource -or (Split-Path $resolvedCache -Leaf) -ne ".paneflow-zig-cache") {
        throw "refusing to remove unexpected Zig cache path $resolvedCache"
    }
    Remove-Item -LiteralPath $resolvedCache -Recurse -Force
    return $prepared
}

try {
    $buildSource = Initialize-CanonicalSource
    $first = Invoke-NativeBuild "build-1"
    if ($VerifyReproducible) {
        $second = Invoke-NativeBuild "build-2"
        $comparedPaths = @($ArchivePath, "headers.sha256", "bindings.rs", "symbols.txt", "build-info.txt")
        $comparisons = @(Export-ReproducibilityEvidence $first $second $EvidenceDir $comparedPaths $llvmAr)
        $mismatches = @($comparisons | Where-Object { -not $_.equal })
        if ($mismatches.Count -ne 0) {
            $mismatch = $mismatches[0]
            throw "reproducibility mismatch for $($mismatch.path) between clean builds (left sha256 $($mismatch.left_sha256), right sha256 $($mismatch.right_sha256)); evidence is $EvidenceDir"
        }
    }

    $actualArchiveSha = Get-Sha256 (Join-Path $first $ArchivePath)
    if ($actualArchiveSha -ne $ExpectedArchiveSha) {
        throw "canonical Windows archive hash differs from manifest; expected $ExpectedArchiveSha, got $actualArchiveSha"
    }
    foreach ($metadata in @(
        @{ Path = "build-info.txt"; Expected = $ExpectedBuildInfoSha },
        @{ Path = "headers.sha256"; Expected = $ExpectedHeadersIndexSha },
        @{ Path = "symbols.txt"; Expected = $ExpectedSymbolsSha }
    )) {
        $metadataPath = Join-Path $first $metadata.Path
        $metadataSha = Get-Sha256 $metadataPath
        if ($metadataSha -ne $metadata.Expected) {
            throw "canonical Windows metadata hash differs for $($metadata.Path); expected $($metadata.Expected), got $metadataSha"
        }
    }

    Assert-NonRootDirectory $OutputDir "libghostty publication"
    $outputParent = Split-Path $OutputDir -Parent
    $outputLeaf = Split-Path $OutputDir -Leaf
    if ([string]::IsNullOrWhiteSpace($outputParent) -or [string]::IsNullOrWhiteSpace($outputLeaf)) {
        throw "cannot derive a safe publication parent from $OutputDir"
    }
    New-Item -ItemType Directory -Force -Path $outputParent | Out-Null
    $publicationId = [guid]::NewGuid().ToString("N")
    $stagedOutput = Join-Path $outputParent ".$outputLeaf.stage-$publicationId"
    $backupOutput = Join-Path $outputParent ".$outputLeaf.backup-$publicationId"
    Assert-NonRootDirectory $stagedOutput "libghostty publication staging"
    Assert-NonRootDirectory $backupOutput "libghostty publication backup"
    New-Item -ItemType Directory -Path $stagedOutput | Out-Null
    Get-ChildItem -LiteralPath $first | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $stagedOutput -Recurse
    }

    $backupCreated = $false
    try {
        if (Test-Path -LiteralPath $OutputDir) {
            $existingBuildInfo = Join-Path $OutputDir "build-info.txt"
            if (-not (Test-Path -LiteralPath $existingBuildInfo -PathType Leaf) -or
                -not ([IO.File]::ReadAllLines($existingBuildInfo) -contains "rust_target=$Target")) {
                throw "refusing to replace unrecognized output directory $OutputDir"
            }
            Move-Item -LiteralPath $OutputDir -Destination $backupOutput
            $backupCreated = $true
        }
        Move-Item -LiteralPath $stagedOutput -Destination $OutputDir
    }
    catch {
        $publicationError = $_.Exception.Message
        if ($backupCreated -and -not (Test-Path -LiteralPath $OutputDir)) {
            Move-Item -LiteralPath $backupOutput -Destination $OutputDir
            $backupCreated = $false
        }
        throw "cannot atomically publish $Target at $OutputDir`: $publicationError"
    }
    finally {
        if (Test-Path -LiteralPath $stagedOutput) {
            Remove-Item -LiteralPath $stagedOutput -Recurse -Force
        }
    }
    if ($backupCreated) {
        Remove-Item -LiteralPath $backupOutput -Recurse -Force
    }
    Write-Host "prepared $Target at $OutputDir"
    $succeeded = $true
}
finally {
    if ($fixedSourceCreated) {
        $expectedSource = [IO.Path]::GetFullPath($CanonicalSourcePath)
        if ($canonicalSource -ne $expectedSource -or (Split-Path $canonicalSource -Leaf) -notlike "paneflow-libghostty-*") {
            throw "refusing to remove unexpected canonical source path $canonicalSource"
        }
        Remove-Item -LiteralPath $canonicalSource -Recurse -Force
    }
    if ($succeeded) {
        $tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
        if (-not $resolvedTemp.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolvedTemp -Leaf) -notlike "paneflow-libghostty-*") {
            throw "refusing to remove unexpected temporary path $resolvedTemp"
        }
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
    else {
        Write-Warning "libghostty build evidence preserved at $tempRoot"
    }
}
