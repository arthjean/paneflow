[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Tag,
    [Parameter(Mandatory = $true)][string] $MsiVersion,
    [Parameter(Mandatory = $true)][string] $SmokeDir,
    [Parameter(Mandatory = $true)][string] $BaselineDir
)

$ErrorActionPreference = 'Stop'


New-Item -ItemType Directory -Force -Path $SmokeDir | Out-Null
$version = $Tag.TrimStart('v')
$msiVersion = $MsiVersion.TrimStart('v')
if ($env:RELEASE_MODE -eq 'release' -and $msiVersion -ne $version) {
    throw "Published MSI ProductVersion '$msiVersion' must match release version '$version'"
}
if ($msiVersion -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$') {
    throw "Windows MSI upgrade smoke requires a valid semver, got '$msiVersion'"
}
if ($msiVersion -match '^\d+\.\d+\.\d+$' -and [version]$msiVersion -le [version]'0.8.0') {
    throw "Windows MSI upgrade smoke requires an MSI ProductVersion newer than the pinned v0.8.0 baseline, got '$msiVersion'"
}

$msis = @(Get-ChildItem -LiteralPath 'release-assets' -Filter 'paneflow-*-x86_64-pc-windows-msvc.msi' -File)
if ($msis.Count -ne 1) {
    $found = $msis | Sort-Object Name | ForEach-Object { $_.Name }
    throw "Expected exactly one Windows MSI, found $($msis.Count): $($found -join ', ')"
}
$msi = $msis[0]
if ($msi.Length -le 0) {
    throw "MSI is empty: $($msi.FullName)"
}

$baselineMsis = @(Get-ChildItem -LiteralPath $BaselineDir -Filter 'paneflow-0.8.0-x86_64-pc-windows-msvc.msi' -File)
if ($baselineMsis.Count -ne 1) {
    throw "Expected exactly one pinned v0.8.0 MSI, found $($baselineMsis.Count)"
}
$baselineMsi = $baselineMsis[0]

$publisherSource = Get-Content -LiteralPath 'src-app/src/update/windows/msi.rs' -Raw
$publisherMatch = [regex]::Match(
    $publisherSource,
    'const\s+WINDOWS_PUBLISHER_ORGANIZATION:\s*&str\s*=\s*"([^"]+)";'
)
if (-not $publisherMatch.Success) {
    throw 'Could not read WINDOWS_PUBLISHER_ORGANIZATION from src-app/src/update/windows/msi.rs'
}
$expectedPublisher = $publisherMatch.Groups[1].Value

