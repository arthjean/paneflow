[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('A','B','C')][string] $Configuration,
    [Parameter(Mandatory)][string] $Output,
    [ValidateSet('empty','scroll','animation','webgl','network','combined','ime')][string] $Scenario = 'animation',
    [int] $Port = 18762,
    [int] $CaptureSeconds = 60,
    [int] $WarmupSeconds = 10,
    [string] $Bundle = 'target/windows-browser-bundle-ep002',
    [switch] $KeepOpen
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$destination = [IO.Path]::GetFullPath($Output)
$bundlePath = if ([IO.Path]::IsPathRooted($Bundle)) { [IO.Path]::GetFullPath($Bundle) } else { [IO.Path]::GetFullPath((Join-Path $repo $Bundle)) }
if (Test-Path -LiteralPath $destination) { throw 'Evidence output already exists' }
New-Item -ItemType Directory -Path $destination | Out-Null
$binary = Join-Path $repo 'target/release/paneflow.exe'
$bundleRuntime = Join-Path $bundlePath 'lib/paneflow/browser'
$bundleHost = Join-Path $bundlePath 'lib/paneflow/paneflow-browser-host.exe'
if (Test-Path -LiteralPath $bundleRuntime -PathType Container) {
    $runtime = $bundleRuntime
    $hostBinary = $bundleHost
} elseif (Test-Path -LiteralPath (Join-Path $bundlePath 'browser/Release/libcef.dll') -PathType Leaf) {
    $runtime = Join-Path $bundlePath 'browser'
    $hostBinary = Join-Path $bundlePath 'browser/Release/paneflow-browser-host.exe'
} else {
    throw "Bundle does not contain a supported Windows runtime layout: $bundlePath"
}
$presentmon = Join-Path $repo 'target/PresentMon-2.5.1-x64.exe'
$pipe = '\\.\pipe\paneflow-m1-' + [Guid]::NewGuid().ToString('N')
$app = $null
$trace = $null
$traceSession = $null
$resources = $null

function Now-Ns {
    [long]([decimal][Diagnostics.Stopwatch]::GetTimestamp() * 1000000000 / [Diagnostics.Stopwatch]::Frequency)
}

function Stop-TraceSession([string] $Name) {
    & logman stop $Name -ets *> $null
}

function Remove-StaleTraceSessions {
    $running = @(& logman query -ets 2>$null | Select-String -Pattern '^Paneflow-M1-\S+' | ForEach-Object { $_.Matches[0].Value })
    foreach ($name in $running) { Stop-TraceSession $name }
    return $running
}

function Get-OsIdentity {
    $currentVersion = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $build = [int]$currentVersion.CurrentBuild
    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    @{
        caption = $operatingSystem.Caption
        version = $operatingSystem.Version
        current_build = $build
        update_build_revision = [int]$currentVersion.UBR
        display_version = $currentVersion.DisplayVersion
        release_id = $currentVersion.ReleaseId
        edition_id = $currentVersion.EditionID
        build_lab = $currentVersion.BuildLabEx
        registry_product_name = $currentVersion.ProductName
        registry_product_name_note = 'ProductName keeps the Windows 10 string on Windows 11; the build number and the CIM caption carry the identity'
        product_line = if ($build -ge 22000) { 'Windows 11' } else { 'Windows 10' }
        windows_11 = ($build -ge 22000)
        architecture = $operatingSystem.OSArchitecture
        kernel_product_version = (Get-Item C:\Windows\System32\ntoskrnl.exe).VersionInfo.ProductVersion
    }
}

function Get-MonitorScales {
    if (-not ('PaneflowMonitorScale' -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class PaneflowMonitorScale
{
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromPoint(POINT pt, uint flags);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint dpiX, out uint dpiY);

    public static uint ScalePercent(int x, int y)
    {
        POINT point = new POINT();
        point.X = x;
        point.Y = y;
        IntPtr monitor = MonitorFromPoint(point, 2);
        uint dpiX = 96;
        uint dpiY = 96;
        if (GetDpiForMonitor(monitor, 0, out dpiX, out dpiY) != 0) return 0;
        return (dpiX * 100 + 95) / 96;
    }
}
"@ -Language CSharp
    }
    Add-Type -AssemblyName System.Windows.Forms
    @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object {
        [pscustomobject]@{
            device_name = $_.DeviceName
            primary = $_.Primary
            scale_percent = [PaneflowMonitorScale]::ScalePercent($_.Bounds.Left + 8, $_.Bounds.Top + 8)
        }
    })
}

function Launch([string] $Executable, [string[]] $Arguments, [hashtable] $EnvironmentValues) {
    foreach ($entry in $EnvironmentValues.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
    }
    Start-Process -FilePath $Executable -ArgumentList $Arguments -WorkingDirectory $repo -PassThru
}

