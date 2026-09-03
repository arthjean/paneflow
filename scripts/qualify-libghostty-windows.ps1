[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$OutputDir,
    [switch]$SkipBuild,
    [switch]$FinalizeOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "unsupported libghostty Windows qualification target: $Target"
}
if ($FinalizeOnly -and -not $SkipBuild) {
    throw "-FinalizeOnly requires -SkipBuild and existing same-commit evidence"
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $Root "target\libghostty-windows-qualification"
}
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$Commit = (& git -C $Root rev-parse HEAD).Trim()
$WorktreeDirty = @(& git -C $Root status --porcelain).Count -ne 0
if ($Commit -notmatch '^[0-9a-f]{40}$') {
    throw "qualification requires a full Git commit SHA"
}
if ($WorktreeDirty) {
    throw "qualification requires a clean worktree so evidence remains commit-scoped"
}
$EvidenceDir = Join-Path $OutputDir $Commit
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null

$GateAttempts = [ordered]@{}
$HostCreationP95BudgetUs = 500000
$HostCreationP95RetryCeilingUs = 525000
$FrameP95BudgetUs = 16700
$FrameP95RetryCeilingUs = 17535

function Test-PerformanceRetryEligible {
    param([Parameter(Mandatory = $true)][string]$LogPath)

    $hostMeasurement = $null
    $frameMeasurement = $null
    foreach ($line in Get-Content -LiteralPath $LogPath) {
        $measurement = Convert-MeasurementLine $line
        if ($null -eq $measurement) {
            continue
        }
        if (($measurement.PSObject.Properties.Name -contains 'scenario') -and
            $measurement.scenario -eq 'windows_ghostty_host_creation') {
            $hostMeasurement = $measurement
        }
        if ($measurement.PSObject.Properties.Name -contains 'input_to_frame_p95_us') {
            $frameMeasurement = $measurement
        }
    }
    if ($null -eq $hostMeasurement -or $null -eq $frameMeasurement) {
        return $false
    }

    $hostP95Us = [double]$hostMeasurement.p95_us
    $frameP95Us = [double]$frameMeasurement.input_to_frame_p95_us
    $budgetFailed = $hostP95Us -ge $HostCreationP95BudgetUs -or
        $frameP95Us -gt $FrameP95BudgetUs
    $withinRetryBand = $hostP95Us -le $HostCreationP95RetryCeilingUs -and
        $frameP95Us -le $FrameP95RetryCeilingUs
    return $budgetFailed -and $withinRetryBand
}

function Test-TeardownRetryEligible {
    param([Parameter(Mandatory = $true)][string]$LogPath)

    if (-not (Test-Path -LiteralPath $LogPath -PathType Leaf)) {
        return $false
    }
    $content = Get-Content -LiteralPath $LogPath -Raw
    if ([string]::IsNullOrEmpty($content)) {
        return $false
    }
    return $content -match 'phase=cleanup'
}

function Convert-MeasurementLine {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Line)

    $starts = @(
        @(
            $Line.IndexOf('{"target_os"'),
            $Line.IndexOf('{"scenario"'),
            $Line.IndexOf('{"seed"')
        ) |
            Where-Object { $_ -ge 0 } |
            Sort-Object
    )
    if ($starts.Count -eq 0) {
        return $null
    }
    $end = $Line.LastIndexOf('}')
    if ($end -le $starts[0]) {
        throw "malformed JSON measurement line"
    }
    $payload = $Line.Substring($starts[0], $end - $starts[0] + 1)
    try {
        return $payload | ConvertFrom-Json
    }
    catch {
        throw "malformed JSON measurement payload"
    }
}

function Invoke-CargoGate {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [switch]$RunnerVarianceRetry,
        [switch]$TeardownVarianceRetry
    )

    $PSNativeCommandUseErrorActionPreference = $false

    $maximumAttempts = if ($RunnerVarianceRetry -or $TeardownVarianceRetry) { 2 } else { 1 }
    $attemptsRun = 0
    for ($attempt = 1; $attempt -le $maximumAttempts; $attempt++) {
        $attemptsRun = $attempt
        $logPath = Join-Path $EvidenceDir "$Name-attempt-$attempt.log"
        Write-Host "gate=$Name attempt=$attempt/$maximumAttempts"
        & cargo @Arguments 2>&1 | Tee-Object -FilePath $logPath
        $exitCode = $LASTEXITCODE
        if ($exitCode -eq 0) {
            $GateAttempts[$Name] = $attempt
            return
        }
        if ($attempt -lt $maximumAttempts) {
            if ($TeardownVarianceRetry) {
                if (-not (Test-TeardownRetryEligible -LogPath $logPath)) {
                    break
                }
                Write-Warning "$Name failed only on the ConPTY descendant teardown deadline; performing the single bounded rerun"
            }
            else {
                if (-not (Test-PerformanceRetryEligible -LogPath $logPath)) {
                    break
                }
                Write-Warning "$Name stayed inside the documented 5 percent runner-variance band; performing the single bounded rerun"
            }
        }
    }
    throw "$Name failed after $attemptsRun bounded attempt(s)"
}

function Copy-ReleaseBinary {
    param([Parameter(Mandatory = $true)][string]$Destination)

    $source = Join-Path $Root "target\$Target\release\paneflow.exe"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "release build did not produce $source"
    }
    Copy-Item -LiteralPath $source -Destination $Destination -Force
}

