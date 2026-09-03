<#
.SYNOPSIS
    Sign a PaneFlow Windows artifact with Azure Artifact Signing (formerly Trusted Signing).

.DESCRIPTION
    US-015. Wraps `signtool.exe sign` with the Azure dlib + a generated
    metadata.json so CI can invoke it uniformly per artifact. Run on a
    windows-2022 GitHub-hosted runner for both the release executable before
    WiX packaging and the final MSI after `cargo wix` has produced it.

    The Azure dlib and signtool authenticate silently via DefaultAzureCredential
    narrowed to EnvironmentCredential (other credential types are excluded via
    ExcludeCredentials in metadata.json). This avoids the dlib hanging on
    managed-identity probes on runners that don't have one.

.PARAMETER InputFile
    Path to the .exe or .msi artifact to sign. Required.

.PARAMETER DlibPath
    Optional override for the path to Azure.CodeSigning.Dlib.dll. When omitted
    (the normal CI case) the script fetches the NuGet package
    Microsoft.ArtifactSigning.Client into a temp directory and resolves the
    x64 dll inside it.

.PARAMETER ExpectedDlibSha256
    Expected SHA-256 for Azure.CodeSigning.Dlib.dll. Defaults to the pinned
    Microsoft.ArtifactSigning.Client 1.0.128 x64 DLL hash. Required for
    intentional custom -DlibPath upgrades.

.PARAMETER TimestampRetryDelaySec
    Seconds to wait between timestamp-server retries. Default 5.

.EXAMPLE
    scripts\sign-windows.ps1 -InputFile .\target\wix\paneflow-0.1.0-x86_64.msi

.EXAMPLE
    scripts\sign-windows.ps1 -InputFile .\target\x86_64-pc-windows-msvc\release\paneflow.exe

.NOTES
    Required env vars (provisioned via GitHub Secrets per US-014):
      AZURE_TENANT_ID
      AZURE_CLIENT_ID
      AZURE_CLIENT_SECRET
      AZURE_TRUSTED_SIGNING_ENDPOINT    e.g. https://eus.codesigning.azure.net/
      AZURE_TRUSTED_SIGNING_ACCOUNT     Trusted Signing account name
      AZURE_TRUSTED_SIGNING_CERT_PROFILE e.g. PaneFlow-Release

    The memory file memory/project_windows_signing.md is the single source of
    truth for what each secret holds and how to rotate it.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$InputFile,

    [Parameter(Mandatory = $false)]
    [string]$DlibPath,

    [Parameter(Mandatory = $false)]
    [string]$ExpectedDlibSha256 = '2D4C1BBC87467B3AC25BBC49DF58CC8B36A0F92B3E21AA98BBBAD08A4D7C98BA',

    [Parameter(Mandatory = $false)]
    [int]$TimestampRetryDelaySec = 5
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $InputFile -PathType Leaf)) {
    Write-Error "InputFile not found: $InputFile"
    exit 1
}

$resolvedInput = (Resolve-Path -LiteralPath $InputFile).Path
$inputExtension = [System.IO.Path]::GetExtension($resolvedInput).ToLowerInvariant()

if ($inputExtension -notin @('.exe', '.msi')) {
    Write-Error "InputFile must be a .exe or .msi artifact: $resolvedInput"
    exit 1
}

$requiredVars = @(
    'AZURE_TENANT_ID',
    'AZURE_CLIENT_ID',
    'AZURE_CLIENT_SECRET',
    'AZURE_TRUSTED_SIGNING_ENDPOINT',
    'AZURE_TRUSTED_SIGNING_ACCOUNT',
    'AZURE_TRUSTED_SIGNING_CERT_PROFILE'
)

$missingVars = @()
foreach ($name in $requiredVars) {
    $value = [System.Environment]::GetEnvironmentVariable($name)
    if ([string]::IsNullOrEmpty($value)) {
        $missingVars += $name
    }
}

if ($missingVars.Count -gt 0) {
    $joined = $missingVars -join ', '
    Write-Error "Missing required env var(s): $joined. Populate them from GitHub Secrets before signing. See memory/project_windows_signing.md for what each one holds."
    exit 1
}

$tempRoot = $null

