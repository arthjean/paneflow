[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Tag,
    [Parameter(Mandatory = $true)][string] $Directory
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($env:GITHUB_REPOSITORY)) {
    throw 'GITHUB_REPOSITORY is required, as owner/name'
}

if ($Tag -eq 'latest') {
    $Tag = gh api "repos/$env:GITHUB_REPOSITORY/releases/latest" --jq .tag_name
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    Write-Host "Latest published release: $Tag"
}
if ($Tag -notmatch '^v(\d+\.\d+\.\d+)$') {
    throw "Release tag '$Tag' is not a vX.Y.Z tag"
}

$msiName = "paneflow-$($Matches[1])-x86_64-pc-windows-msvc.msi"
New-Item -ItemType Directory -Force -Path $Directory | Out-Null
gh release download $Tag `
  --repo $env:GITHUB_REPOSITORY `
  --pattern $msiName `
  --pattern "$msiName.sha256" `
  --dir $Directory
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "Downloaded $msiName of $Tag into $Directory"
