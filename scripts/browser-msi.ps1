<#
.SYNOPSIS
    Prepare the Windows browser MSI payload, sign the PaneFlow components and build the installer.

.DESCRIPTION
    US-011 (prd-agents-browser-windows.md). Runs the reproducible chain that
    turns a fetched CEF runtime plus a locally built browser host into an MSI
    payload:

      1. restamp the shipped SBOM for the msi format, verify the staged prefix
         and regenerate its WiX component fragment,
      2. sign the two components PaneFlow itself produces
         (paneflow-browser-host.exe and paneflow-browser-host.dll) through
         scripts/sign-windows.ps1,
      3. re-verify so the plan digests match the signed bytes,
      4. invoke cargo wix with both WiX sources and -dBrowserStage so main.wxs
         picks the generated BrowserRuntime component group up. cargo-wix 0.3.9
         ignores the wxs key in package.metadata.wix and only auto-discovers
         src-app/wix/*.wxs, which does not exist, so main.wxs and the generated
         fragment are both passed with --include.

    Signing secrets come from the maintainer environment described in
    docs/release/windows-signing.md. They are never read from the repository.
    Without -Sign the script stops before step 4 and reports that the payload is
    not distributable, so an unsigned MSI is never produced by accident.

.PARAMETER Stage
    Staged prefix produced by `python scripts/browser-package.py stage`.

.PARAMETER Sign
    Sign the PaneFlow browser components and build the MSI. Requires the
    Azure Artifact Signing environment variables used by sign-windows.ps1.

.PARAMETER Output
    Directory cargo wix writes the MSI into. Defaults to target/wix.

.EXAMPLE
    pwsh scripts/browser-msi.ps1 -Stage target/browser-stage
#>

[CmdletBinding()]
param(
    [string] $Stage = "target/browser-stage",
    [switch] $Sign,
    [string] $Output = "target/wix"
)

$ErrorActionPreference = "Stop"
$repository = Split-Path -Parent $PSScriptRoot
$stagePath = [System.IO.Path]::GetFullPath((Join-Path $repository $Stage))
$outputPath = [System.IO.Path]::GetFullPath((Join-Path $repository $Output))
$target = "x86_64-pc-windows-msvc"

if (-not (Test-Path -LiteralPath $stagePath -PathType Container)) {
    throw "staged browser prefix does not exist: $stagePath. Run scripts/browser-package.py stage first."
}

$python = Get-Command python -ErrorAction Stop
function Invoke-BrowserPackage {
    param([string[]] $Arguments)
    & $python.Source (Join-Path $PSScriptRoot "browser-package.py") --target $target @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "browser-package.py $($Arguments -join ' ') failed"
    }
}

$sbomPath = Join-Path $stagePath "share/doc/paneflow/browser-sbom.json"
Invoke-BrowserPackage @("sbom", "--format", "msi", "--prefix", $stagePath, "--output", $sbomPath)
Invoke-BrowserPackage @("msi-plan", "--prefix", $stagePath)
Invoke-BrowserPackage @("verify", "--format", "msi", "--prefix", $stagePath)

$planPath = Join-Path $stagePath "browser-msi-plan.json"
$plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json
$signed = @($plan.signed_components)
if ($signed.Count -eq 0) {
    throw "the MSI plan lists no PaneFlow component to sign"
}

$requiredSecrets = @(
    "AZURE_TENANT_ID",
    "AZURE_CLIENT_ID",
    "AZURE_CLIENT_SECRET",
    "AZURE_TRUSTED_SIGNING_ENDPOINT",
    "AZURE_TRUSTED_SIGNING_ACCOUNT",
    "AZURE_TRUSTED_SIGNING_CERT_PROFILE"
)
$missing = @($requiredSecrets | Where-Object { -not [System.Environment]::GetEnvironmentVariable($_) })

if (-not $Sign) {
    $receipt = [ordered]@{
        schema_version = 1
        target = $target
        stage = $stagePath
        component_group = $plan.component_group
        components = $plan.files.Count
        signed_components = $signed
        signing_configuration = if ($missing.Count -eq 0) { "present" } else { "absent" }
        missing_signing_variables = $missing
        distributable = $false
        reason = "run with -Sign to sign the PaneFlow browser components and build the MSI"
    }
    Write-Output ($receipt | ConvertTo-Json -Depth 4)
    return
}

if ($missing.Count -gt 0) {
    throw "signing configuration is incomplete; missing $($missing -join ', '). See docs/release/windows-signing.md."
}

foreach ($component in $signed) {
    $componentPath = Join-Path $stagePath ($component -replace "/", "\")
    if (-not (Test-Path -LiteralPath $componentPath -PathType Leaf)) {
        throw "component to sign is absent from the stage: $componentPath"
    }
    & (Join-Path $PSScriptRoot "sign-windows.ps1") -InputFile $componentPath
    if ($LASTEXITCODE -ne 0) {
        throw "signing failed for $componentPath"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $componentPath
    if ($signature.Status -ne "Valid") {
        throw "Authenticode status for $componentPath is $($signature.Status)"
    }
}

Invoke-BrowserPackage @("msi-plan", "--prefix", $stagePath)
Invoke-BrowserPackage @("verify", "--format", "msi", "--prefix", $stagePath)

$cargo = Get-Command cargo -ErrorAction Stop
$mainSource = Join-Path $repository "packaging/wix/main.wxs"
$fragmentSource = Join-Path $stagePath "browser-components.wxs"
& $cargo.Source wix --package paneflow-app --target $target --nocapture `
    --include $mainSource --include $fragmentSource `
    --output $outputPath -C "-dBrowserStage=$stagePath"
if ($LASTEXITCODE -ne 0) {
    throw "cargo wix did not produce the browser MSI"
}

$msi = Get-ChildItem -LiteralPath $outputPath -Filter "*.msi" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $msi) {
    throw "no MSI was written to $outputPath"
}
& (Join-Path $PSScriptRoot "sign-windows.ps1") -InputFile $msi.FullName
if ($LASTEXITCODE -ne 0) {
    throw "signing failed for $($msi.FullName)"
}

$receipt = [ordered]@{
    schema_version = 1
    target = $target
    stage = $stagePath
    component_group = $plan.component_group
    components = $plan.files.Count
    signed_components = $signed
    signing_configuration = "present"
    msi = $msi.FullName
    msi_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $msi.FullName).Hash.ToLowerInvariant()
    msi_signature = (Get-AuthenticodeSignature -LiteralPath $msi.FullName).Status.ToString()
    distributable = $true
}
Write-Output ($receipt | ConvertTo-Json -Depth 4)
