<#
.SYNOPSIS
Measures the one Paneflow instance running on this machine for the real hardware protocol.

.DESCRIPTION
Samples the desktop, host and worker for 60 s and writes
bench/results/hardware-windows-<arch>-windows-<state>-<label>-<stamp>.json:
CPU and resident memory per process, their work counters (root_renders among
them), the desktop's 3D engine load from the "GPU Engine" performance counter,
and frame times from -FrameLog. A source that cannot be read is recorded
not_measured with its reason, never 0. See "Real hardware protocol" in
bench/README.md.

.PARAMETER State
idle-4-panes, agent-thinking, stream-4 or panes-8.

.PARAMETER Label
The build under test, for example v0.17.5 or main-0a74eb30.

.PARAMETER FrameLog
A PresentMon CSV captured over the same window.

.PARAMETER Window
Window length in seconds, 60 unless overridden.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("idle-4-panes", "agent-thinking", "stream-4", "panes-8")]
    [string]$State,
    [Parameter(Mandatory = $true)]
    [ValidatePattern("^[A-Za-z0-9._-]+$")]
    [string]$Label,
    [string]$FrameLog,
    [int]$Window = 0
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$paneflow = @(Get-CimInstance Win32_Process -Filter "Name = 'paneflow.exe'")
$workers = @($paneflow | Where-Object { $_.CommandLine -match " serve run" })
$desktops = @($paneflow | Where-Object { $_.CommandLine -notmatch " serve run" })
$hosts = @(Get-CimInstance Win32_Process -Filter "Name = 'paneflow-host.exe'")
if ($desktops.Count -ne 1) {
    Write-Error "expected exactly one running paneflow desktop, found $($desktops.Count): quit every other Paneflow instance and run this script from a terminal outside Paneflow"
    exit 1
}
if ($hosts.Count -gt 1 -or $workers.Count -gt 1) {
    Write-Error "more than one paneflow-host or worker is running: stop the other instances' hosts and workers first"
    exit 1
}

$exe = $desktops[0].ExecutablePath
$version = ""
if ($exe) {
    $version = (& $exe --version 2>$null | Select-Object -First 1)
}

$harness = cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json |
    ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq "compiler-artifact" -and $_.target.name -eq "persistent_baseline" -and $_.executable } |
    Select-Object -Last 1 -ExpandProperty executable
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
if (-not $harness -or -not (Test-Path $harness)) {
    Write-Error "the persistent_baseline harness was not built: $harness"
    exit 2
}

$env:PANEFLOW_BENCH_STAMP = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
$env:PANEFLOW_HW_STATE = $State
$env:PANEFLOW_HW_LABEL = $Label
$env:PANEFLOW_HW_VERSION = if ($version) { "$version" } else { "unknown ($exe)" }
$env:PANEFLOW_HW_DESKTOP_PID = "$($desktops[0].ProcessId)"
$env:PANEFLOW_HW_HOST_PID = if ($hosts.Count -eq 1) { "$($hosts[0].ProcessId)" } else { "" }
$env:PANEFLOW_HW_WORKER_PID = if ($workers.Count -eq 1) { "$($workers[0].ProcessId)" } else { "" }
if ($FrameLog) {
    $env:PANEFLOW_HW_FRAME_LOG = (Resolve-Path $FrameLog).Path
} else {
    Remove-Item Env:PANEFLOW_HW_FRAME_LOG -ErrorAction SilentlyContinue
}
if ($Window -gt 0) {
    $env:PANEFLOW_HW_WINDOW_S = "$Window"
} else {
    Remove-Item Env:PANEFLOW_HW_WINDOW_S -ErrorAction SilentlyContinue
}
Write-Host "measuring desktop $($env:PANEFLOW_HW_DESKTOP_PID), host $($env:PANEFLOW_HW_HOST_PID), worker $($env:PANEFLOW_HW_WORKER_PID) ($($env:PANEFLOW_HW_VERSION)) in state $State; leave the machine alone for the window"
& $harness hardware_protocol --ignored --exact --nocapture --test-threads=1
exit $LASTEXITCODE
