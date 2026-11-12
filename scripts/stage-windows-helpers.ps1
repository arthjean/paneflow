[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Target
)

$ErrorActionPreference = 'Stop'

$artifactDir = Join-Path 'target/embed-build' "$Target/release-min"
$embedDir = Join-Path 'src-app/target/embed/bin' $Target
$packagedHelperDir = Join-Path (Join-Path 'target' $Target) 'release/paneflow-helpers'
New-Item -ItemType Directory -Path $embedDir -Force | Out-Null
New-Item -ItemType Directory -Path $packagedHelperDir -Force | Out-Null

foreach ($name in @('paneflow-shim.exe', 'paneflow-ai-hook.exe', 'paneflow-mcp.exe')) {
  $artifact = Join-Path $artifactDir $name
  if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
    throw "Expected helper artifact missing: $artifact"
  }
  pwsh -NoProfile -File ./scripts/sign-windows.ps1 -InputFile $artifact
  if ($LASTEXITCODE -ne 0) { throw "Signing failed for $artifact (exit $LASTEXITCODE)" }
  Copy-Item -LiteralPath $artifact -Destination (Join-Path $embedDir $name) -Force
}

$shim = Join-Path $artifactDir 'paneflow-shim.exe'
foreach ($name in @(
  'claude.exe',
  'codex.exe',
  'opencode.exe',
  'pi.exe',
  'hermes.exe',
  'grok.exe',
  'amp.exe',
  'gemini.exe',
  'copilot.exe',
  'droid.exe',
  'openclaw.exe',
  'muse.exe'
)) {
  Copy-Item -LiteralPath $shim -Destination (Join-Path $packagedHelperDir $name) -Force
}
Copy-Item -LiteralPath (Join-Path $artifactDir 'paneflow-ai-hook.exe') `
  -Destination (Join-Path $packagedHelperDir 'paneflow-ai-hook.exe') `
  -Force

"helpers_prepared=true" | Out-File -FilePath $env:GITHUB_OUTPUT -Append -Encoding utf8
