[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Runtime,
    [Parameter(Mandatory = $true)]
    [string] $Bootstrap,
    [string] $ClientDll,
    [string] $Subject = "CN=Paneflow Local Browser Staging",
    [int] $ValidDays = 30
)

$ErrorActionPreference = "Stop"

$repositoryPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../.."))
$runtimePath = [System.IO.Path]::GetFullPath($Runtime)
$bootstrapPath = [System.IO.Path]::GetFullPath($Bootstrap)
if (-not $ClientDll) {
    $ClientDll = [System.IO.Path]::ChangeExtension($bootstrapPath, ".dll")
}
$clientPath = [System.IO.Path]::GetFullPath($ClientDll)
$chromeElfPath = Join-Path $runtimePath "Release/chrome_elf.dll"

$signToolPath = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
if (-not $signToolPath) {
    $signToolPath = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "\\x64\\" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $signToolPath) {
    throw "signtool.exe is required for staging signatures"
}
if (-not (Get-Command New-SelfSignedCertificate -ErrorAction SilentlyContinue)) {
    throw "run windows-sign-staging.ps1 with Windows PowerShell: New-SelfSignedCertificate is unavailable"
}

foreach ($required in @($chromeElfPath, $bootstrapPath, $clientPath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "staging input is absent: $required"
    }
}

function Set-StagingSignature([System.Security.Cryptography.X509Certificates.X509Certificate2] $Certificate, [string] $Path) {
    if ((Get-AuthenticodeSignature -FilePath $Path).Status.ToString() -ne "NotSigned") {
        $previous = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $signToolPath remove /s $Path 2>&1 | Out-Null
        $ErrorActionPreference = $previous
    }
    & $signToolPath sign /fd SHA256 /sha1 $Certificate.Thumbprint $Path | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for ${Path} with exit code $LASTEXITCODE"
    }
    $status = (Get-AuthenticodeSignature -FilePath $Path).Status.ToString()
    if ($status -ne "Valid") {
        throw "signature is not trusted for ${Path}: $status"
    }
}

$trustedThumbprints = @(Get-ChildItem Cert:\CurrentUser\Root |
    Where-Object { $_.Subject -eq $Subject } |
    Select-Object -ExpandProperty Thumbprint)
$certificate = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object {
        $_.Subject -eq $Subject -and
        $_.HasPrivateKey -and
        $_.NotAfter -gt (Get-Date).AddHours(1) -and
        $trustedThumbprints -contains $_.Thumbprint
    } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1
$reused = $null -ne $certificate

if (-not $reused) {
    $certificate = New-SelfSignedCertificate -Type CodeSigningCert `
        -Subject $Subject `
        -CertStoreLocation Cert:\CurrentUser\My `
        -NotAfter (Get-Date).AddDays($ValidDays)
    $exportedPath = Join-Path ([System.IO.Path]::GetTempPath()) ("paneflow-staging-" + $certificate.Thumbprint + ".cer")
    try {
        Export-Certificate -Cert $certificate -FilePath $exportedPath -Type CERT | Out-Null
        Import-Certificate -FilePath $exportedPath -CertStoreLocation Cert:\CurrentUser\Root | Out-Null
    } finally {
        Remove-Item -LiteralPath $exportedPath -Force -ErrorAction SilentlyContinue
    }
}

Set-StagingSignature $certificate $chromeElfPath
Set-StagingSignature $certificate $bootstrapPath
Set-StagingSignature $certificate $clientPath

$digest = & python -c "import importlib.util,pathlib; s=importlib.util.spec_from_file_location('bp',r'$repositoryPath/scripts/browser-package.py'); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(m.runtime_digest(pathlib.Path(r'$runtimePath')))"
if ($LASTEXITCODE -ne 0 -or $digest -notmatch "^[a-f0-9]{64}$") {
    throw "runtime digest failed for $runtimePath"
}

Write-Output "certificate=$($certificate.Subject)"
Write-Output "thumbprint=$($certificate.Thumbprint)"
Write-Output "reused=$($reused.ToString().ToLowerInvariant())"
Write-Output "not_after=$($certificate.NotAfter.ToString('yyyy-MM-ddTHH:mm:ssK'))"
Write-Output "PANEFLOW_BROWSER_QUALIFICATION_SHA256=$digest"
