[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $SmokeDir,
    [Parameter(Mandatory = $true)][string] $BaselineDir
)

$ErrorActionPreference = 'Stop'

New-Item -ItemType Directory -Force -Path $SmokeDir | Out-Null

function Get-SingleMsi {
    param(
        [Parameter(Mandatory = $true)][string] $Directory,
        [Parameter(Mandatory = $true)][string] $Label
    )

    $found = @(Get-ChildItem -LiteralPath $Directory -Filter 'paneflow-*-x86_64-pc-windows-msvc.msi' -File)
    if ($found.Count -ne 1) {
        throw "Expected exactly one $Label MSI in $Directory, found $($found.Count)"
    }
    return $found[0]
}

$candidate = Get-SingleMsi -Directory 'release-assets' -Label 'candidate'
$baseline = Get-SingleMsi -Directory $BaselineDir -Label 'latest release'
$baselineHash = ((Get-Content -LiteralPath "$($baseline.FullName).sha256" -Raw).Trim() -split '\s+')[0]
if ((Get-FileHash -LiteralPath $baseline.FullName -Algorithm SHA256).Hash -ne $baselineHash) {
    throw "Latest release MSI does not match its SHA-256 sidecar: $($baseline.Name)"
}

$installer = New-Object -ComObject WindowsInstaller.Installer

