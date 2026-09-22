#Requires -Version 7.0
param(
    [string]$Profile = "release",
    [string]$Target = "",
    [string]$Out = ""
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not $Target) {
    $Target = (rustc -vV | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
}
$exe = if ($Target -like "*windows*") { ".exe" } else { "" }
$binDir = "target/$Profile"
if (Test-Path "target/$Target/$Profile") {
    $binDir = "target/$Target/$Profile"
}
$helperDir = "target/embed-build/$Target/release-min"

$sha = (git rev-parse HEAD).Trim()
$dirty = [bool](git status --porcelain --untracked-files=no)

$failures = New-Object System.Collections.Generic.List[string]
$artifacts = [ordered]@{}

function Add-Artifact {
    param([string]$Name, [string]$Path, [bool]$Required)
    if (-not (Test-Path -PathType Leaf $Path)) {
        if ($Required) {
            $failures.Add("missing required artifact $Name at $Path")
        }
        $artifacts[$Name] = [ordered]@{ path = $Path; present = $false; required = $Required }
        return
    }
    $item = Get-Item $Path
    $artifacts[$Name] = [ordered]@{
        path = $Path
        present = $true
        required = $Required
        bytes = $item.Length
        sha256 = (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant()
    }
}

Add-Artifact "desktop" "$binDir/paneflow$exe" $true
Add-Artifact "host" "$binDir/paneflow-host$exe" $true
Add-Artifact "fixture" "$binDir/paneflow-session-fixture$exe" $false
foreach ($helper in @("paneflow-shim", "paneflow-ai-hook", "paneflow-mcp")) {
    Add-Artifact "helper.$helper" "$helperDir/$helper$exe" $true
}
Add-Artifact "engine.manifest" "native/libghostty/manifest.toml" $true
$engineDir = "native/libghostty/prebuilt/$Target/lib"
if (Test-Path $engineDir) {
    foreach ($archive in Get-ChildItem -File $engineDir) {
        Add-Artifact "engine.$($archive.Name)" "$engineDir/$($archive.Name)" $true
    }
} else {
    $failures.Add("no libghostty archive for $Target under native/libghostty/prebuilt; run scripts/fetch-libghostty.ps1")
}
Add-Artifact "conpty.manifest" "native/conpty/manifest.json" $true
if ($Target -like "*windows*") {
    Add-Artifact "conpty.dll" "native/conpty/prebuilt/$Target/conpty.dll" $true
    Add-Artifact "conpty.openconsole" "native/conpty/prebuilt/$Target/OpenConsole.exe" $true
}

if (-not $Out) {
    New-Item -ItemType Directory -Force bench/results | Out-Null
    $short = (git rev-parse --short=12 HEAD).Trim()
    $Out = "bench/results/candidate-$short-$Target.json"
}

$document = [ordered]@{
    schema_version = 1
    candidate_sha = $sha
    dirty = $dirty
    target = $Target
    profile = $Profile
    recorded_at = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    preflight = $(if ($failures.Count -eq 0) { "pass" } else { "fail" })
    failures = @($failures)
    artifacts = $artifacts
}
$document | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $Out
Write-Host "candidate manifest: $Out"
if ($failures.Count -ne 0) {
    Write-Error ("preflight failed:`n  " + ($failures -join "`n  "))
    exit 1
}
