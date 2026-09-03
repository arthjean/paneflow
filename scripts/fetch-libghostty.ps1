[CmdletBinding()]
param(
    [string[]]$Target = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$ManifestPath = Join-Path $Root "native\libghostty\manifest.toml"

. "$PSScriptRoot/libghostty-manifest.ps1"

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

if ($Target.Count -eq 0) {
    $Target = @(
        Get-Content -LiteralPath $ManifestPath |
            Select-String -Pattern '^\[targets\."([^"]+)"\]$' |
            ForEach-Object { $_.Matches[0].Groups[1].Value }
    )
}

$Repository = Get-ManifestString "archive_release_repository"
$Tag = Get-ManifestString "archive_release_tag"

foreach ($triple in $Target) {
    $archivePath = (Get-ManifestTargetString $triple "archive_path").Replace('/', '\')
    $expected = Get-ManifestTargetString $triple "archive_sha256"
    $destination = Join-Path $Root "native\libghostty\prebuilt\$triple\$archivePath"
    if ((Test-Path -LiteralPath $destination -PathType Leaf) -and (Get-Sha256 $destination) -eq $expected) {
        Write-Host "${triple}: archive already in place ($expected)"
        continue
    }

    $asset = "$triple-$(Split-Path $archivePath -Leaf)"
    $url = "https://github.com/$Repository/releases/download/$Tag/$asset"
    $partial = "$destination.part"
    New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
    Remove-Item -LiteralPath $partial -Force -ErrorAction SilentlyContinue
    Write-Host "${triple}: downloading $url"
    $attempts = 3
    for ($attempt = 1; $attempt -le $attempts; $attempt++) {
        try {
            Invoke-WebRequest -Uri $url -OutFile $partial -MaximumRetryCount 0
            break
        }
        catch {
            if ($attempt -eq $attempts) {
                throw
            }
            Write-Warning "download attempt $attempt failed ($($_.Exception.Message)); retrying"
            Start-Sleep -Seconds (5 * $attempt)
        }
    }
    $actual = Get-Sha256 $partial
    if ($actual -ne $expected) {
        Remove-Item -LiteralPath $partial -Force
        throw "${triple}: downloaded archive hash $actual does not match manifest archive_sha256 $expected"
    }
    Move-Item -LiteralPath $partial -Destination $destination -Force
    Write-Host "${triple}: placed $destination ($expected)"
}
