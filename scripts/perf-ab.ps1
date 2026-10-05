[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)][string]$Base,
    [Parameter(Mandatory = $true, Position = 1)][string]$Head
)

$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $false
Set-Location (Join-Path $PSScriptRoot "..")
$root = (Get-Location).Path
$rounds = 10
$cargo = if ($env:CARGO) { $env:CARGO } else { "cargo" }
$pwsh = (Get-Process -Id $PID).Path

Get-ChildItem Env: | Where-Object { $_.Name -like "PANEFLOW_*" -and $_.Name -ne "PANEFLOW_PERF_AB_DIR" } |
    ForEach-Object { Remove-Item "Env:$($_.Name)" }

function Resolve-Commit([string]$Side, [string]$Ref) {
    $sha = git rev-parse --verify --quiet "$Ref^{commit}"
    if ($LASTEXITCODE -ne 0 -or -not $sha) {
        [Console]::Error.WriteLine("$Side $Ref is not a commit")
        exit 2
    }
    return $sha.Trim()
}

$baseSha = Resolve-Commit "base" $Base
$headSha = Resolve-Commit "head" $Head
if ((git status --porcelain --untracked-files=no | Measure-Object).Count -gt 0) {
    $checkoutSha = (git rev-parse HEAD).Trim()
    foreach ($side in @(@("base", $baseSha), @("head", $headSha))) {
        if ($side[1] -eq $checkoutSha) {
            [Console]::Error.WriteLine("the working tree has uncommitted changes and $($side[0]) resolves to its HEAD commit: the A/B measures commits, never a dirty tree; commit the changes first (git status --short)")
            exit 2
        }
    }
}

$out = if ($env:PANEFLOW_PERF_AB_DIR) { $env:PANEFLOW_PERF_AB_DIR } else { Join-Path $root "target/perf-ab" }
New-Item -ItemType Directory -Force -Path $out | Out-Null
Get-ChildItem -LiteralPath $out -Force |
    Where-Object { $_.Name -like "attempt-*" -or $_.Name -like "build-*.log" -or $_.Name -in @("result.json", "summary.md", "verdict", "compare.log", "instructions-verdict") } |
    Remove-Item -Recurse -Force
$scratch = Join-Path ([IO.Path]::GetTempPath()) "paneflow-perf-ab-$([Guid]::NewGuid().ToString('N').Substring(0, 8))"
New-Item -ItemType Directory -Path $scratch | Out-Null
$ownTarget = -not $env:CARGO_TARGET_DIR
if ($ownTarget) {
    $env:CARGO_TARGET_DIR = Join-Path $scratch "target"
}
$started = Get-Date
$script:ExitCode = $null

function Stop-Ab([int]$Code) {
    $script:ExitCode = $Code
    throw "the A/B stopped with exit code $Code"
}

function Stop-Unavailable([string]$Side, [string]$Reason, [string]$Log) {
    $sha = if ($Side -eq "head") { $headSha } else { $baseSha }
    [Console]::Error.WriteLine("$Side $($sha.Substring(0, 12)) unavailable: $Reason")
    if (Test-Path -LiteralPath $Log) {
        Select-String -LiteralPath $Log -Pattern '^error' -Context 0, 6 | Select-Object -First 10 | ForEach-Object {
            [Console]::Error.WriteLine($_.Line)
            $_.Context.PostContext | ForEach-Object { [Console]::Error.WriteLine($_) }
        }
    }
    [Console]::Error.WriteLine("log: $Log")
    Stop-Ab $(if ($Side -eq "head") { 4 } else { 3 })
}

function Invoke-Logged {
    param(
        [string]$Directory,
        [string]$Log,
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$Stdout,
        [hashtable]$Environment = @{}
    )
    $saved = @{}
    foreach ($name in $Environment.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $Environment[$name])
    }
    Push-Location -LiteralPath $Directory
    try {
        $ErrorActionPreference = "Continue"
        if ($Stdout) {
            & $FilePath @Arguments 1> $Stdout 2>> $Log
        } else {
            & $FilePath @Arguments *>> $Log
        }
        return $LASTEXITCODE
    } finally {
        Pop-Location
        foreach ($name in $saved.Keys) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name])
        }
    }
}

