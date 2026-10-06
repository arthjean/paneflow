[CmdletBinding()]
param(
    [switch]$SetBaseline
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$sha = (git rev-parse --short=12 HEAD).Trim()
$dirty = if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) { "true" } else { "false" }
if ($SetBaseline -and $dirty -eq "true") {
    Write-Error "the tracked worktree is dirty: commit before recording a baseline, so it records a commit that exists. Check with: git status --porcelain --untracked-files=no"
    exit 1
}
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
New-Item -ItemType Directory -Force -Path "bench/results" | Out-Null
$root = (Get-Location).Path
$out = Join-Path $root "bench/results/startup-$stamp-$sha.json"

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp
$env:PANEFLOW_BENCH_BASELINE_DIR = Join-Path $root "bench/baselines"

cargo build --release --locked -p paneflow-app
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

cargo test --release --locked -p paneflow-app --bin paneflow `
    startup_bench::startup_first_frame_benchmark `
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
    $platform = (Get-Content $out -Raw | ConvertFrom-Json).platform
    if (-not $platform) {
        Write-Error "the result names no platform, refusing to record a baseline from it: $out"
        exit 1
    }
    New-Item -ItemType Directory -Force -Path "bench/baselines/$platform" | Out-Null
    Copy-Item $out "bench/baselines/$platform/startup.json" -Force
    Write-Host "baseline: bench/baselines/$platform/startup.json now points at $sha"
}
