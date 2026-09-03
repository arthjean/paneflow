<#
.SYNOPSIS
    Sign a PaneFlow Windows artifact with an OV (Organization Validation)
    code-signing certificate. **Local-only** fallback for the Azure Trusted
    Signing path implemented by sign-windows.ps1.

.DESCRIPTION
    US-024 AC-4. If Azure Trusted Signing becomes unavailable
    (subscription suspended, regional outage > 1 day, onboarding
    stalled > 6 weeks), this script lets a developer sign an MSI
    locally with a Sectigo or DigiCert OV cert held in a `.p12` file.
    The script intentionally does NOT run in CI - a `.p12` private key
    in GitHub Actions Secrets is a higher-risk pattern than the Azure
    service-principal flow (the `.p12` is a long-lived secret; the
    Azure path leaf-certs are 3-day-rotated via ACME).

    Uses the same 3-host timestamp retry chain as sign-windows.ps1
    (acs.microsoft.com → digicert → sectigo) so the resulting signature
    behaves identically with respect to time-based verification long
    after the OV cert itself expires.

.PARAMETER InputFile
    Path to the .exe or .msi artifact to sign. Required.

.PARAMETER CertPath
    Path to the OV cert in PKCS#12 format. Defaults to $env:OV_CERT_PATH.

.PARAMETER CertPassword
    Password for the .p12 file. Defaults to $env:OV_CERT_PASSWORD. Pass
    via env var rather than as a script argument to keep it out of the
    PowerShell history.

.PARAMETER TimestampRetryDelaySec
    Seconds to wait between timestamp-server retries. Default 5.

.EXAMPLE
    $env:OV_CERT_PATH = 'C:\paneflow-ov.p12'
    $env:OV_CERT_PASSWORD = '<from-password-manager>'
    scripts\sign-windows-ov.ps1 -InputFile .\target\wix\paneflow-0.3.0-x86_64.msi

.EXAMPLE
    $env:OV_CERT_PATH = 'C:\paneflow-ov.p12'
    $env:OV_CERT_PASSWORD = '<from-password-manager>'
    scripts\sign-windows-ov.ps1 -InputFile .\target\x86_64-pc-windows-msvc\release\paneflow.exe

.NOTES
    See docs/release/windows-signing.md §"OV-cert fallback" for the
    decision tree on when to use this script vs the Azure path.

    SECURITY: signtool receives the .p12 password via /p on its argv,
    which is visible to other local processes via Win32_Process / WMI
    while signtool runs. This matches Microsoft's own documented signtool
    invocation. Mitigations: only run on a trusted workstation, ensure no
    third-party AV / EDR with broad WMI subscription is profiling the
    user session, and prefer the Azure Trusted Signing path (no
    long-lived credential on argv) when available.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$InputFile,

    [Parameter(Mandatory = $false)]
    [string]$CertPath = $env:OV_CERT_PATH,

    [Parameter(Mandatory = $false)]
    [string]$CertPassword = $env:OV_CERT_PASSWORD,

    [Parameter(Mandatory = $false)]
    [int]$TimestampRetryDelaySec = 5
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $InputFile -PathType Leaf)) {
    throw "InputFile not found: $InputFile"
}

$resolvedInput = (Resolve-Path -LiteralPath $InputFile).Path
$inputExtension = [System.IO.Path]::GetExtension($resolvedInput).ToLowerInvariant()

if ($inputExtension -notin @('.exe', '.msi')) {
    throw "InputFile must be a .exe or .msi artifact: $resolvedInput"
}

if ([string]::IsNullOrEmpty($CertPath)) {
    throw "CertPath not provided and `$env:OV_CERT_PATH is empty. Pass -CertPath or set the env var."
}

if (-not (Test-Path -LiteralPath $CertPath -PathType Leaf)) {
    throw "OV cert not found: $CertPath"
}

if ([string]::IsNullOrEmpty($CertPassword)) {
    throw "CertPassword not provided and `$env:OV_CERT_PASSWORD is empty. Pass via env var to keep it out of the shell history."
}

$resolvedCert = (Resolve-Path -LiteralPath $CertPath).Path

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
        /f $resolvedCert `
        /p $CertPassword `
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

Write-Host "Signed + verified (OV path): $resolvedInput"
Write-Host "  Status:  $($sig.Status)"
Write-Host "  Subject: $subject"