try {
    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("paneflow-sign-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

    $ExpectedDlibSha256 = $ExpectedDlibSha256.ToUpperInvariant()
    if ($ExpectedDlibSha256 -notmatch '^[0-9A-F]{64}$') {
        throw "ExpectedDlibSha256 must be a 64-character SHA-256 hex string."
    }

    if ([string]::IsNullOrEmpty($DlibPath)) {
        $nuget = Get-Command -Name nuget.exe -ErrorAction SilentlyContinue
        if ($null -eq $nuget) {
            $nuget = Get-Command -Name nuget -ErrorAction SilentlyContinue
        }
        if ($null -eq $nuget) {
            throw "nuget.exe not found on PATH and no -DlibPath was provided. Install NuGet CLI or pass -DlibPath explicitly."
        }

        $packagesDir = Join-Path $tempRoot 'nuget'
        New-Item -ItemType Directory -Path $packagesDir -Force | Out-Null

        $ArtifactSigningClientVersion = '1.0.128'

        Write-Host "Fetching Microsoft.ArtifactSigning.Client $ArtifactSigningClientVersion to $packagesDir"
        & $nuget install 'Microsoft.ArtifactSigning.Client' `
            -Version $ArtifactSigningClientVersion `
            -Source 'https://api.nuget.org/v3/index.json' `
            -OutputDirectory $packagesDir `
            -ExcludeVersion `
            -NonInteractive
        if ($LASTEXITCODE -ne 0) {
            throw "nuget install failed with exit code $LASTEXITCODE"
        }

        $dlibCandidate = Join-Path $packagesDir 'Microsoft.ArtifactSigning.Client\bin\x64\Azure.CodeSigning.Dlib.dll'
        if (-not (Test-Path -LiteralPath $dlibCandidate -PathType Leaf)) {
            $dlibCandidate = Get-ChildItem -Path $packagesDir -Recurse -Filter 'Azure.CodeSigning.Dlib.dll' |
                Where-Object { $_.FullName -match '\\x64\\' } |
                Select-Object -First 1 -ExpandProperty FullName
        }

        if ([string]::IsNullOrEmpty($dlibCandidate) -or -not (Test-Path -LiteralPath $dlibCandidate -PathType Leaf)) {
            throw "Could not locate Azure.CodeSigning.Dlib.dll after NuGet install. Check the package layout."
        }
        $dlibHash = (Get-FileHash -LiteralPath $dlibCandidate -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($dlibHash -ne $ExpectedDlibSha256) {
            throw "Azure.CodeSigning.Dlib.dll SHA-256 mismatch. Expected $ExpectedDlibSha256, got $dlibHash."
        }
        $DlibPath = $dlibCandidate
    } else {
        if (-not (Test-Path -LiteralPath $DlibPath -PathType Leaf)) {
            throw "DlibPath not found: $DlibPath"
        }
        $DlibPath = (Resolve-Path -LiteralPath $DlibPath).Path
        $dlibHash = (Get-FileHash -LiteralPath $DlibPath -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($dlibHash -ne $ExpectedDlibSha256) {
            throw "DlibPath SHA-256 mismatch. Expected $ExpectedDlibSha256, got $dlibHash."
        }
    }

    Write-Host "Using dlib: $DlibPath"

    $metadata = [ordered]@{
        Endpoint               = $env:AZURE_TRUSTED_SIGNING_ENDPOINT
        CodeSigningAccountName = $env:AZURE_TRUSTED_SIGNING_ACCOUNT
        CertificateProfileName = $env:AZURE_TRUSTED_SIGNING_CERT_PROFILE
        ExcludeCredentials     = @(
            'ManagedIdentityCredential',
            'WorkloadIdentityCredential',
            'SharedTokenCacheCredential',
            'VisualStudioCredential',
            'VisualStudioCodeCredential',
            'AzureCliCredential',
            'AzurePowerShellCredential',
            'AzureDeveloperCliCredential',
            'InteractiveBrowserCredential'
        )
    }

    $metadataPath = Join-Path $tempRoot 'metadata.json'
    $metadata | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $metadataPath -Encoding UTF8

    $sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $signtool = $null
    if (Test-Path -LiteralPath $sdkRoot) {
        $signtool = Get-ChildItem -Path $sdkRoot -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0.0.0' } } -Descending |
            Select-Object -First 1 -ExpandProperty FullName
    }

    if ([string]::IsNullOrEmpty($signtool)) {
        $onPath = Get-Command -Name signtool.exe -ErrorAction SilentlyContinue
        if ($null -ne $onPath) {
            $signtool = $onPath.Source
        }
    }

    if ([string]::IsNullOrEmpty($signtool)) {
        throw "signtool.exe not found. Install the Windows 10/11 SDK (>= 10.0.22621) or add signtool to PATH."
    }

    Write-Host "Using signtool: $signtool"

    $timestampServers = @(
        'http://timestamp.acs.microsoft.com',
        'http://timestamp.digicert.com',
        'http://timestamp.sectigo.com'
    )

    $signed = $false
    $lastExit = -1
    foreach ($tr in $timestampServers) {
        Write-Host "signtool sign (timestamp=$tr)"
        & $signtool sign /v `
            /fd SHA256 `
            /tr $tr `
            /td SHA256 `
            /dlib $DlibPath `
            /dmdf $metadataPath `
            "$resolvedInput"
        $lastExit = $LASTEXITCODE
        if ($lastExit -eq 0) {
            $signed = $true
            break
        }
        Write-Warning "signtool exited with code $lastExit against $tr; trying next timestamp server in ${TimestampRetryDelaySec}s"
        Start-Sleep -Seconds $TimestampRetryDelaySec
    }

    if (-not $signed) {
        throw "signtool sign failed against all timestamp servers. Last exit code: $lastExit."
    }

    Write-Host "signtool verify"
    & $signtool verify /pa /v "$resolvedInput"
    if ($LASTEXITCODE -ne 0) {
        throw "signtool verify failed with exit code $LASTEXITCODE"
    }

    $sig = Get-AuthenticodeSignature -LiteralPath $resolvedInput
    if ($sig.Status -ne 'Valid') {
        throw "Get-AuthenticodeSignature status is '$($sig.Status)' (expected 'Valid'). StatusMessage: $($sig.StatusMessage)"
    }

    $subject = $sig.SignerCertificate.Subject
    if ($subject -notmatch '(?i)(^|,\s*)O\s*=\s*Strivex\b') {
        throw "Signer subject O= does not match 'Strivex': $subject"
    }

    Write-Host "Signed + verified: $resolvedInput"
    Write-Host "  Status:  $($sig.Status)"
    Write-Host "  Subject: $subject"
}
finally {
    if ($null -ne $tempRoot -and (Test-Path -LiteralPath $tempRoot)) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