function Get-TestExecutable([string]$Jsonl, [string]$Target) {
    Get-Content -LiteralPath $Jsonl |
        Where-Object { $_.StartsWith("{") } |
        ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.reason -eq "compiler-artifact" -and $_.target.name -eq $Target -and $_.profile.test -and $_.executable } |
        Select-Object -Last 1 -ExpandProperty executable
}

function Initialize-Worktree([string]$Side, [string]$Sha) {
    $worktree = Join-Path $scratch $Side
    git worktree add --detach --quiet $worktree $Sha
    if ($LASTEXITCODE -ne 0) {
        throw "git worktree add failed for $Side"
    }
    foreach ($entry in @(git -C $root ls-files --others --ignored --exclude-standard --directory -- native)) {
        $source = Join-Path $root $entry
        $destination = Join-Path $worktree $entry
        if (Test-Path -LiteralPath $source -PathType Container) {
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            Copy-Item -Path (Join-Path $source "*") -Destination $destination -Recurse -Force
        } else {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
            Copy-Item -LiteralPath $source -Destination $destination -Force
        }
    }
}

function Build-Side([string]$Side, [string]$Sha) {
    $worktree = Join-Path $scratch $Side
    $bin = Join-Path $scratch "bin-$Side"
    $log = Join-Path $out "build-$Side.log"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    Write-Host "== building $Side $($Sha.Substring(0, 12))"
    $steps = @(
        @{ FilePath = $pwsh; Arguments = @("-NoLogo", "-NoProfile", "-File", "scripts/fetch-libghostty.ps1", "-Target", $hostTriple) },
        @{ FilePath = $cargo; Arguments = @("build", "--profile", "gates", "--locked", "-p", "paneflow-host") },
        @{ FilePath = $cargo; Arguments = @("test", "--profile", "gates", "--locked", "-p", "paneflow-app", "--bin", "paneflow", "--no-run", "--message-format=json-render-diagnostics"); Stdout = (Join-Path $bin "terminal.jsonl") }
    )
    if ($Side -eq "head") {
        $steps += @{ FilePath = $cargo; Arguments = @("test", "--profile", "gates", "--locked", "-p", "paneflow-host", "--test", "persistent_baseline", "--no-run", "--message-format=json-render-diagnostics"); Stdout = (Join-Path $bin "harness.jsonl") }
    }
    foreach ($step in $steps) {
        $status = Invoke-Logged -Directory $worktree -Log $log -FilePath $step.FilePath -Arguments $step.Arguments -Stdout $step.Stdout
        if ($status -ne 0) {
            Stop-Unavailable $Side "the build failed" $log
        }
    }
    $terminal = Get-TestExecutable (Join-Path $bin "terminal.jsonl") "paneflow"
    if (-not $terminal -or -not (Test-Path -LiteralPath $terminal)) {
        Stop-Unavailable $Side "the terminal suite executable was not built" $log
    }
    Copy-Item -LiteralPath $terminal -Destination (Join-Path $bin "terminal-suite.exe")
    Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR "gates/paneflow-host.exe") -Destination $bin
    if ($Side -eq "head") {
        Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR "gates/paneflow-session-fixture.exe") -Destination $bin
        $harness = Get-TestExecutable (Join-Path $bin "harness.jsonl") "persistent_baseline"
        if (-not $harness -or -not (Test-Path -LiteralPath $harness)) {
            Stop-Unavailable "head" "the persistent_baseline harness was not built" $log
        }
        Copy-Item -LiteralPath $harness -Destination (Join-Path $bin "harness.exe")
    }
}