function Rpc([string] $Method, [object] $Parameters) {
    $json = ConvertTo-Json -InputObject $Parameters -Depth 12 -Compress
    $parameterFile = Join-Path $destination ("rpc-" + [Guid]::NewGuid().ToString('N') + '.json')
    [IO.File]::WriteAllText($parameterFile, $json)
    try {
        $result = (& bun (Join-Path $PSScriptRoot 'windows-rpc.mjs') $pipe $Method "@$parameterFile" 2>&1 | Out-String).Trim()
    } finally {
        Remove-Item -LiteralPath $parameterFile -Force -ErrorAction SilentlyContinue
    }
    if ($LASTEXITCODE -ne 0) { throw "IPC failed: $Method : $result" }
    $result | ConvertFrom-Json
}

try {
    foreach ($required in @($binary, $runtime, $hostBinary, $presentmon)) {
        if (-not (Test-Path -LiteralPath $required)) { throw "Required capture input is missing: $required" }
    }
    $runtimeHash = & python -c "import importlib.util,pathlib; s=importlib.util.spec_from_file_location('bp',r'$repo/scripts/browser-package.py'); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(m.runtime_digest(pathlib.Path(r'$runtime')))"
    if ($LASTEXITCODE -ne 0 -or $runtimeHash -notmatch '^[a-f0-9]{64}$') { throw 'Runtime digest failed' }
    $logPath = Join-Path $destination 'application.jsonl'
    $environmentValues = @{
        PANEFLOW_BROWSER_HOST = $hostBinary
        PANEFLOW_CEF_ROOT = $runtime
        PANEFLOW_BROWSER_QUALIFICATION_SHA256 = $runtimeHash
        PANEFLOW_BROWSER_STAGE_ROOT = (Join-Path $destination 'staging')
        PANEFLOW_M1_LOG = $logPath
        PANEFLOW_M1_STATE_ROOT = (Join-Path $destination 'state')
        PANEFLOW_SOCKET_PATH = $pipe
        PANEFLOW_IPC_SCRIPTING = '1'
    }
    $arguments = @()
    if ($Configuration -eq 'B') {
        $arguments = @('browser-prototype','--url',"http://127.0.0.1:$Port/$Scenario",'--width','1920','--height','1080','--hold',"$($WarmupSeconds + $CaptureSeconds + 15)",'--log',(Join-Path $destination 'prototype.jsonl'))
    }
    $app = @(Launch $binary $arguments $environmentValues)[-1]
    if ($null -eq $app -or $app.GetType().FullName -ne 'System.Diagnostics.Process') { throw 'Paneflow process launch returned no Process handle' }
    $surfaceIds = @()
    if ($Configuration -ne 'B') {
        $replayJson = & bun -e "import {replayBundle} from './scripts/browser-qualification/replay.mjs'; console.log(JSON.stringify(await replayBundle()));"
        if ($LASTEXITCODE -ne 0) { throw 'Replay generation failed' }
        [IO.File]::WriteAllText((Join-Path $destination 'replay.json'), $replayJson)
        $ready = $false
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            Start-Sleep -Milliseconds 500
            if ($app.HasExited) { throw 'Application exited during startup' }
            try { $null = Rpc 'workspace.list' @{}; $ready = $true; break } catch { }
        }
        if (-not $ready) { throw 'Application IPC did not become ready' }
        $panes = @(0..3 | ForEach-Object {
            $replay = (Join-Path $PSScriptRoot 'windows-replay.py').Replace('\','/')
            $replayDirectory = $destination.Replace('\','/')
            @{ name="M1-$_"; cwd=$repo; command="python $replay $_ $replayDirectory" }
        })
        $workspace = Rpc 'workspace.up' @{ name='Windows M1'; layout='tiled'; panes=$panes }
        $surfaceIds = @($workspace.surface_ids)
        if ($surfaceIds.Count -ne 4) { throw 'M1 requires four replay terminals' }
        $null = Rpc 'qualification.window' @{}
        if ($Configuration -eq 'C') { $null = Rpc 'qualification.browser.open' @{url="http://127.0.0.1:$Port/$Scenario"} }
    }
    $ready = $false
    for ($attempt = 0; $attempt -lt 90; $attempt++) {
        Start-Sleep -Milliseconds 500
        if ($app.HasExited) { throw "Application exited before presentation (exit code $($app.ExitCode))" }
        if (Test-Path -LiteralPath $logPath) {
            $content = Get-Content -LiteralPath $logPath -Tail 30
            $prototypeLog = Join-Path $destination 'prototype.jsonl'
            $prototypeContent = if ($Configuration -eq 'B' -and (Test-Path -LiteralPath $prototypeLog)) { Get-Content -LiteralPath $prototypeLog -Tail 30 } else { @() }
            if (($Configuration -eq 'A' -and ($content -match 'renderer_stage')) -or ($Configuration -eq 'B' -and ($prototypeContent -match '"event":"frame"')) -or ($Configuration -eq 'C' -and ($content -match 'browser_scene'))) { $ready=$true; break }
        }
    }
    if (-not $ready) { throw 'The requested presentation path was not reached' }
    if ($Configuration -ne 'B') {
        for ($attempt=0; $attempt -lt 60; $attempt++) {
            if (@(0..3 | Where-Object { Test-Path -LiteralPath (Join-Path $destination "terminal-$_.jsonl") }).Count -eq 4) { break }
            Start-Sleep -Milliseconds 500
        }
        if (@(0..3 | Where-Object { Test-Path -LiteralPath (Join-Path $destination "terminal-$_.jsonl") }).Count -ne 4) { throw 'Replay workers did not start' }
    }
    $origin = (Now-Ns) + 2000000000L
    $originTemporary = Join-Path $destination 'origin.pending.json'
    [IO.File]::WriteAllText($originTemporary, (@{origin_ns=$origin} | ConvertTo-Json -Compress))
    [IO.File]::Move($originTemporary, (Join-Path $destination 'origin.json'))
    $presentmonCsv = Join-Path $destination 'presentmon.csv'
    $presentmonStdout = Join-Path $destination 'presentmon.stdout.log'
    $presentmonStderr = Join-Path $destination 'presentmon.stderr.log'
    $traceSession = "Paneflow-M1-$($app.Id)"
    $reclaimed = Remove-StaleTraceSessions
    $presentmonArguments = @('--process_id', "$($app.Id)", '--output_file', $presentmonCsv, '--qpc_time', '--timed', "$($WarmupSeconds + $CaptureSeconds + 2)", '--terminate_after_timed', '--no_console_stats', '--no_track_gpu', '--no_track_input', '--session_name', $traceSession, '--stop_existing_session')
    $trace = Start-Process -FilePath $presentmon -ArgumentList $presentmonArguments -WorkingDirectory $repo -WindowStyle Hidden -RedirectStandardOutput $presentmonStdout -RedirectStandardError $presentmonStderr -PassThru
    if ($surfaceIds.Count -eq 4) {
        $replay = $replayJson | ConvertFrom-Json
        for ($terminal = 0; $terminal -lt 4; $terminal++) {
            $events = @($replay.events | Where-Object { $_.terminal -eq $terminal -and $_.input } | ForEach-Object {
                @{input=$_.input; planned_ns=($origin + [long]$_.at_ns)}
            })
            if (-not $events.Count) { throw "The replay bundle carries no input for terminal $terminal" }
            $null = Rpc 'qualification.input' @{surface_id=$surfaceIds[$terminal]; events=$events}
        }
    }
    $resources = [IO.StreamWriter]::new((Join-Path $destination 'resources.jsonl'), $false)
    while ((Now-Ns) -lt $origin + ($WarmupSeconds + $CaptureSeconds) * 1000000000L) {
        if ($app.HasExited) { throw "Application exited during capture (exit code $($app.ExitCode))" }
        $processes = @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,ExecutablePath)
        $ids = [Collections.Generic.HashSet[uint32]]::new()
        $null = $ids.Add([uint32]$app.Id)
        do {
            $added = $false
            foreach ($process in $processes) {
                if ($ids.Contains([uint32]$process.ParentProcessId)) { $added = $ids.Add([uint32]$process.ProcessId) -or $added }
            }
        } while ($added)
        $memory = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $ids.Contains([uint32]$_.IDProcess) } | Select-Object IDProcess,WorkingSetPrivate,HandleCount,PercentProcessorTime)
        $gpu = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory | Where-Object { $_.Name -match '^pid_(\d+)_' -and $ids.Contains([uint32]$Matches[1]) } | Select-Object Name,DedicatedUsage,SharedUsage,TotalCommitted)
        $resources.WriteLine((@{at_ns=(Now-Ns); processes=@($processes | Where-Object { $ids.Contains([uint32]$_.ProcessId) }); memory=$memory; gpu=$gpu} | ConvertTo-Json -Depth 8 -Compress))
        $resources.Flush()
        Start-Sleep -Milliseconds 1000
    }
    if (-not $trace.WaitForExit(10000)) {
        throw "step=presentmon_wait exit_code=running proof=$presentmonCsv stdout=$presentmonStdout stderr=$presentmonStderr"
    }
    if ($trace.ExitCode -ne 0) {
        throw "step=presentmon_exit exit_code=$($trace.ExitCode) proof=$presentmonCsv stdout=$presentmonStdout stderr=$presentmonStderr"
    }
    if (-not (Test-Path -LiteralPath $presentmonCsv -PathType Leaf) -or (Get-Item -LiteralPath $presentmonCsv).Length -eq 0) {
        throw "PresentMon produced no CSV; stdout=$presentmonStdout stderr=$presentmonStderr"
    }
    Add-Type -AssemblyName System.Windows.Forms
    $displayWmi = @(Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID | ForEach-Object {
        [pscustomobject]@{
            instance_name=$_.InstanceName
            manufacturer=[Text.Encoding]::ASCII.GetString([byte[]]($_.ManufacturerName | Where-Object { $_ -ne 0 }))
            product=[Text.Encoding]::ASCII.GetString([byte[]]($_.ProductCodeID | Where-Object { $_ -ne 0 }))
            serial=[Text.Encoding]::ASCII.GetString([byte[]]($_.SerialNumberID | Where-Object { $_ -ne 0 }))
        }
    })
    $displayBounds = @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object {
        [pscustomobject]@{
            device_name=$_.DeviceName
            primary=$_.Primary
            left=$_.Bounds.Left
            top=$_.Bounds.Top
            right=$_.Bounds.Right
            bottom=$_.Bounds.Bottom
        }
    })
    $metadata = @{
        schema_version=1; configuration=$Configuration; scenario=$Scenario; pid=$app.Id
        origin_ns=$origin; end_ns=(Now-Ns); qpc_frequency=[Diagnostics.Stopwatch]::Frequency
        warmup_seconds=$WarmupSeconds; duration_seconds=$CaptureSeconds; terminal_count=$surfaceIds.Count
        binary_sha256=(Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
        host_sha256=(Get-FileHash -LiteralPath $hostBinary -Algorithm SHA256).Hash.ToLowerInvariant()
        client_sha256=(Get-FileHash -LiteralPath ([IO.Path]::ChangeExtension($hostBinary,'dll')) -Algorithm SHA256).Hash.ToLowerInvariant()
        manifest_sha256=(Get-FileHash -LiteralPath (Join-Path $repo 'native/browser/manifest.toml') -Algorithm SHA256).Hash.ToLowerInvariant()
        gpui_manifest_sha256=(Get-FileHash -LiteralPath (Join-Path $repo 'native/gpui/manifest.toml') -Algorithm SHA256).Hash.ToLowerInvariant()
        runtime_sha256=$runtimeHash; paneflow_commit=(& git rev-parse HEAD); worktree_dirty=$true
        os=[Environment]::OSVersion.Version.ToString()
        os_identity=(Get-OsIdentity)
        trace_session=@{name=$traceSession; reclaimed_stale_sessions=$reclaimed}
        gpu=@(Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion,CurrentHorizontalResolution,CurrentVerticalResolution,CurrentRefreshRate)
        displays=@{wmi=$displayWmi; bounds=$displayBounds; scales=(Get-MonitorScales)}
        cpu=@(Get-CimInstance Win32_Processor | Select-Object Name)
        ram_bytes=(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory
        qualification='CAPTURED_NOT_YET_VALIDATED'
        fixture_sha256=(& bun -e "import {fixtureBundle} from './scripts/browser-qualification/fixtures.mjs'; console.log((await fixtureBundle()).manifest.sha256)")
        replay_sha256=if ($replayJson) { ($replayJson | ConvertFrom-Json).sha256 } else { $null }
        power_source=if (@(Get-CimInstance Win32_Battery).Count -eq 0) { 'desktop-no-battery' } else { @(Get-CimInstance Win32_Battery | Select-Object BatteryStatus) }
        source_files=@((& git ls-files --cached --others --exclude-standard) | Where-Object { $_ -notlike 'tasks/*' -and (Test-Path -LiteralPath (Join-Path $repo $_) -PathType Leaf) } | ForEach-Object {
            @{ path=$_; sha256=(Get-FileHash -LiteralPath (Join-Path $repo $_) -Algorithm SHA256).Hash.ToLowerInvariant() }
        })
    }
    [IO.File]::WriteAllText((Join-Path $destination 'capture.json'), ($metadata | ConvertTo-Json -Depth 8))
    Write-Output "CAPTURED $Configuration $Scenario $destination"
} finally {
    if ($resources) { $resources.Dispose() }
    if ($trace -is [Diagnostics.Process] -and -not $trace.HasExited) {
        $trace.Kill()
        $null = $trace.WaitForExit(5000)
    }
    if ($traceSession) { Stop-TraceSession $traceSession }
    if ($app -is [Diagnostics.Process] -and -not $app.HasExited -and -not $KeepOpen) {
        if ($Configuration -eq 'B') { $null = $app.WaitForExit(20000) }
        if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }
    }
}
