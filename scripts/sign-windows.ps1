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
    [int]$TimestampRetryDelaySec = 5
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# --- Validate the input file ---------------------------------------------

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

# --- Validate required env vars (AC-5) -----------------------------------
# Collect all missing names first so the operator sees the full list in one
# CI run, rather than fixing them one at a time over several retries.

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

# --- Temp workspace -------------------------------------------------------
# One directory per invocation; cleaned up on exit regardless of success. We
# write metadata.json into this dir, and if we need to fetch the NuGet
# package we extract it here too. Using a fresh dir per run is belt-and-
# braces against cross-run contamination on a self-hosted runner.

$tempRoot = $null

try {
    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("paneflow-sign-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

    # --- Resolve dlib path ------------------------------------------------
    # Prefer a caller-supplied path (CI can cache the NuGet package across
    # runs to avoid re-downloading on every tag). Otherwise fetch
    # Microsoft.ArtifactSigning.Client from NuGet.org and find the x64 dll
    # inside. The package was renamed from Microsoft.Trusted.Signing.Client
    # in early 2026 -- the old name is deprecated.

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

        # Pin exact version so a malicious new release on nuget.org cannot be
        # silently adopted. Bump this intentionally when Microsoft ships a new
        # Artifact Signing Client and we've verified the delta. Last pinned:
        # 1.0.128 (2026-Q1 latest per NuGet.org).
        $ArtifactSigningClientVersion = '1.0.128'
        $ArtifactSigningDlibSha256 = '2D4C1BBC87467B3AC25BBC49DF58CC8B36A0F92B3E21AA98BBBAD08A4D7C98BA'

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
            # Fall back to a glob in case the package layout shifts across
            # minor versions. We target x64 explicitly -- signtool is x64 on
            # the windows-2022 runner and the architectures must match.
            $dlibCandidate = Get-ChildItem -Path $packagesDir -Recurse -Filter 'Azure.CodeSigning.Dlib.dll' |
                Where-Object { $_.FullName -match '\\x64\\' } |
                Select-Object -First 1 -ExpandProperty FullName
        }

        if ([string]::IsNullOrEmpty($dlibCandidate) -or -not (Test-Path -LiteralPath $dlibCandidate -PathType Leaf)) {
            throw "Could not locate Azure.CodeSigning.Dlib.dll after NuGet install. Check the package layout."
        }
        $dlibHash = (Get-FileHash -LiteralPath $dlibCandidate -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($dlibHash -ne $ArtifactSigningDlibSha256) {
            throw "Azure.CodeSigning.Dlib.dll SHA-256 mismatch. Expected $ArtifactSigningDlibSha256, got $dlibHash."
        }
        $DlibPath = $dlibCandidate
    } else {
        if (-not (Test-Path -LiteralPath $DlibPath -PathType Leaf)) {
            throw "DlibPath not found: $DlibPath"
        }
        $DlibPath = (Resolve-Path -LiteralPath $DlibPath).Path
    }

    Write-Host "Using dlib: $DlibPath"

    # --- Write metadata.json ---------------------------------------------
    # ExcludeCredentials narrows DefaultAzureCredential to EnvironmentCredential
    # only. Without this, on a runner without a managed identity the dlib can
    # hang for minutes probing IMDS and other credential sources before
    # falling back to EnvironmentCredential. Explicitly excluding them makes
    # the auth path deterministic and fast.

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

    # --- Locate signtool.exe ---------------------------------------------
    # The windows-2022 runner ships several Windows SDKs under Windows Kits.
    # The Azure dlib requires signtool >= 10.0.22621.755; we resolve the
    # highest installed version dynamically rather than pinning a path so
    # this script keeps working when the runner image bumps SDK versions.

    $sdkRoot = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $signtool = $null
    if (Test-Path -LiteralPath $sdkRoot) {
        $signtool = Get-ChildItem -Path $sdkRoot -Recurse -Filter 'signtool.exe' -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0.0.0' } } -Descending |
            Select-Object -First 1 -ExpandProperty FullName
    }

    if ([string]::IsNullOrEmpty($signtool)) {
        # Fallback: maybe the runner put signtool on PATH (e.g. via the
        # add-signtool-action or a custom image).
        $onPath = Get-Command -Name signtool.exe -ErrorAction SilentlyContinue
        if ($null -ne $onPath) {
            $signtool = $onPath.Source
        }
    }

    if ([string]::IsNullOrEmpty($signtool)) {
        throw "signtool.exe not found. Install the Windows 10/11 SDK (>= 10.0.22621) or add signtool to PATH."
    }

    Write-Host "Using signtool: $signtool"

    # --- Sign with timestamp retry (AC-1, AC-4) --------------------------
    # signtool accepts only one /tr per invocation, so fallback is a retry
    # loop -- not multiple /tr flags. Primary is Azure's own RFC 3161 server;
    # DigiCert and Sectigo are widely-used alternates. Rotating through them
    # survives short-lived Azure timestamp outages (documented in the
    # failure-mode playbook in memory/project_windows_signing.md).

    $timestampServers = @(
        'http://timestamp.acs.microsoft.com',
        'http://timestamp.digicert.com',
        'http://timestamp.sectigo.com'
    )

    $signed = $false
    # -1 so a never-entered loop (e.g. empty $timestampServers) reports as
    # "did not run" rather than masking as success.
    $lastExit = -1
    foreach ($tr in $timestampServers) {
        Write-Host "signtool sign (timestamp=$tr)"
        # Flag order matches Microsoft Learn's canonical invocation (/v right
        # after `sign`, then the required flag block, then the file). Matches
        # the AC-1 shape in tasks/prd-windows-port.md; /v is an additive
        # verbosity flag per the 2026 docs.
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

    # --- signtool verify (AC-3) ------------------------------------------
    # /pa selects the Default Authentication Verification Policy, which is
    # what Windows itself uses when Explorer or msiexec checks a signature.
    # /v gives us the full cert chain in the CI log -- useful for confirming
    # the Strivex chain made it through.

    Write-Host "signtool verify"
    & $signtool verify /pa /v "$resolvedInput"
    if ($LASTEXITCODE -ne 0) {
        throw "signtool verify failed with exit code $LASTEXITCODE"
    }

    # --- Get-AuthenticodeSignature check (AC-6) --------------------------
    # Complementary to signtool verify -- tests the same signature through
    # PowerShell's Authenticode API, which is the API Windows Defender and
    # AppLocker query. Catches the "signtool says OK but Defender rejects"
    # failure mode, which usually means a broken cert chain.

    $sig = Get-AuthenticodeSignature -LiteralPath $resolvedInput
    if ($sig.Status -ne 'Valid') {
        throw "Get-AuthenticodeSignature status is '$($sig.Status)' (expected 'Valid'). StatusMessage: $($sig.StatusMessage)"
    }

    # Anchor the publisher check to the O= (Organization) RDN rather than a
    # bare substring. A bare `-match 'Strivex'` would pass on a hostile
    # subject like `CN=Strivex-Evil-Clone`; anchoring to `O=Strivex` keeps
    # the check meaningful even if the runner's trust store is ever
    # compromised. Case-insensitive since DN casing is not normative.
    $subject = $sig.SignerCertificate.Subject
    if ($subject -notmatch '(?i)(^|,\s*)O\s*=\s*Strivex\b') {
        throw "Signer subject O= does not match 'Strivex': $subject"
    }

    Write-Host "Signed + verified: $resolvedInput"
    Write-Host "  Status:  $($sig.Status)"
    Write-Host "  Subject: $subject"
}
finally {
    # Best-effort cleanup. If the removal itself fails (e.g., a file held
    # open briefly by AV), don't mask the real error from the try block.
    # $tempRoot is $null if we failed before New-Item ran -- skip cleanup then
    # so we don't throw a second error inside the finally.
    if ($null -ne $tempRoot -and (Test-Path -LiteralPath $tempRoot)) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
