<#
.SYNOPSIS
Runs the paneflow-worker-bench suite.

.DESCRIPTION
Builds the worker_round_trip test under the gates profile and writes
bench/results/worker-<stamp>-<sha>.json: the median, 95th percentile and
maximum of 1,000 no-op requests to a worker over its local endpoint, and the
median and maximum time the endpoint stops answering across 20 worker
replacements triggered as an update triggers them. A run whose two halves
differ by more than 10 % is discarded and repeated; the result lists the
discarded attempts. No budget is gated.

.PARAMETER Help
Print this help and exit.

.EXAMPLE
scripts/bench-worker.ps1
#>
[CmdletBinding()]
param(
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    Get-Help $PSCommandPath -Detailed
    exit 0
}

Set-Location (Join-Path $PSScriptRoot "..")

$sha = (git rev-parse --short=12 HEAD).Trim()
$dirty = if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) { "true" } else { "false" }
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
New-Item -ItemType Directory -Force -Path "bench/results" | Out-Null
$root = (Get-Location).Path
$out = Join-Path $root "bench/results/worker-$stamp-$sha.json"

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp

cargo test --profile gates --locked -p paneflow-app --test worker_round_trip `
    worker_round_trip_benchmark -- --ignored --exact --nocapture --test-threads=1
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

if (-not (Test-Path $out)) {
    Write-Error "benchmark produced no result file: $out"
    exit 1
}
Write-Host "result: $out"