function Assert-Sha256Sidecar {
    param(
        [Parameter(Mandatory = $true)][string] $Path,
        [Parameter(Mandatory = $true)][string] $Sidecar,
        [Parameter(Mandatory = $true)][string] $Label
    )

    if (-not (Test-Path -LiteralPath $Sidecar -PathType Leaf)) {
        throw "$Label is missing SHA-256 sidecar: $Sidecar"
    }
    $expectedHash = ((Get-Content -LiteralPath $Sidecar -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    if ($expectedHash -notmatch '^[a-f0-9]{64}$') {
        throw "$Label has invalid SHA-256 sidecar content in $Sidecar"
    }
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "$Label SHA-256 mismatch: expected $expectedHash, got $actualHash"
    }
}

function Assert-MsiTrust {
    param(
        [Parameter(Mandatory = $true)][System.IO.FileInfo] $Msi,
        [Parameter(Mandatory = $true)][string] $Publisher,
        [Parameter(Mandatory = $true)][string] $Label
    )

    Assert-Sha256Sidecar -Path $Msi.FullName -Sidecar "$($Msi.FullName).sha256" -Label $Label
    $signature = Get-AuthenticodeSignature -LiteralPath $Msi.FullName
    if ($signature.Status -ne 'Valid' -or $null -eq $signature.SignerCertificate) {
        throw "$Label Authenticode signature is $($signature.Status): $($signature.StatusMessage)"
    }
    $publisherPattern = "(?:^|,\s*)O=$([regex]::Escape($Publisher))(?:,|$)"
    if ($signature.SignerCertificate.Subject -notmatch $publisherPattern) {
        throw "$Label publisher mismatch: expected O=$Publisher, got $($signature.SignerCertificate.Subject)"
    }
    Write-Host "$Label signed by $($signature.SignerCertificate.Subject)"
}

Assert-MsiTrust -Msi $baselineMsi -Publisher $expectedPublisher -Label 'Pinned v0.8.0 MSI'
Assert-MsiTrust -Msi $msi -Publisher $expectedPublisher -Label 'Candidate MSI'

$releaseReports = @(Get-ChildItem -LiteralPath 'release-assets' -Filter 'paneflow-*.libghostty-provenance.json' -File)
if ($releaseReports.Count -ne 1) {
    throw "Expected exactly one Windows libghostty provenance report, found $($releaseReports.Count)"
}
$releaseReportFile = $releaseReports[0]
Assert-Sha256Sidecar `
    -Path $releaseReportFile.FullName `
    -Sidecar "$($releaseReportFile.FullName).sha256" `
    -Label 'Windows libghostty provenance report'
$releaseReportText = Get-Content -LiteralPath $releaseReportFile.FullName -Raw
if ($releaseReportText -match '(?i)[a-z]:\\') {
    throw 'Windows libghostty provenance report contains a runner-local absolute path'
}
$releaseReport = $releaseReportText | ConvertFrom-Json
if ($releaseReport.target -ne 'x86_64-pc-windows-msvc' -or
    $releaseReport.linkage -ne 'static-candidate' -or
    $releaseReport.runtime_smoke_verified -ne $false) {
    throw 'Windows libghostty provenance report has the wrong target or linkage mode'
}

function Show-LogTail {
    param([Parameter(Mandatory = $true)][string] $Path)

    if (Test-Path -LiteralPath $Path) {
        Write-Host "--- tail of $Path ---"
        Get-Content -LiteralPath $Path -Tail 160 | ForEach-Object { Write-Host $_ }
    }
}

function Invoke-SmokeProcess {
    param(
        [Parameter(Mandatory = $true)][string] $FilePath,
        [Parameter(Mandatory = $true)][string[]] $Arguments,
        [Parameter(Mandatory = $true)][string] $Label,
        [int] $TimeoutSeconds = 300,
        [int[]] $AllowedExitCodes = @(0)
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.UseShellExecute = $false
    foreach ($argument in $Arguments) {
        [void] $startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::Start($startInfo)
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try {
            $process.Kill($true)
        } catch {
            Write-Warning "Could not kill timed-out process $($process.Id): $_"
        }
        throw "$Label timed out after $TimeoutSeconds seconds"
    }

    if ($AllowedExitCodes -notcontains $process.ExitCode) {
        throw "$Label exited with $($process.ExitCode)"
    }

    return $process.ExitCode
}

$complianceSources = [ordered]@{
    'LICENSE.txt' = 'LICENSE'
    'THIRD_PARTY_NOTICES.md' = 'native/libghostty/THIRD_PARTY_NOTICES.md'
    'libghostty-sbom.cdx.json' = 'native/libghostty/sbom.cdx.json'
    'libghostty-manifest.toml' = 'native/libghostty/manifest.toml'
    'libghostty-build-info.txt' = 'native/libghostty/prebuilt/x86_64-pc-windows-msvc/build-info.txt'
}

function Assert-NoMsiResidue {
    param([Parameter(Mandatory = $true)][string] $Label)

    if (-not (Test-Path -LiteralPath $installDir -PathType Container)) {
        return
    }
    $residue = @(Get-ChildItem -LiteralPath $installDir -Recurse -File -Force |
        ForEach-Object { [IO.Path]::GetRelativePath($installDir, $_.FullName) } |
        Sort-Object)
    if ($residue.Count -ne 0) {
        throw "$Label left installed files: $($residue -join ', ')"
    }
}

function Invoke-GhosttyLaunchSmoke {
    param([Parameter(Mandatory = $true)][string] $Exe)

    $stdout = Join-Path $SmokeDir 'backend-ghostty.stdout.log'
    $stderr = Join-Path $SmokeDir 'backend-ghostty.stderr.log'
    $smokeHome = Join-Path $SmokeDir 'backend-ghostty-home'
    New-Item -ItemType Directory -Force -Path $smokeHome | Out-Null
    $cwd = $SmokeDir.Replace('\', '\\')
    $session = '{"version":2,"active_workspace":0,"workspaces":[{"title":"smoke","cwd":"' + $cwd + '","tabs":[{"title":"","title_source":"preset","layout":{"type":"pane","surfaces":[{"surface_type":"terminal","name":"Terminal","command":null,"cwd":"' + $cwd + '","env":null,"focus":true}]}}]}]}'
    Set-Content -LiteralPath (Join-Path $smokeHome 'session.json') -Encoding utf8 -Value $session
    Set-Content -LiteralPath (Join-Path $smokeHome 'paneflow.json') -Encoding utf8 -Value '{}'
    Set-Content -LiteralPath (Join-Path $smokeHome 'window-state.json') -Encoding utf8 -Value '{"width":1400.0,"height":900.0}'
    Set-Content -LiteralPath (Join-Path $smokeHome 'telemetry_id') -Encoding utf8 -Value 'msi-smoke'
    $previousRustLog = $env:RUST_LOG
    $previousHome = $env:PANEFLOW_HOME
    $process = $null
    try {
        $env:RUST_LOG = 'paneflow::terminal::backend=info'
        $env:PANEFLOW_HOME = $smokeHome
        $process = Start-Process -FilePath $Exe `
            -PassThru `
            -RedirectStandardOutput $stdout `
            -RedirectStandardError $stderr
        Start-Sleep -Seconds 15
        if ($process.HasExited) {
            throw 'PaneFlow launch exited before the backend diagnostic was captured'
        }
    }
    finally {
        if ($null -ne $process -and -not $process.HasExited) {
            $process.Kill($true)
            $null = $process.WaitForExit(10000)
        }
        if ($null -eq $previousRustLog) {
            Remove-Item Env:RUST_LOG -ErrorAction SilentlyContinue
        }
        else {
            $env:RUST_LOG = $previousRustLog
        }
        if ($null -eq $previousHome) {
            Remove-Item Env:PANEFLOW_HOME -ErrorAction SilentlyContinue
        }
        else {
            $env:PANEFLOW_HOME = $previousHome
        }
    }

    $backendLogParts = foreach ($logPath in @($stdout, $stderr)) {
        if (Test-Path -LiteralPath $logPath -PathType Leaf) {
            Get-Content -LiteralPath $logPath -Raw
        }
    }
    $backendLog = @($backendLogParts) -join "`n"
    $expected = 'backend=ghostty failure_phase=none'
    if (-not $backendLog.Contains($expected)) {
        throw "PaneFlow launch did not emit the expected privacy-safe diagnostic '$expected'"
    }
    if ($backendLog -match '(?i)(command=|cwd=|clipboard=|osc_payload=|[a-z]:\\users\\)') {
        throw 'PaneFlow backend diagnostic contains a forbidden sensitive field'
    }
    foreach ($identity in @(
        "ghostty_source_sha=$($releaseReport.source_sha)",
        "ghostty_version=$($releaseReport.ghostty_version)"
    )) {
        if (-not $backendLog.Contains($identity)) {
            throw "PaneFlow launch did not emit runtime identity '$identity'"
        }
    }
    $runtimeEvidencePath = Join-Path $SmokeDir 'ghostty-runtime-evidence.json'
    [ordered]@{
        schema_version = 1
        binary_sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
        source_sha = $releaseReport.source_sha
        ghostty_version = $releaseReport.ghostty_version
        status = 'passed'
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $runtimeEvidencePath -Encoding utf8
    return $runtimeEvidencePath
}