function Invoke-Slot([string]$Directory, [int]$Round, [int]$Slot, [string]$Side) {
    $bin = Join-Path $scratch "bin-$Side"
    $headBin = Join-Path $scratch "bin-head"
    $tag = "r{0:D2}-{1}-{2}" -f $Round, $Slot, $Side
    $samples = Join-Path $Directory "$tag-terminal.json"
    $log = Join-Path $Directory "$tag-terminal.log"
    $status = Invoke-Logged -Directory (Join-Path $scratch "$Side/src-app") -Log $log `
        -FilePath (Join-Path $bin "terminal-suite.exe") `
        -Arguments @("terminal::perf_bench::terminal_pipeline_benchmark", "--ignored", "--exact", "--nocapture", "--test-threads=1") `
        -Environment @{ PANEFLOW_BENCH_SKIP_IDLE = "1"; PANEFLOW_BENCH_SAMPLES_OUT = $samples }
    if ($status -ne 0 -or -not (Test-Path -LiteralPath $samples)) {
        Stop-Unavailable $Side "the terminal suite exited $status without samples in round $Round; a commit older than the A/B sample export writes none" $log
    }
    $samples = Join-Path $Directory "$tag-active.json"
    $log = Join-Path $Directory "$tag-active.log"
    $status = Invoke-Logged -Directory (Join-Path $scratch "head/crates/paneflow-host") -Log $log `
        -FilePath (Join-Path $headBin "harness.exe") `
        -Arguments @("ab::perf_ab_active_samples", "--ignored", "--exact", "--nocapture", "--test-threads=1") `
        -Environment @{
            PANEFLOW_BENCH_HOST = (Join-Path $bin "paneflow-host.exe")
            PANEFLOW_BENCH_FIXTURE = (Join-Path $headBin "paneflow-session-fixture.exe")
            PANEFLOW_AB_SAMPLES_OUT = $samples
        }
    if ($status -ne 0 -or -not (Test-Path -LiteralPath $samples)) {
        Stop-Unavailable $Side "the active scenario exited $status without samples in round $Round" $log
    }
}

function Measure-Attempt([int]$Attempt) {
    $directory = Join-Path $out "attempt-$Attempt"
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
    foreach ($round in 1..$rounds) {
        Write-Host "== attempt $Attempt, round $round of $rounds"
        Invoke-Slot $directory $round 1 "base"
        Invoke-Slot $directory $round 2 "head"
        Invoke-Slot $directory $round 3 "head"
        Invoke-Slot $directory $round 4 "base"
    }
}

function Compare-Attempts {
    $status = Invoke-Logged -Directory (Join-Path $scratch "head/crates/paneflow-host") -Log (Join-Path $out "compare.log") `
        -FilePath (Join-Path $scratch "bin-head/harness.exe") `
        -Arguments @("ab::perf_ab_compare", "--ignored", "--exact", "--nocapture", "--test-threads=1") `
        -Environment @{
            PANEFLOW_AB_DIR = $out
            PANEFLOW_AB_BASE_REF = $Base
            PANEFLOW_AB_BASE_SHA = $baseSha
            PANEFLOW_AB_HEAD_REF = $Head
            PANEFLOW_AB_HEAD_SHA = $headSha
        }
    Get-Content -LiteralPath (Join-Path $out "compare.log") | Write-Host
    if ($status -ne 0) {
        throw "the comparator exited $status"
    }
}

try {
    $hostTriple = ((rustc -vV) | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
    Initialize-Worktree "base" $baseSha
    Build-Side "base" $baseSha
    Initialize-Worktree "head" $headSha
    Build-Side "head" $headSha
    Measure-Attempt 1
    Compare-Attempts
    $first = (Get-Content -LiteralPath (Join-Path $out "attempt-verdict") -Raw).Trim()
    if ($first -in @("regression", "uncalibrated")) {
        Write-Host "== the first execution was $first; measuring once more before any verdict"
        Measure-Attempt 2
        Compare-Attempts
    }
    Set-Content -LiteralPath (Join-Path $out "instructions-verdict") -Value "not_measured"
    Add-Content -LiteralPath (Join-Path $out "summary.md") -Value @(
        "",
        "## Instruction counts",
        "",
        "Verdict: **not_measured**",
        "",
        "Not measured: the Callgrind instruction counts need Valgrind, which runs on Linux only; scripts/perf-ab.sh measures them."
    )
    $verdict = (Get-Content -LiteralPath (Join-Path $out "verdict") -Raw).Trim()
    Write-Host "A/B finished in $([int]((Get-Date) - $started).TotalSeconds) s: $verdict; report $out/result.json, summary $out/summary.md"
    $script:ExitCode = switch ($verdict) {
        "pass" { 0 }
        "unconfirmed_regression" { 0 }
        "regression" { 1 }
        "uncalibrated" { 5 }
        default { 2 }
    }
} catch {
    if ($null -eq $script:ExitCode) {
        [Console]::Error.WriteLine("the A/B stopped on an unexpected error: $($_.Exception.Message)")
        $script:ExitCode = 2
    }
} finally {
    foreach ($side in @("base", "head")) {
        $worktree = Join-Path $scratch $side
        if (Test-Path -LiteralPath $worktree) {
            git worktree remove --force $worktree 2>&1 | Out-Null
        }
    }
    git worktree prune
    if ($ownTarget) {
        Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}
exit $script:ExitCode