function Get-MsiProperty {
    param(
        [Parameter(Mandatory = $true)][string] $Path,
        [Parameter(Mandatory = $true)][string] $Name
    )

    $database = $installer.OpenDatabase($Path, 0)
    $view = $database.OpenView("SELECT Value FROM Property WHERE Property = '$Name'")
    [void] $view.Execute()
    $record = $view.Fetch()
    [void] $view.Close()
    [void] [System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
    [void] [System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    if ($null -eq $record) {
        throw "$Path has no $Name property"
    }
    return $record.StringData(1)
}

function Get-MajorUpgradeVersion {
    param([Parameter(Mandatory = $true)][string] $ProductVersion)

    $fields = $ProductVersion.Split('.')
    return [version]::new([int] $fields[0], [int] $fields[1], [int] $fields[2])
}

$upgradeCode = Get-MsiProperty -Path $candidate.FullName -Name 'UpgradeCode'
$candidateVersion = Get-MsiProperty -Path $candidate.FullName -Name 'ProductVersion'
$baselineVersion = Get-MsiProperty -Path $baseline.FullName -Name 'ProductVersion'
if ((Get-MajorUpgradeVersion $candidateVersion) -le (Get-MajorUpgradeVersion $baselineVersion)) {
    if ($env:GITHUB_EVENT_NAME -eq 'push') {
        Write-Host "::notice title=MSI upgrade smoke not applicable::Candidate MSI ProductVersion $candidateVersion does not upgrade the latest release $baselineVersion, so there is no upgrade over a live host to test for this tag."
        exit 0
    }
    throw "Candidate MSI ProductVersion $candidateVersion does not upgrade the latest release $baselineVersion; dispatch the dry-run with msi_upgrade_version above $baselineVersion"
}
Write-Host "Upgrading $baselineVersion to $candidateVersion (UpgradeCode $upgradeCode)"

function Show-LogTail {
    param([Parameter(Mandatory = $true)][string] $Path)

    if (Test-Path -LiteralPath $Path) {
        Write-Host "--- tail of $Path ---"
        Get-Content -LiteralPath $Path -Tail 160 | ForEach-Object { Write-Host $_ }
    }
}

function Invoke-SmokeProcess {
    param(
        [Parameter(Mandatory = $true)][string] $FilePath,
        [Parameter(Mandatory = $true)][string[]] $Arguments,
        [Parameter(Mandatory = $true)][string] $Label,
        [int] $TimeoutSeconds = 300
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.UseShellExecute = $false
    foreach ($argument in $Arguments) {
        [void] $startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::Start($startInfo)
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try {
            $process.Kill($true)
        } catch {
            Write-Warning "Could not kill timed-out process $($process.Id): $_"
        }
        throw "$Label timed out after $TimeoutSeconds seconds"
    }
    if ($process.ExitCode -ne 0) {
        throw "$Label exited with $($process.ExitCode)"
    }
}

$msiexec = Join-Path $env:WINDIR 'System32\msiexec.exe'
$installDir = Join-Path $env:ProgramFiles 'PaneFlow'
$installedExe = Join-Path $installDir 'paneflow.exe'
$installedHost = Join-Path $installDir 'paneflow-host.exe'
$filesInUseMarkers = @(
    'Note: 1: 1611',
    '`Error` = 1603',
    '`Error` = 1607',
    '`Error` = 1610',
    'is being held in use',
    'held files in use',
    'ReplacedInUseFiles'
)

function Invoke-Msiexec {
    param(
        [Parameter(Mandatory = $true)][string[]] $Arguments,
        [Parameter(Mandatory = $true)][string] $Log,
        [Parameter(Mandatory = $true)][string] $Label
    )

    try {
        Invoke-SmokeProcess -FilePath $msiexec -Arguments ($Arguments + @('/l*v', $Log)) -Label $Label
    } catch {
        Show-LogTail -Path $Log
        throw
    }
}

function Assert-InstalledVersion {
    param(
        [Parameter(Mandatory = $true)][string] $Expected,
        [Parameter(Mandatory = $true)][string] $Label
    )

    $installed = @(foreach ($productCode in @($installer.RelatedProducts($upgradeCode))) {
        $installer.ProductInfo($productCode, 'VersionString')
    })
    if ($installed.Count -ne 1 -or
        (Get-MajorUpgradeVersion $installed[0]) -ne (Get-MajorUpgradeVersion $Expected)) {
        throw "$Label expected installed ProductVersion $Expected, found '$($installed -join ', ')'"
    }
}

function Invoke-LiveHostUpgrade {
    param(
        [Parameter(Mandatory = $true)][string] $Name,
        [string[]] $Properties = @()
    )

    Invoke-Msiexec `
        -Arguments @('/i', $baseline.FullName, '/qn', '/norestart') `
        -Log (Join-Path $SmokeDir "$Name-baseline-install.log") `
        -Label "$Name install of $baselineVersion"
    Assert-InstalledVersion -Expected $baselineVersion -Label "$Name install of $baselineVersion"

    Invoke-SmokeProcess -FilePath $installedExe -Arguments @('host', 'start') -Label "$Name host start" -TimeoutSeconds 60
    $liveHosts = @(Get-Process -Name 'paneflow-host' -ErrorAction SilentlyContinue |
        Where-Object Path -eq $installedHost)
    if ($liveHosts.Count -eq 0) {
        throw "$Name host start left no process running $installedHost"
    }
    foreach ($liveHost in $liveHosts) {
        $null = $liveHost.Handle
    }
    Write-Host "$Name live host pid $($liveHosts.Id -join ', ') runs $installedHost"

    $upgradeLog = Join-Path $SmokeDir "$Name-upgrade.log"
    Invoke-Msiexec `
        -Arguments (@('/i', $candidate.FullName, '/qb', '/norestart') + $Properties) `
        -Log $upgradeLog `
        -Label "$Name upgrade to $candidateVersion"
    $inUse = @(Select-String -LiteralPath $upgradeLog -SimpleMatch -Pattern $filesInUseMarkers)
    if ($inUse.Count -ne 0) {
        $inUse | ForEach-Object { Write-Host $_.Line }
        throw "$Name upgrade log reports files in use"
    }
    Assert-InstalledVersion -Expected $candidateVersion -Label "$Name upgrade to $candidateVersion"
    $survivors = @($liveHosts | Where-Object { -not $_.HasExited })
    if ($survivors.Count -ne 0) {
        throw "$Name upgrade left the old host image running as pid $($survivors.Id -join ', ')"
    }

    Invoke-Msiexec `
        -Arguments @('/x', $candidate.FullName, '/qn', '/norestart') `
        -Log (Join-Path $SmokeDir "$Name-uninstall.log") `
        -Label "$Name uninstall of $candidateVersion"
}

Invoke-LiveHostUpgrade -Name 'relay-args' -Properties @('MSIRESTARTMANAGERCONTROL=Disable')
Invoke-LiveHostUpgrade -Name 'v0.16-args'
