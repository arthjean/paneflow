[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$executables = @(
    'target\x86_64-pc-windows-msvc\release\paneflow.exe',
    'target\x86_64-pc-windows-msvc\release\paneflow-host.exe'
)
foreach ($exe in $executables) {
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "No Windows release executable found at $exe. The Build release binary step may have failed or emitted an unexpected path."
    }
    pwsh -NoProfile -File ./scripts/sign-windows.ps1 -InputFile $exe
    if ($LASTEXITCODE -ne 0) { throw "Signing failed for $exe (exit $LASTEXITCODE)" }
}
