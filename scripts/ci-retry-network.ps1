#Requires -Version 7
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][scriptblock]$Command,
    [int]$Attempts = 3,
    [int]$BackoffSeconds = 20
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$signatureSource = Join-Path $PSScriptRoot "ci-retry-network.sh"
$signatures = [System.Collections.Generic.List[string]]::new()
$inList = $false
foreach ($line in Get-Content -LiteralPath $signatureSource) {
    if ($line -match '^TRANSIENT=\($') {
        $inList = $true
        continue
    }
    if ($inList -and $line -match '^\)$') {
        break
    }
    if ($inList -and $line -match "^\s+'(.+)'$") {
        $signatures.Add($Matches[1])
    }
}
if ($signatures.Count -eq 0) {
    throw "no transient signature found in $signatureSource"
}

for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
    Write-Host "::group::attempt $attempt/$Attempts"
    $captured = [System.Collections.Generic.List[string]]::new()
    $failure = $null
    $global:LASTEXITCODE = 0
    try {
        & $Command 2>&1 | ForEach-Object {
            $captured.Add("$_")
            Write-Host "$_"
        }
        if ($LASTEXITCODE -ne 0) {
            $failure = "exit code $LASTEXITCODE"
        }
    }
    catch {
        $failure = "$($_.Exception.Message)"
        Write-Host $failure
    }
    Write-Host "::endgroup::"

    if ($null -eq $failure) {
        if ($attempt -gt 1) {
            Write-Host "succeeded on attempt $attempt after a transient network failure"
        }
        exit 0
    }

    $text = ($captured -join "`n") + "`n" + $failure
    $transient = $false
    foreach ($signature in $signatures) {
        if ($text.Contains($signature)) {
            $transient = $true
            break
        }
    }
    if (-not $transient) {
        Write-Host "::error::command failed and shows no transient network signature; not retrying"
        exit 1
    }
    if ($attempt -eq $Attempts) {
        Write-Host "::error::still failing on a transient network error after $Attempts attempts"
        exit 1
    }
    $delay = $BackoffSeconds * $attempt
    Write-Host "::warning::transient network failure; retrying in ${delay}s"
    Start-Sleep -Seconds $delay
}
