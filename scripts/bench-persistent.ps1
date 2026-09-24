[CmdletBinding()]
param(
    [switch]$SetBaseline,
    [switch]$WithWorker,
    [switch]$WithDesktop,
    [switch]$NoFollowers,
    [switch]$Quick,
    [switch]$SeedFailure,
    [string]$Prior,
    [string]$WorkerReplacement,
    [int]$Endurance = 0,
    [int]$IdleMinutes = 0
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$sha = (git rev-parse --short=12 HEAD).Trim()
$dirty = if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) { "true" } else { "false" }
$stamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
New-Item -ItemType Directory -Force -Path "bench/results" | Out-Null
$root = (Get-Location).Path
$suite = if ($Endurance -gt 0) { "persistent-endurance" } else { "persistent" }
$test = if ($Endurance -gt 0) { "persistent_session_endurance" } else { "persistent_session_baseline" }
$out = Join-Path $root "bench/results/$suite-$stamp-$sha.json"
if ($Endurance -gt 0) {
    $env:PANEFLOW_BENCH_ENDURANCE_MINUTES = "$Endurance"
} else {
    Remove-Item Env:PANEFLOW_BENCH_ENDURANCE_MINUTES -ErrorAction SilentlyContinue
}
if ($IdleMinutes -gt 0) {
    $env:PANEFLOW_BENCH_IDLE_MINUTES = "$IdleMinutes"
} else {
    Remove-Item Env:PANEFLOW_BENCH_IDLE_MINUTES -ErrorAction SilentlyContinue
}

$env:PANEFLOW_BENCH_OUT = $out
$env:PANEFLOW_BENCH_SHA = $sha
$env:PANEFLOW_BENCH_DIRTY = $dirty
$env:PANEFLOW_BENCH_STAMP = $stamp
if (Test-Path "bench/persistent-baseline.json") {
    $env:PANEFLOW_BENCH_BASELINE = Join-Path $root "bench/persistent-baseline.json"
} else {
    Remove-Item Env:PANEFLOW_BENCH_BASELINE -ErrorAction SilentlyContinue
}

$harness = cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json |
    ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq "compiler-artifact" -and $_.target.name -eq "persistent_baseline" -and $_.executable } |
    Select-Object -Last 1 -ExpandProperty executable
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
cargo build --release --locked -p paneflow-app -p paneflow-host
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
if (-not $harness -or -not (Test-Path $harness)) {
    Write-Error "the persistent_baseline harness was not built: $harness"
    exit 2
}
$env:PANEFLOW_BENCH_HOST = Join-Path $root "target/release/paneflow-host.exe"
$env:PANEFLOW_BENCH_FIXTURE = Join-Path $root "target/release/paneflow-session-fixture.exe"
if ($WithWorker -or $WithDesktop -or $WorkerReplacement) {
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
if ($Quick) {
    $env:PANEFLOW_BENCH_QUICK = "1"
} else {
    Remove-Item Env:PANEFLOW_BENCH_QUICK -ErrorAction SilentlyContinue
}
if ($SeedFailure) {
    $env:PANEFLOW_BENCH_SEED_FAILURE = "1"
} else {
    Remove-Item Env:PANEFLOW_BENCH_SEED_FAILURE -ErrorAction SilentlyContinue
}
if ($Prior) {
    $env:PANEFLOW_BENCH_PRIOR_RESULT = $Prior
} else {
    Remove-Item Env:PANEFLOW_BENCH_PRIOR_RESULT -ErrorAction SilentlyContinue
}
if ($WorkerReplacement) {
    $env:PANEFLOW_BENCH_CONTROLLER_REPLACEMENT = $WorkerReplacement
} else {
    Remove-Item Env:PANEFLOW_BENCH_CONTROLLER_REPLACEMENT -ErrorAction SilentlyContinue
}

Push-Location (Join-Path $root "crates/paneflow-host")
& $harness $test --ignored --exact --nocapture --test-threads=1
$status = $LASTEXITCODE
Pop-Location

if (-not (Test-Path $out)) {
    Write-Error "benchmark produced no result file: $out"
    exit 1
}
Write-Host "result: $out"
if ($status -ne 0) {
    Write-Error "persistent-path thresholds failed (exit $status); the artifact above is retained and a rerun must pass -Prior $out"
    exit $status
}
if ($SetBaseline -and $Endurance -eq 0) {
    Copy-Item $out "bench/persistent-baseline.json" -Force
    Write-Host "baseline: bench/persistent-baseline.json now points at $sha"
}
