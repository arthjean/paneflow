<#
.SYNOPSIS
Runs the paneflow-editor-bench suite and compares it against its baseline.

.DESCRIPTION
Builds the paneflow test binary under the release profile, runs the
editor_pipeline_benchmark suite, writes
bench/results/editor-<stamp>-<sha>.json, and prints a Markdown table between
the PANEFLOW_BENCH_TABLE_BEGIN and PANEFLOW_BENCH_TABLE_END markers.

Environment variables: PANEFLOW_BENCH_OUT is the result file this script sets,
PANEFLOW_BENCH_BASELINE is the baseline the table compares against and is set
when bench/editor-baseline.json exists (without it the table drops its
comparison columns), PANEFLOW_BENCH_SHA, PANEFLOW_BENCH_DIRTY and
PANEFLOW_BENCH_STAMP are recorded in the result, PANEFLOW_BENCH_ALLOW_DEBUG
allows a debug-profile run that the suite otherwise refuses, and
PANEFLOW_BENCH_SKIP_SHAPE skips the platform shaping probe.

.PARAMETER SetBaseline
Copy the fresh result over bench/editor-baseline.json. Refused when the run
reports a cpu_share below 0.90, because a contended run inflates every timing
it would freeze.

.PARAMETER Help
Print this help and exit.

.EXAMPLE
scripts/bench-editor.ps1

.EXAMPLE
scripts/bench-editor.ps1 -SetBaseline
#>
[CmdletBinding()]
param(
    [switch]$SetBaseline,
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
$out = Join-Path $root "bench/results/editor-$stamp-$sha.json"

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp
if (Test-Path "bench/editor-baseline.json") {
    $env:PANEFLOW_BENCH_BASELINE = Join-Path $root "bench/editor-baseline.json"
} else {
    Remove-Item Env:PANEFLOW_BENCH_BASELINE -ErrorAction SilentlyContinue
}

cargo test --release --locked -p paneflow-app --bin paneflow `
    app::diff_dock::code::perf_bench::editor_pipeline_benchmark `
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
    $cpuShare = (Get-Content $out -Raw | ConvertFrom-Json).cpu_share
    if ($null -eq $cpuShare) {
        Write-Error "the result carries no cpu_share, refusing to record a baseline from it: $out"
        exit 1
    }
    if ($cpuShare -lt 0.9) {
        Write-Error "cpu_share $cpuShare is below 0.90: this run got less than 90% of a core, so its timings are inflated and every later comparison against them would read as a false improvement. Close the competing workload and run again."
        exit 1
    }
    Copy-Item $out "bench/editor-baseline.json" -Force
    Write-Host "baseline: bench/editor-baseline.json now points at $sha"
}
