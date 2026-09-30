$DownloadConnectTimeout = [TimeSpan]::FromSeconds(30)
$DownloadTotalTimeout = [TimeSpan]::FromMinutes(10)

function Save-Download {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$OutFile
    )

    $handler = [System.Net.Http.SocketsHttpHandler]::new()
    $handler.ConnectTimeout = $DownloadConnectTimeout
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = $DownloadTotalTimeout
    try {
        $bytes = $client.GetByteArrayAsync($Uri).GetAwaiter().GetResult()
        [System.IO.File]::WriteAllBytes($OutFile, $bytes)
    }
    finally {
        $client.Dispose()
    }
}
