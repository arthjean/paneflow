[CmdletBinding()]
param(
    [switch]$SetBaseline,
    [switch]$WithWorker,
    [switch]$WithDesktop,
    [switch]$NoFollowers
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$sha = (git rev-parse --short=12 HEAD).Trim()
$dirty = if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) { "true" } else { "false" }
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
New-Item -ItemType Directory -Force -Path "bench/results" | Out-Null
$root = (Get-Location).Path
$out = Join-Path $root "bench/results/persistent-$stamp-$sha.json"

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp
if (Test-Path "bench/persistent-baseline.json") {
    $env:PANEFLOW_BENCH_BASELINE = Join-Path $root "bench/persistent-baseline.json"
} else {
    Remove-Item Env:PANEFLOW_BENCH_BASELINE -ErrorAction SilentlyContinue
}

cargo build --release --locked -p paneflow-host
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
if ($WithWorker -or $WithDesktop) {
    cargo build --release --locked -p paneflow-app
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    $env:PANEFLOW_BENCH_CONTROLLER = Join-Path $root "target/release/paneflow.exe"
} else {
    Remove-Item Env:PANEFLOW_BENCH_CONTROLLER -ErrorAction SilentlyContinue
}
if ($WithDesktop) {
    $env:PANEFLOW_BENCH_DESKTOP = "1"
} else {
    Remove-Item Env:PANEFLOW_BENCH_DESKTOP -ErrorAction SilentlyContinue
}
if ($NoFollowers) {
    $env:PANEFLOW_BENCH_NO_FOLLOWERS = "1"
} else {
    Remove-Item Env:PANEFLOW_BENCH_NO_FOLLOWERS -ErrorAction SilentlyContinue
}

cargo test --release --locked -p paneflow-host --test persistent_baseline `
    persistent_session_baseline `
    -- --ignored --exact --nocapture --test-threads=1
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

if (-not (Test-Path $out)) {
    Write-Error "benchmark produced no result file: $out"
    exit 1
}
Write-Host "result: $out"
if ($SetBaseline) {
    Copy-Item $out "bench/persistent-baseline.json" -Force
    Write-Host "baseline: bench/persistent-baseline.json now points at $sha"
}
