[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Runtime,
    [Parameter(Mandatory = $true)]
    [string] $Bootstrap,
    [Parameter(Mandatory = $true)]
    [string] $ClientDll,
    [string] $Output = "target/windows-browser-qualification",
    [switch] $SignWithTestCertificate,
    [switch] $KeepTestCertificate,
    [int] $TimeoutSeconds = 30
)

$runtimePath = [System.IO.Path]::GetFullPath($Runtime)
$bootstrapPath = [System.IO.Path]::GetFullPath($Bootstrap)
$clientPath = [System.IO.Path]::GetFullPath($ClientDll)
$outputPath = [System.IO.Path]::GetFullPath($Output)
$browserPath = Join-Path $outputPath "browser"
$releasePath = Join-Path $browserPath "Release"
$profilePath = Join-Path $outputPath "profile"
$stderrPath = Join-Path $outputPath "bootstrap.stderr"
$receiptPath = Join-Path $outputPath "windows-bootstrap-qualification.json"
$certificatePath = Join-Path $outputPath "qualification.cer"
$target = "x86_64-pc-windows-msvc"
$certificate = $null
$process = $null
$server = $null
$oldPath = $env:Path
$oldVariables = @{}
$signToolPath = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
if (-not $signToolPath) {
    $signToolPath = @(
        "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe",
        "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\signtool.exe"
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}

function Require-File([string] $Path, [string] $Message) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw $Message
    }
}

function Read-Frame([System.IO.Stream] $Stream) {
    $header = New-Object byte[] 4
    $offset = 0
    while ($offset -lt $header.Length) {
        $count = $Stream.Read($header, $offset, $header.Length - $offset)
        if ($count -le 0) {
            throw "native bootstrap closed the control pipe before the frame header was complete"
        }
        $offset += $count
    }
    $length = ([int]$header[0] * 16777216) + ([int]$header[1] * 65536) + ([int]$header[2] * 256) + [int]$header[3]
    if ($length -le 0 -or $length -gt 262144) {
        throw "native bootstrap returned an invalid frame length: $length"
    }
    $body = New-Object byte[] $length
    $offset = 0
    while ($offset -lt $body.Length) {
        $count = $Stream.Read($body, $offset, $body.Length - $offset)
        if ($count -le 0) {
            throw "native bootstrap closed the control pipe before the frame payload was complete"
        }
        $offset += $count
    }
    [System.Text.Encoding]::UTF8.GetString($body)
}

function Write-Frame([System.IO.Stream] $Stream, [object] $Value) {
    $body = [System.Text.Encoding]::UTF8.GetBytes(($Value | ConvertTo-Json -Compress -Depth 8))
    $length = $body.Length
    $header = [byte[]]@(
        [int]([Math]::Floor($length / 16777216)),
        [int]([Math]::Floor($length / 65536) % 256),
        [int]([Math]::Floor($length / 256) % 256),
        [int]($length % 256)
    )
    [void]$Stream.Write($header, 0, $header.Length)
    [void]$Stream.Write($body, 0, $body.Length)
    $Stream.Flush()
}

function Set-ProfileAcl([string] $Path) {
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $grant = "*{0}:(OI)(CI)(F)" -f $sid
    & icacls.exe $Path /inheritance:r /grant:r $grant '*S-1-5-18:(OI)(CI)(F)' '*S-1-5-32-544:(OI)(CI)(F)' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "profile ACL setup failed with exit code $LASTEXITCODE"
    }
}

function Set-TestSignature([System.Security.Cryptography.X509Certificates.X509Certificate2] $Certificate, [string] $Path) {
    if (-not $signToolPath) {
        throw "signtool.exe is required for native qualification signing"
    }
    & $signToolPath sign /fd SHA256 /sha1 $Certificate.Thumbprint $Path | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for ${Path} with exit code $LASTEXITCODE"
    }
    $signature = Get-AuthenticodeSignature -FilePath $Path
    if ($signature.Status.ToString() -ne "Valid") {
        throw "test signature failed for ${Path}: $($signature.Status.ToString())"
    }
}

function Remove-TestSignature([string] $Path) {
    if (-not $signToolPath) {
        throw "signtool.exe is required for native qualification signing"
    }
    & $signToolPath remove /s $Path | Out-Null
}

