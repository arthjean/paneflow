#Requires -Version 7
$ErrorActionPreference = 'Stop'
$root = Join-Path (Split-Path -Parent $PSScriptRoot) 'native/conpty'
$manifest = Get-Content -LiteralPath (Join-Path $root 'manifest.json') -Raw | ConvertFrom-Json -AsHashtable
$missing = @()
foreach ($target in $manifest.targets.Keys) {
    foreach ($name in $manifest.targets[$target].Keys) {
        $entry = $manifest.targets[$target][$name]
        $destination = Join-Path $root "prebuilt/$target/$name"
        if (!(Test-Path -LiteralPath $destination) -or (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.sha256) {
            $missing += @{ Destination = $destination; Entry = $entry }
        }
    }
}
if ($missing.Count -eq 0) {
    Write-Host "ConPTY $($manifest.version): verified"
    exit 0
}
$scratch = Join-Path ([IO.Path]::GetTempPath()) "paneflow-conpty-$([Guid]::NewGuid())"
$null = New-Item -ItemType Directory -Path $scratch
try {
    $package = Join-Path $scratch 'conpty.nupkg'
    Invoke-WebRequest -Uri $manifest.url -OutFile $package
    if ((Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant() -ne $manifest.sha256) {
        throw 'ConPTY package SHA-256 mismatch'
    }
    $archive = [IO.Compression.ZipFile]::OpenRead($package)
    try {
        foreach ($item in $missing) {
            $entry = $archive.GetEntry($item.Entry.entry)
            if ($null -eq $entry) { throw "Missing package entry: $($item.Entry.entry)" }
            $temporary = Join-Path $scratch ([Guid]::NewGuid().ToString())
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $temporary)
            if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $item.Entry.sha256) {
                throw "ConPTY file SHA-256 mismatch: $($item.Entry.entry)"
            }
            $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $item.Destination)
            Move-Item -LiteralPath $temporary -Destination $item.Destination -Force
        }
    } finally {
        $archive.Dispose()
    }
    Write-Host "ConPTY $($manifest.version): fetched and verified"
} finally {
    Get-ChildItem -LiteralPath $scratch -File | Remove-Item -Force
    Remove-Item -LiteralPath $scratch
}
exit 0
