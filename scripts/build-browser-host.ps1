[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Runtime,
    [string] $CefDevelopmentRoot,
    [Parameter(Mandatory = $true)]
    [string] $Bootstrap,
    [string] $ClientDll,
    [string] $Output = "target/windows-browser-bundle",
    [switch] $RequireSignature
)

$runtimePath = [System.IO.Path]::GetFullPath($Runtime)
$developmentPath = if ($CefDevelopmentRoot) {
    [System.IO.Path]::GetFullPath($CefDevelopmentRoot)
} else {
    $runtimePath
}
$bootstrapPath = [System.IO.Path]::GetFullPath($Bootstrap)
$outputPath = [System.IO.Path]::GetFullPath($Output)
$target = "x86_64-pc-windows-msvc"

if (-not (Test-Path -LiteralPath (Join-Path $runtimePath "verified-manifest.sha256") -PathType Leaf)) {
    throw "CEF runtime is not stamped; run scripts/fetch-browser.py explicitly"
}
if (-not (Test-Path -LiteralPath (Join-Path $runtimePath "Release/libcef.dll") -PathType Leaf)) {
    throw "CEF runtime has no Release/libcef.dll"
}
if (-not (Test-Path -LiteralPath (Join-Path $developmentPath "cef_version.json") -PathType Leaf)) {
    throw "CEF development package has no cef_version.json"
}
$importLibrary = @(
    (Join-Path $developmentPath "Release/libcef.lib"),
    (Join-Path $developmentPath "libcef.lib")
) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $importLibrary) {
    throw "CEF development package has no libcef.lib"
}
$runtimeMetadata = Get-Content -LiteralPath (Join-Path $runtimePath "cef_version.json") -Raw | ConvertFrom-Json
$developmentMetadata = Get-Content -LiteralPath (Join-Path $developmentPath "cef_version.json") -Raw | ConvertFrom-Json
if ($runtimeMetadata.version_full -ne $developmentMetadata.version_full -or $runtimeMetadata.abi_hash -ne $developmentMetadata.abi_hash) {
    throw "CEF runtime and development package metadata do not match"
}
if ([System.IO.Path]::GetExtension($bootstrapPath) -ne ".exe") {
    throw "the browser bootstrap must be an .exe"
}
if (-not (Test-Path -LiteralPath $bootstrapPath -PathType Leaf)) {
    throw "bootstrap executable does not exist: $bootstrapPath"
}
$bootstrapMagic = [System.IO.File]::ReadAllBytes($bootstrapPath)
if ($bootstrapMagic.Length -lt 2 -or $bootstrapMagic[0] -ne 0x4d -or $bootstrapMagic[1] -ne 0x5a) {
    throw "bootstrap executable is not a PE image"
}

$cargo = Get-Command cargo -ErrorAction Stop
$env:PANEFLOW_CEF_ROOT = $developmentPath
$env:CEF_PATH = $developmentPath
& $cargo.Source build --release --locked -p paneflow-browser-host --features cef-runtime --lib --target $target
if ($LASTEXITCODE -ne 0) {
    throw "the pinned Windows browser client DLL did not build"
}

if (-not $ClientDll) {
    $clientCandidates = @(
        (Join-Path $PSScriptRoot "../target/$target/release/paneflow_browser_host.dll"),
        (Join-Path $PSScriptRoot "../target/$target/release/paneflow-browser-host.dll")
    )
    $ClientDll = $clientCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $ClientDll) {
    throw "the Cargo build produced no client DLL"
}
$clientPath = [System.IO.Path]::GetFullPath($ClientDll)
if (-not (Test-Path -LiteralPath $clientPath -PathType Leaf)) {
    throw "client DLL does not exist: $clientPath"
}
$clientMagic = [System.IO.File]::ReadAllBytes($clientPath)
if ($clientMagic.Length -lt 2 -or $clientMagic[0] -ne 0x4d -or $clientMagic[1] -ne 0x5a) {
    throw "client DLL is not a PE image"
}

$dumpbin = Get-Command dumpbin -ErrorAction SilentlyContinue
if ($dumpbin) {
    $exports = & $dumpbin.Source /exports $clientPath 2>&1 | Out-String
    if ($exports -notmatch "RunWinMain") {
        throw "client DLL does not export RunWinMain"
    }
}

$signature = Get-AuthenticodeSignature -LiteralPath $bootstrapPath
if ($RequireSignature -and $signature.Status -ne "Valid") {
    throw "bootstrap Authenticode signature is $($signature.Status)"
}

if (Test-Path -LiteralPath $outputPath) {
    Remove-Item -LiteralPath $outputPath -Recurse -Force
}
$runtimeDestination = Join-Path $outputPath "lib/paneflow/browser"
$hostDestination = Join-Path $outputPath "lib/paneflow"
New-Item -ItemType Directory -Path $runtimeDestination -Force | Out-Null
Copy-Item -Path (Join-Path $runtimePath "*") -Destination $runtimeDestination -Recurse -Force
Copy-Item -LiteralPath $bootstrapPath -Destination (Join-Path $hostDestination "paneflow-browser-host.exe") -Force
Copy-Item -LiteralPath $clientPath -Destination (Join-Path $hostDestination "paneflow-browser-host.dll") -Force

$receipt = [ordered]@{
    schema_version = 1
    target = $target
    runtime = $runtimePath
    development_root = $developmentPath
    import_library = $importLibrary
    bootstrap = (Get-FileHash -Algorithm SHA256 -LiteralPath $bootstrapPath).Hash.ToLowerInvariant()
    client = (Get-FileHash -Algorithm SHA256 -LiteralPath $clientPath).Hash.ToLowerInvariant()
    bootstrap_signature = $signature.Status.ToString()
    sandbox_contract = "bootstrap.exe invokes RunWinMain(sandbox_info); browser_subprocess_path is not used"
}
$receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $outputPath "windows-bootstrap.json") -Encoding UTF8
Write-Output ($receipt | ConvertTo-Json)