try {
    if ([Environment]::OSVersion.Version.Build -lt 17763) {
        throw "Windows build $([Environment]::OSVersion.Version.Build) is below Windows 10 1809"
    }
    Require-File (Join-Path $runtimePath "verified-manifest.sha256") "CEF runtime is not stamped"
    Require-File (Join-Path $runtimePath "Release/libcef.dll") "CEF runtime has no Release/libcef.dll"
    Require-File (Join-Path $runtimePath "Release/chrome_elf.dll") "CEF runtime has no Release/chrome_elf.dll"
    Require-File $bootstrapPath "CEF bootstrap executable is absent"
    Require-File $clientPath "CEF client DLL is absent"

    if (Test-Path -LiteralPath $outputPath) {
        $existingProfile = Join-Path $outputPath "profile"
        if (Test-Path -LiteralPath $existingProfile) {
            & icacls.exe $existingProfile /reset /T /C | Out-Null
        }
        Remove-Item -LiteralPath $outputPath -Recurse -Force
    }
    New-Item -ItemType Directory -Path $releasePath,$profilePath -Force | Out-Null
    Set-ProfileAcl $profilePath
    Copy-Item -Path (Join-Path $runtimePath "*") -Destination $browserPath -Recurse -Force
    Copy-Item -LiteralPath $bootstrapPath -Destination (Join-Path $releasePath "paneflow-browser-host.exe") -Force
    Copy-Item -LiteralPath $clientPath -Destination (Join-Path $releasePath "paneflow-browser-host.dll") -Force

    if ($SignWithTestCertificate) {
        if (-not (Get-Command New-SelfSignedCertificate -ErrorAction SilentlyContinue)) {
            throw "run windows-bootstrap.ps1 with Windows PowerShell when -SignWithTestCertificate is used"
        }
        $certificate = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Paneflow EP-001 Native Qualification" -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddDays(2)
        Export-Certificate -Cert $certificate -FilePath $certificatePath -Type CERT | Out-Null
        Import-Certificate -FilePath $certificatePath -CertStoreLocation Cert:\CurrentUser\Root | Out-Null
        Remove-TestSignature (Join-Path $releasePath "chrome_elf.dll")
        Set-TestSignature $certificate (Join-Path $releasePath "chrome_elf.dll")
        Set-TestSignature $certificate (Join-Path $releasePath "paneflow-browser-host.exe")
        Set-TestSignature $certificate (Join-Path $releasePath "paneflow-browser-host.dll")
    }

    $pipeName = "paneflow-browser-qualification-$PID"
    $pipePath = "\\.\pipe\$pipeName"
    $server = [System.IO.Pipes.NamedPipeServerStream]::new(
        $pipeName,
        [System.IO.Pipes.PipeDirection]::InOut,
        1,
        [System.IO.Pipes.PipeTransmissionMode]::Byte,
        [System.IO.Pipes.PipeOptions]::Asynchronous
    )
    $oldVariables = @{
        PANEFLOW_CEF_ROOT = $env:PANEFLOW_CEF_ROOT
        PANEFLOW_CEF_PROFILE = $env:PANEFLOW_CEF_PROFILE
        PANEFLOW_BROWSER_OWNER = $env:PANEFLOW_BROWSER_OWNER
        PANEFLOW_BROWSER_CONTROL_PIPE = $env:PANEFLOW_BROWSER_CONTROL_PIPE
        PANEFLOW_CEF_SANDBOX = $env:PANEFLOW_CEF_SANDBOX
        PANEFLOW_BROWSER_SUBPROCESS_PATH = $env:PANEFLOW_BROWSER_SUBPROCESS_PATH
    }
    $env:Path = "$releasePath;$oldPath"
    $env:PANEFLOW_CEF_ROOT = $browserPath
    $env:PANEFLOW_CEF_PROFILE = $profilePath
    $env:PANEFLOW_BROWSER_OWNER = "qualification/ep001"
    $env:PANEFLOW_BROWSER_CONTROL_PIPE = $pipePath
    $env:PANEFLOW_CEF_SANDBOX = "required"
    $env:PANEFLOW_BROWSER_SUBPROCESS_PATH = ""
    $process = Start-Process -FilePath (Join-Path $releasePath "paneflow-browser-host.exe") -WorkingDirectory $releasePath -RedirectStandardError $stderrPath -PassThru
    $connection = $server.WaitForConnectionAsync()
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not $connection.IsCompleted -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    if (-not $connection.IsCompleted) {
        throw "native bootstrap did not connect to the control pipe within $TimeoutSeconds seconds"
    }
    if ($connection.IsFaulted) {
        throw "native bootstrap control pipe failed: $($connection.Exception.InnerException.Message)"
    }
    $handshake = Read-Frame $server
    $value = $handshake | ConvertFrom-Json
    if ($value.native -ne "initialized" -or -not $value.sandbox_requested -or $value.contract_version -ne 3 -or $value.presentation) {
        throw "native bootstrap handshake violates the Windows control contract"
    }
    $createValue = [ordered]@{
        version = 3
        operation = "qualification-create"
        command = [ordered]@{
            type = "create"
            owner = [ordered]@{ workspace = "qualification"; session = "ep001" }
            browser = "qualification"
            profile = "qualification-profile"
            url = "https://example.test/"
            title = "EP-001 qualification"
        }
    }
    Write-Frame $server $createValue
    $createReply = Read-Frame $server | ConvertFrom-Json
    $document = $createReply.protocol.result.Ok.session.document
    if ($null -eq $document) {
        throw "native bootstrap rejected the qualification create command"
    }
    $closeValue = [ordered]@{
        version = 3
        operation = "qualification-close"
        command = [ordered]@{
            type = "close"
            document = $document
        }
    }
    Write-Frame $server $closeValue
    $closeReply = Read-Frame $server | ConvertFrom-Json
    $server.Dispose()
    $server = $null
    $forcedTermination = $false
    if (-not $process.WaitForExit(15000)) {
        $forcedTermination = $true
        & taskkill.exe /T /F /PID $process.Id | Out-Null
        [void]$process.WaitForExit(5000)
    }
    $process.Refresh()
    $exitCode = $null
    if ($process.HasExited) {
        $exitCode = [int]$process.ExitCode
    }
    $signatures = @{}
    foreach ($name in "chrome_elf.dll", "paneflow-browser-host.exe", "paneflow-browser-host.dll") {
        $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $releasePath $name)
        $signatures[$name] = [ordered]@{
            status = $signature.Status.ToString()
            thumbprint = $signature.SignerCertificate.Thumbprint
        }
    }
    $profileAcl = Get-Acl -LiteralPath $profilePath
    $profileAccess = @($profileAcl.Access | ForEach-Object {
        [ordered]@{
            identity = $_.IdentityReference.Value
            rights = $_.FileSystemRights.ToString()
            access_type = $_.AccessControlType.ToString()
            inheritance = $_.InheritanceFlags.ToString()
            propagation = $_.PropagationFlags.ToString()
            inherited = $_.IsInherited
        }
    })
    $receipt = [ordered]@{
        schema_version = 1
        target = $target
        os_version = [Environment]::OSVersion.Version.ToString()
        minimum_windows_build = 17763
        runtime = $runtimePath
        runtime_manifest_stamp = (Get-Content -LiteralPath (Join-Path $runtimePath "verified-manifest.sha256") -Raw).Trim()
        profile = $profilePath
        profile_owner = $profileAcl.Owner
        profile_acl = $profileAccess
        bootstrap = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $releasePath "paneflow-browser-host.exe")).Hash.ToLowerInvariant()
        client = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $releasePath "paneflow-browser-host.dll")).Hash.ToLowerInvariant()
        signatures = $signatures
        handshake = $value
        create_reply = $createReply
        close_reply = $closeReply
        exit_code = $exitCode
        clean_exit = ($null -eq $exitCode -or $exitCode -eq 0)
        forced_termination = $forcedTermination
        stderr = $stderrPath
        sandbox_contract = "bootstrap.exe invokes RunWinMain(sandbox_info); browser_subprocess_path is not used"
    }
    $receipt | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $receiptPath -Encoding UTF8
    $receipt | ConvertTo-Json -Depth 8
}
finally {
    if ($server) {
        $server.Dispose()
    }
    if ($process -and -not $process.HasExited) {
        & taskkill.exe /T /F /PID $process.Id | Out-Null
    }
    $env:Path = $oldPath
    foreach ($name in $oldVariables.Keys) {
        if ($null -eq $oldVariables[$name]) {
            Remove-Item "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item "Env:$name" $oldVariables[$name]
        }
    }
    if ($certificate -and -not $KeepTestCertificate) {
        Remove-Item "Cert:\CurrentUser\Root\$($certificate.Thumbprint)" -ErrorAction SilentlyContinue
        Remove-Item "Cert:\CurrentUser\My\$($certificate.Thumbprint)" -ErrorAction SilentlyContinue
    }
}
