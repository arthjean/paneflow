[CmdletBinding()]
param(
    [switch]$SetBaseline
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$sha = (git rev-parse --short=12 HEAD).Trim()
$dirty = if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) { "true" } else { "false" }
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
New-Item -ItemType Directory -Force -Path "bench/results" | Out-Null
$root = (Get-Location).Path
$out = Join-Path $root "bench/results/$stamp-$sha.json"

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp
if (Test-Path "bench/baseline.json") {
    $env:PANEFLOW_BENCH_BASELINE = Join-Path $root "bench/baseline.json"
} else {
    Remove-Item Env:PANEFLOW_BENCH_BASELINE -ErrorAction SilentlyContinue
}

cargo test --release --locked -p paneflow-app --bin paneflow `
    terminal::perf_bench::terminal_pipeline_benchmark `
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
    Copy-Item $out "bench/baseline.json" -Force
    Write-Host "baseline: bench/baseline.json now points at $sha"
}