$msiexec = Join-Path $env:WINDIR 'System32\msiexec.exe'
$installDir = Join-Path $env:ProgramFiles 'PaneFlow'
$pwsh = (Get-Command -Name pwsh -ErrorAction Stop).Source
$verifier = (Resolve-Path -LiteralPath 'scripts/verify-libghostty-windows.ps1').Path
$freshInstallLog = Join-Path $SmokeDir 'fresh-install.log'
$freshUninstallLog = Join-Path $SmokeDir 'fresh-uninstall.log'
$baselineInstallLog = Join-Path $SmokeDir 'baseline-install.log'
$upgradeLog = Join-Path $SmokeDir 'upgrade.log'
$uninstallLog = Join-Path $SmokeDir 'uninstall.log'
$installed = $false
$uninstallMsi = $baselineMsi.FullName

$freshInstalled = $false
try {
    try {
        Invoke-SmokeProcess `
            -FilePath $msiexec `
            -Arguments @('/i', $msi.FullName, '/qn', '/norestart', '/l*v', $freshInstallLog) `
            -Label 'candidate MSI fresh install'
    }
    catch {
        Show-LogTail -Path $freshInstallLog
        throw
    }
    $freshInstalled = $true
    $freshExe = Join-Path $installDir 'paneflow.exe'
    $freshVersion = (& $freshExe --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $freshVersion -ne "paneflow $version") {
        throw "Fresh candidate returned '$freshVersion', expected 'paneflow $version'"
    }

    $freshGhosttyEvidence = Invoke-GhosttyLaunchSmoke -Exe $freshExe

    $freshReportPath = Join-Path $SmokeDir 'fresh-install-libghostty-provenance.json'
    Invoke-SmokeProcess `
        -FilePath $pwsh `
        -Arguments @(
            '-NoProfile',
            '-File', $verifier,
            '-Target', 'x86_64-pc-windows-msvc',
            '-Binary', $freshExe,
            '-PackageRoot', $installDir,
            '-RuntimeEvidence', $freshGhosttyEvidence,
            '-ReportPath', $freshReportPath,
            '-AllowReleaseAssets'
        ) `
        -Label 'fresh-installed libghostty runtime verifier'
    $freshReport = Get-Content -LiteralPath $freshReportPath -Raw | ConvertFrom-Json
    if ($freshReport.linkage -ne 'static' -or
        $freshReport.runtime_smoke_verified -ne $true -or
        $freshReport.binary_sha256 -ne $releaseReport.binary_sha256) {
        throw 'Fresh-installed PaneFlow binary did not produce runtime-verified static provenance'
    }
}
finally {
    if ($freshInstalled) {
        try {
            Invoke-SmokeProcess `
                -FilePath $msiexec `
                -Arguments @('/x', $msi.FullName, '/qn', '/norestart', '/l*v', $freshUninstallLog) `
                -Label 'candidate MSI fresh uninstall'
        }
        catch {
            Show-LogTail -Path $freshUninstallLog
            throw
        }
    }
}
Assert-NoMsiResidue -Label 'Candidate fresh uninstall'

$configDir = Join-Path $env:APPDATA 'paneflow'
$configPath = Join-Path $configDir 'paneflow.json'
New-Item -ItemType Directory -Force -Path $configDir | Out-Null
[ordered]@{ terminal = [ordered]@{ scrollback_lines = 4321 } } |
    ConvertTo-Json -Depth 4 |
    Set-Content -LiteralPath $configPath -Encoding utf8
$configHashBeforeUpgrade = (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash

try {
    try {
        Invoke-SmokeProcess `
            -FilePath $msiexec `
            -Arguments @('/i', $baselineMsi.FullName, '/qn', '/norestart', '/l*v', $baselineInstallLog) `
            -Label 'pinned v0.8.0 MSI install'
    } catch {
        Show-LogTail -Path $baselineInstallLog
        throw
    }
    $installed = $true
    $exe = Join-Path $installDir 'paneflow.exe'
    if (-not (Test-Path -LiteralPath $exe)) {
        throw "Pinned v0.8.0 paneflow.exe not found at $exe"
    }
    $baselineActualVersion = (& $exe --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $baselineActualVersion -ne 'paneflow 0.8.0') {
        throw "Pinned baseline returned '$baselineActualVersion', expected 'paneflow 0.8.0'"
    }

    try {
        Invoke-SmokeProcess `
            -FilePath $msiexec `
            -Arguments @('/i', $msi.FullName, '/qn', '/norestart', '/l*v', $upgradeLog) `
            -Label 'candidate MSI upgrade'
    } catch {
        Show-LogTail -Path $upgradeLog
        throw
    }
    $uninstallMsi = $msi.FullName

    if (-not (Test-Path -LiteralPath $exe)) {
        throw "Upgraded paneflow.exe not found at $exe"
    }
    $actualVersion = (& $exe --version).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "paneflow --version exited with $LASTEXITCODE"
    }
    $expectedVersion = "paneflow $version"
    if ($actualVersion -ne $expectedVersion) {
        throw "paneflow --version returned '$actualVersion', expected '$expectedVersion'"
    }

    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
        throw 'MSI upgrade removed the existing PaneFlow configuration'
    }
    $configHashAfterUpgrade = (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash
    $preservedConfig = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    if ($configHashAfterUpgrade -ne $configHashBeforeUpgrade -or
        $preservedConfig.terminal.scrollback_lines -ne 4321) {
        throw 'MSI upgrade did not preserve the existing user configuration'
    }

    foreach ($entry in $complianceSources.GetEnumerator()) {
        $installedPath = Join-Path $installDir $entry.Key
        if (-not (Test-Path -LiteralPath $installedPath -PathType Leaf)) {
            throw "Installed MSI compliance inventory is missing $($entry.Key)"
        }
        $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $entry.Value).Hash
        $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installedPath).Hash
        if ($installedHash -ne $sourceHash) {
            throw "Installed MSI compliance file differs from its release source: $($entry.Key)"
        }
    }
    $forbiddenDlls = @(Get-ChildItem -LiteralPath $installDir -Recurse -Filter '*ghostty*.dll' -File)
    if ($forbiddenDlls.Count -ne 0) {
        throw "Installed MSI contains forbidden Ghostty DLLs: $(($forbiddenDlls.FullName) -join ', ')"
    }

    $installedReportPath = Join-Path $SmokeDir 'installed-libghostty-provenance.json'
    Invoke-SmokeProcess `
        -FilePath $pwsh `
        -Arguments @(
            '-NoProfile',
            '-File', $verifier,
            '-Target', 'x86_64-pc-windows-msvc',
            '-Binary', $exe,
            '-PackageRoot', $installDir,
            '-RuntimeEvidence', $freshGhosttyEvidence,
            '-ReportPath', $installedReportPath,
            '-AllowReleaseAssets'
        ) `
        -Label 'installed libghostty verifier'

    $installedReportText = Get-Content -LiteralPath $installedReportPath -Raw
    if ($installedReportText -match '(?i)[a-z]:\\') {
        throw 'Installed libghostty provenance report contains a runner-local absolute path'
    }
    $installedReport = $installedReportText | ConvertFrom-Json
    $installedBinaryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLowerInvariant()
    if ($installedReport.linkage -ne 'static' -or
        $installedReport.runtime_smoke_verified -ne $true -or
        $installedReport.binary_sha256 -ne $installedBinaryHash) {
        throw 'Installed libghostty provenance does not match the statically linked PaneFlow binary'
    }
    if ($releaseReport.binary_sha256 -ne $installedBinaryHash) {
        throw 'Packaged Windows libghostty provenance does not match the installed PaneFlow binary'
    }
    foreach ($requiredFile in @('paneflow.exe') + @($complianceSources.Keys)) {
        if (@($installedReport.package_files) -notcontains $requiredFile) {
            throw "Installed libghostty provenance is missing package inventory entry $requiredFile"
        }
    }

    $relayExe = Join-Path $SmokeDir 'paneflow-msi-relay.exe'
    $relayMsi = Join-Path $SmokeDir $msi.Name
    $relayLog = Join-Path $SmokeDir 'relay.log'
    $relayMsiLog = Join-Path $SmokeDir 'relay-msiexec.log'
    $restartProbe = Join-Path $env:WINDIR 'System32\whoami.exe'
    $missingParentPid = '4294967295'

    Copy-Item -LiteralPath $exe -Destination $relayExe -Force
    Copy-Item -LiteralPath $msi.FullName -Destination $relayMsi -Force

    try {
        Invoke-SmokeProcess `
            -FilePath $relayExe `
            -Arguments @(
                '--msi-relay',
                '--parent-pid', $missingParentPid,
                '--msi', $relayMsi,
                '--msi-log', $relayMsiLog,
                '--restart', $restartProbe,
                '--relay-log', $relayLog
            ) `
            -Label 'paneflow MSI relay'
    } catch {
        Show-LogTail -Path $relayLog
        Show-LogTail -Path $relayMsiLog
        throw
    }

    if (Test-Path -LiteralPath $relayMsi) {
        throw "MSI relay did not remove staged MSI copy: $relayMsi"
    }

    $relayText = Get-Content -LiteralPath $relayLog -Raw
    foreach ($needle in @(
        'started pid=',
        "parent $missingParentPid already exited or cannot be opened",
        'running msiexec',
        'msiexec exited with 0',
        'relaunch requested'
    )) {
        if (-not $relayText.Contains($needle)) {
            throw "Relay log missing expected entry '$needle'"
        }
    }
} finally {
    if ($installed) {
        try {
            Invoke-SmokeProcess `
                -FilePath $msiexec `
                -Arguments @('/x', $uninstallMsi, '/qn', '/norestart', '/l*v', $uninstallLog) `
                -Label 'msiexec uninstall'
        } catch {
            Show-LogTail -Path $uninstallLog
            throw
        }
    }
}

Assert-NoMsiResidue -Label 'Candidate upgrade uninstall'
if (-not (Test-Path -LiteralPath $configPath -PathType Leaf) -or
    (Get-FileHash -LiteralPath $configPath -Algorithm SHA256).Hash -ne $configHashBeforeUpgrade) {
    throw 'MSI uninstall did not preserve the existing user configuration'
}