function Import-SuccessfulGateEvidence {
    foreach ($name in @('performance', 'stress-cycles', 'stress-panes')) {
        $successful = @(Get-ChildItem -LiteralPath $EvidenceDir -Filter "$name-attempt-*.log" -File |
            Sort-Object Name |
            Where-Object { (Get-Content -LiteralPath $_.FullName -Raw).Contains('test result: ok.') } |
            Select-Object -Last 1)
        if ($successful.Count -ne 1) {
            throw "-FinalizeOnly could not find successful same-commit evidence for $name"
        }
        $attemptMatch = [regex]::Match($successful[0].Name, '-attempt-([0-9]+)\.log$')
        if (-not $attemptMatch.Success) {
            throw "-FinalizeOnly found malformed evidence name for $name"
        }
        $GateAttempts[$name] = [int]$attemptMatch.Groups[1].Value
    }
}

Push-Location $Root
try {
    & (Join-Path $PSScriptRoot "verify-libghostty-windows.ps1")

    $CandidateBinary = Join-Path $EvidenceDir "paneflow-ghostty.exe"

    if (-not $SkipBuild) {
        Invoke-CargoGate "build-ghostty" @(
            "build", "--release", "--locked",
            "-p", "paneflow-app", "--target", $Target
        )
        Copy-ReleaseBinary $CandidateBinary
    }
    elseif (-not (Test-Path -LiteralPath $CandidateBinary -PathType Leaf)) {
        throw "-SkipBuild requires an existing same-commit candidate binary in $EvidenceDir"
    }

    if ($FinalizeOnly) {
        Import-SuccessfulGateEvidence
    }
    else {
        Invoke-CargoGate "performance" @(
            "test", "--release", "--locked", "-p", "paneflow-app",
            "--target", $Target, "performance_gate",
            "--", "--ignored", "--nocapture", "--test-threads=1"
        ) -RunnerVarianceRetry

        Invoke-CargoGate "stress-cycles" @(
            "test", "--release", "--locked", "-p", "paneflow-app",
            "--target", $Target, "ghostty_spawn_resize_close_stress_has_no_residual_growth",
            "--", "--ignored", "--nocapture", "--test-threads=1"
        ) -TeardownVarianceRetry

        Invoke-CargoGate "stress-panes" @(
            "test", "--release", "--locked", "-p", "paneflow-app",
            "--target", $Target, "windows_ghostty_32_pane_resize_and_close_orders_are_bounded",
            "--", "--ignored", "--nocapture", "--test-threads=1"
        ) -TeardownVarianceRetry
    }

    $CandidateSize = (Get-Item -LiteralPath $CandidateBinary).Length

    $ProvenancePath = Join-Path $EvidenceDir "provenance.json"
    & (Join-Path $PSScriptRoot "verify-libghostty-windows.ps1") `
        -Binary $CandidateBinary `
        -ReportPath $ProvenancePath

    $Measurements = @()
    foreach ($log in Get-ChildItem -LiteralPath $EvidenceDir -Filter "*.log" -File | Sort-Object Name) {
        foreach ($line in Get-Content -LiteralPath $log.FullName) {
            $measurement = Convert-MeasurementLine $line
            if ($null -eq $measurement) {
                continue
            }
            $measurement | Add-Member -NotePropertyName source_log -NotePropertyValue $log.Name
            $Measurements += $measurement
        }
    }

    $MeasurementCounts = [ordered]@{
        'host creation median and P95' = @($Measurements | Where-Object {
                $null -ne $_ -and $null -ne $_.PSObject.Properties['scenario'] -and
                $_.scenario -eq 'windows_ghostty_host_creation'
            }).Count
        'eight-pane GPUI frame P95' = @($Measurements | Where-Object {
                $null -ne $_ -and $null -ne $_.PSObject.Properties['input_to_frame_p95_us']
            }).Count
        '200-cycle resource budget' = @($Measurements | Where-Object {
                $null -ne $_ -and $null -ne $_.PSObject.Properties['scenario'] -and
                $_.scenario -eq 'ghostty_spawn_resize_close'
            }).Count
        '32-pane resource budget' = @($Measurements | Where-Object {
                $null -ne $_ -and $null -ne $_.PSObject.Properties['scenario'] -and
                $_.scenario -eq 'windows_ghostty_32_panes'
            }).Count
    }
    foreach ($entry in $MeasurementCounts.GetEnumerator()) {
        if ($entry.Value -eq 0) {
            throw "qualification passed without required measurement: $($entry.Key)"
        }
    }

    $Summary = [ordered]@{
        schema_version = 1
        commit = $Commit
        worktree_dirty = $WorktreeDirty
        target = $Target
        runner = if ([string]::IsNullOrWhiteSpace($env:RUNNER_NAME)) { "local" } else { $env:RUNNER_NAME }
        runner_image = if ([string]::IsNullOrWhiteSpace($env:ImageOS)) { "unknown" } else { $env:ImageOS }
        profile = "release"
        variance_tolerance = [ordered]@{
            host_creation_p95_retry_ceiling_us = $HostCreationP95RetryCeilingUs
            input_to_frame_p95_retry_ceiling_us = $FrameP95RetryCeilingUs
            policy = "one rerun only inside the 5 percent boundary band; regressions outside it fail immediately"
        }
        attempts = $GateAttempts
        binary = [ordered]@{
            ghostty_bytes = $CandidateSize
        }
        measurements = $Measurements
        status = "passed"
    }
    $SummaryPath = Join-Path $EvidenceDir "qualification.json"
    $Summary | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $SummaryPath -Encoding utf8
    Write-Host "libghostty Windows qualification passed; evidence=$SummaryPath"
}
finally {
    Pop-Location
}
