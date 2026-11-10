$DownloadConnectTimeoutSeconds = 30
$DownloadTotalTimeoutSeconds = 600
$DownloadRetries = 5
$DownloadRetryDelaySeconds = 5

function Save-Download {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$OutFile
    )

    $curl = if ($IsWindows) { "curl.exe" } else { "curl" }
    if (-not (Get-Command $curl -CommandType Application -ErrorAction SilentlyContinue)) {
        throw "$curl is required to download $Uri"
    }
    $arguments = @(
        "--fail", "--silent", "--show-error", "--location",
        "--connect-timeout", $DownloadConnectTimeoutSeconds,
        "--max-time", $DownloadTotalTimeoutSeconds,
        "--retry", $DownloadRetries,
        "--retry-delay", $DownloadRetryDelaySeconds,
        "--retry-all-errors"
    )
    if ($IsWindows) {
        $arguments += "--ssl-revoke-best-effort"
    }
    & $curl @arguments --output $OutFile $Uri
    if ($LASTEXITCODE -ne 0) {
        throw "download of $Uri failed after $DownloadRetries retries (curl exit $LASTEXITCODE)"
    }
}
