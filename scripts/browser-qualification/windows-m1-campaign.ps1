[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $Output,
    [string[]] $Configurations = @('A', 'B', 'C'),
    [int] $Repetitions = 5,
    [int] $Port = 18762,
    [int] $CaptureSeconds = 60,
    [int] $WarmupSeconds = 10,
    [string] $Scenario = 'animation',
    [string] $Bundle = 'target/windows-browser-bundle-ep002',
    [int] $CooldownSeconds = 6
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$root = [IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $root) { throw 'Campaign output already exists' }
New-Item -ItemType Directory -Path $root | Out-Null
$fixtures = $null
$runs = @()

function Test-Fixture([int] $Port) {
    try {
        $client = [Net.Sockets.TcpClient]::new()
        $client.Connect('127.0.0.1', $Port)
        $client.Close()
        return $true
    } catch { return $false }
}

try {
    if (-not (Test-Fixture $Port)) {
        $fixtures = Start-Process -FilePath 'bun' -ArgumentList @('scripts/browser-qualification.mjs', 'serve', "$Port") -WorkingDirectory $repo -WindowStyle Hidden -PassThru
        for ($attempt = 0; $attempt -lt 60; $attempt++) {
            if (Test-Fixture $Port) { break }
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not (Test-Fixture $Port)) { throw "the fixture server did not listen on 127.0.0.1:$Port" }
    foreach ($configuration in $Configurations) {
        for ($repetition = 1; $repetition -le $Repetitions; $repetition++) {
            $directory = Join-Path $root ("{0}-r{1}" -f $configuration.ToLowerInvariant(), $repetition)
            $entry = [ordered]@{ configuration = $configuration; repetition = $repetition; directory = $directory; status = 'CAPTURE_FAILED'; error = $null }
            try {
                & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'windows-capture.ps1') -Configuration $configuration -Output $directory -Scenario $Scenario -Port $Port -CaptureSeconds $CaptureSeconds -WarmupSeconds $WarmupSeconds -Bundle $Bundle
                if ($LASTEXITCODE -ne 0) { throw "windows-capture.ps1 exited with $LASTEXITCODE" }
                $entry.status = 'CAPTURED'
                & bun (Join-Path $PSScriptRoot 'windows-analysis.mjs') $directory | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "windows-analysis.mjs exited with $LASTEXITCODE" }
                $entry.status = 'ANALYZED'
            } catch {
                $entry.error = $_.Exception.Message
            }
            $runs += [pscustomobject]$entry
            Write-Output ("{0} {1} r{2} {3}" -f $entry.status, $configuration, $repetition, $entry.error)
            Start-Sleep -Seconds $CooldownSeconds
        }
    }
    $analyzed = @($runs | Where-Object { $_.status -eq 'ANALYZED' } | ForEach-Object { $_.directory })
    $comparison = Join-Path $root 'm1-comparison.json'
    if ($analyzed.Count) {
        & bun (Join-Path $PSScriptRoot 'windows-m1-compare.mjs') $comparison @analyzed | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "windows-m1-compare.mjs exited with $LASTEXITCODE" }
    }
    $campaign = [ordered]@{
        schema_version = 1
        started_from = $repo
        warmup_seconds = $WarmupSeconds
        duration_seconds = $CaptureSeconds
        repetitions = $Repetitions
        scenario = $Scenario
        bundle = $Bundle
        runs = $runs
        comparison = if (Test-Path -LiteralPath $comparison) { 'm1-comparison.json' } else { $null }
    }
    [IO.File]::WriteAllText((Join-Path $root 'campaign.json'), ($campaign | ConvertTo-Json -Depth 8))
    Write-Output ("CAMPAIGN {0} analyzed={1} total={2}" -f $root, $analyzed.Count, $runs.Count)
} finally {
    if ($fixtures -is [Diagnostics.Process] -and -not $fixtures.HasExited) { Stop-Process -Id $fixtures.Id -Force }
}
