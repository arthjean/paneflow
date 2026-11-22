[CmdletBinding()]
param(
    [switch]$Check,
    [string]$Commit,
    [string]$ZigCommit,
    [string]$ZedDir = $env:ZED_DIR,
    [string]$ZedZigDir = $env:ZED_ZIG_DIR
)

$ErrorActionPreference = 'Stop'
if (-not $ZedDir -or -not (Test-Path -LiteralPath $ZedDir -PathType Container)) {
    throw 'Set ZED_DIR to the local Zed checkout.'
}
if (-not $ZedZigDir -or -not (Test-Path -LiteralPath $ZedZigDir -PathType Container)) {
    throw 'Set ZED_ZIG_DIR to the local zed-extensions/zig checkout.'
}
$queryRoot = Join-Path $PSScriptRoot '../src-app/src/diff/queries'
$manifestPath = Join-Path $queryRoot 'MANIFEST.toml'
if (-not $Commit) {
    $match = [regex]::Match([IO.File]::ReadAllText($manifestPath), '(?m)^commit = "([a-f0-9]{40})"$')
    if (-not $match.Success) { throw 'MANIFEST.toml is missing an immutable commit.' }
    $Commit = $match.Groups[1].Value
}
$resolved = & git -C $ZedDir rev-parse --verify "$Commit^{commit}"
if ($LASTEXITCODE -ne 0 -or $resolved -notmatch '^[a-f0-9]{40}$') { throw 'Cannot resolve the Zed commit.' }
$Commit = $resolved.Trim()
if (-not $ZigCommit) {
    $match = [regex]::Match([IO.File]::ReadAllText($manifestPath), '(?m)^name = "zig"\r?\n(?:(?!\[\[queries\]\]).*\r?\n)*?commit = "([a-f0-9]{40})"')
    if (-not $match.Success) { throw 'MANIFEST.toml has no Zig commit: pass -ZigCommit.' }
    $ZigCommit = $match.Groups[1].Value
}
$resolved = & git -C $ZedZigDir rev-parse --verify "$ZigCommit^{commit}"
if ($LASTEXITCODE -ne 0 -or $resolved -notmatch '^[a-f0-9]{40}$') { throw 'Cannot resolve the zed-extensions/zig commit.' }
$ZigCommit = $resolved.Trim()
function Read-GitBlob([string]$Repository, [string]$Revision, [string]$Path) {
    $info = [Diagnostics.ProcessStartInfo]::new('git')
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    foreach ($argument in @('-C', $Repository, 'show', "${Revision}:$Path")) { $info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    $stream = [IO.MemoryStream]::new()
    try {
        $process.StandardOutput.BaseStream.CopyTo($stream)
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "Cannot read $Path at $Revision" }
        return , $stream.ToArray()
    } finally {
        $stream.Dispose()
        $process.Dispose()
    }
}
function Get-Sha256([byte[]]$Bytes) {
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}
$languages = @('rust', 'json', 'jsonc', 'bash', 'python', 'typescript', 'tsx', 'javascript', 'markdown', 'markdown-inline', 'go', 'yaml', 'css', 'c', 'cpp')
$manifest = "repository = `"https://github.com/zed-industries/zed`"`ncommit = `"$Commit`"`nlicense = `"GPL-3.0-or-later`"`nlicense_evidence = `"crates/languages/Cargo.toml; LICENSE-GPL; crates/grammars/Cargo.toml has no license field`"`n"
$files = [ordered]@{}
foreach ($language in $languages) {
    $source = "crates/grammars/src/$language/highlights.scm"
    $bytes = Read-GitBlob $ZedDir $Commit $source
    $hash = Get-Sha256 $bytes
    $deviations = if ($language -eq 'javascript') { '["Uses the TSX grammar instead of tree-sitter-javascript"]' } else { '[]' }
    $manifest += "`n[[queries]]`nname = `"$language`"`nsource = `"$source`"`nsha256 = `"$hash`"`ndeviations = $deviations`n"
    $files[(Join-Path $queryRoot "$language/highlights.scm")] = $bytes
}
$zigSource = 'languages/zig/highlights.scm'
$zigBytes = Read-GitBlob $ZedZigDir $ZigCommit $zigSource
$manifest += "`n[[queries]]`nname = `"zig`"`nrepository = `"https://github.com/zed-extensions/zig`"`ncommit = `"$ZigCommit`"`nsource = `"$zigSource`"`nsha256 = `"$(Get-Sha256 $zigBytes)`"`nlicense = `"Apache-2.0`"`nlicense_evidence = `"LICENSE; Cargo.toml declares Apache-2.0`"`ndeviations = [`"Uses the tree-sitter-zig 1.1.2 crate parser instead of the grammar commit the extension pins`"]`n"
$files[(Join-Path $queryRoot 'zig/highlights.scm')] = $zigBytes
$files[(Join-Path $queryRoot 'zig/LICENSE')] = Read-GitBlob $ZedZigDir $ZigCommit 'LICENSE'
$files[$manifestPath] = [Text.UTF8Encoding]::new($false).GetBytes($manifest)
$notice = @"
Syntax highlighting queries from Zed, Copyright Zed Industries and contributors.
Source: https://github.com/zed-industries/zed
Revision: $Commit

Imported byte for byte from crates/grammars/src/<language>/highlights.scm.
The originating crates/languages package declares GPL-3.0-or-later at this
revision. crates/grammars/Cargo.toml has no license field; the repository
contains LICENSE-GPL and LICENSE-APACHE. These queries are distributed under
GPL-3.0-or-later with Paneflow; see the repository LICENSE.

Only highlighting queries are imported. Injections and font styles are outside
this integration. JavaScript uses Paneflow's existing TSX grammar.

The Zig highlighting query comes from the Zed Zig extension, Copyright Zed
Industries and contributors, licensed under Apache-2.0.
Source: https://github.com/zed-extensions/zig
Revision: $ZigCommit

Imported byte for byte from languages/zig/highlights.scm; zig/LICENSE holds
the extension's Apache-2.0 text. It applies to .zig and .zon files.
"@
$files[(Join-Path $queryRoot 'NOTICE')] = [Text.UTF8Encoding]::new($false).GetBytes($notice.Replace("`r`n", "`n") + "`n")
$diverged = [Collections.Generic.List[string]]::new()
foreach ($entry in $files.GetEnumerator()) {
    if ($Check) {
        if (-not (Test-Path -LiteralPath $entry.Key) -or
            [Convert]::ToBase64String([IO.File]::ReadAllBytes($entry.Key)) -cne [Convert]::ToBase64String($entry.Value)) {
            $diverged.Add($entry.Key)
        }
    } else {
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($entry.Key)) | Out-Null
        [IO.File]::WriteAllBytes($entry.Key, $entry.Value)
    }
}
if ($diverged.Count) { throw "Divergent Zed queries:`n$($diverged -join "`n")" }
Write-Output "15 Zed queries and 1 Zed extension query verified at $Commit and $ZigCommit"
