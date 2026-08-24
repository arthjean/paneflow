function Get-ManifestString {
    param([Parameter(Mandatory = $true)][string]$Key)

    $pattern = '^' + [regex]::Escape($Key) + ' = "(.*)"$'
    $match = Get-Content -LiteralPath $ManifestPath |
        Select-String -Pattern $pattern |
        Select-Object -First 1
    if ($null -eq $match) {
        throw "libghostty manifest is missing '$Key'"
    }
    return $match.Matches[0].Groups[1].Value
}

function Get-ManifestTargetRaw {
    param(
        [Parameter(Mandatory = $true)][string]$Target,
        [Parameter(Mandatory = $true)][string]$Key
    )

    $targetPattern = '^\[targets\."' + [regex]::Escape($Target) + '"\]$'
    $keyPattern = '^' + [regex]::Escape($Key) + ' = (.+)$'
    $inTarget = $false
    foreach ($line in Get-Content -LiteralPath $ManifestPath) {
        if ($line -match '^\[targets\."') {
            $inTarget = $line -match $targetPattern
            continue
        }
        if ($inTarget -and $line -match '^\[') {
            break
        }
        if ($inTarget -and $line -match $keyPattern) {
            return $matches[1]
        }
    }
    throw "libghostty manifest target '$Target' is missing '$Key'"
}

function Get-ManifestTargetString {
    param(
        [Parameter(Mandatory = $true)][string]$Target,
        [Parameter(Mandatory = $true)][string]$Key
    )

    $raw = Get-ManifestTargetRaw $Target $Key
    if ($raw -notmatch '^"(.*)"$') {
        throw "libghostty manifest target '$Target' value '$Key' is not a string"
    }
    return $matches[1]
}

function Get-ManifestTargetBoolean {
    param(
        [Parameter(Mandatory = $true)][string]$Target,
        [Parameter(Mandatory = $true)][string]$Key
    )

    $raw = Get-ManifestTargetRaw $Target $Key
    if ($raw -notmatch '^(true|false)$') {
        throw "libghostty manifest target '$Target' value '$Key' is not a boolean"
    }
    return $raw -eq "true"
}

function Get-ManifestTargetStringArray {
    param(
        [Parameter(Mandatory = $true)][string]$Target,
        [Parameter(Mandatory = $true)][string]$Key
    )

    $raw = Get-ManifestTargetRaw $Target $Key
    if ($raw -notmatch '^\[(.*)\]$') {
        throw "libghostty manifest target '$Target' value '$Key' is not an array"
    }
    $body = $matches[1].Trim()
    if ([string]::IsNullOrWhiteSpace($body)) {
        return @()
    }
    return @($body.Split(',') | ForEach-Object {
        $item = $_.Trim()
        if ($item -notmatch '^"(.*)"$') {
            throw "libghostty manifest target '$Target' value '$Key' contains a non-string"
        }
        $matches[1]
    })
}
