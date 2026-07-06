param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'
$failures = New-Object System.Collections.Generic.List[string]

function Add-Failure {
    param([string]$Message)
    $failures.Add($Message)
}

$tasksDir = Join-Path $Root 'tasks'
$examplesDir = Join-Path $Root 'examples'

Get-ChildItem $tasksDir -Filter '*-status.json' | Sort-Object Name | ForEach-Object {
    $statusFile = $_
    try {
        $status = Get-Content -Raw $statusFile.FullName | ConvertFrom-Json
    } catch {
        Add-Failure "$($statusFile.Name): invalid JSON: $($_.Exception.Message)"
        return
    }

    $prdRel = $status.prd.file
    $prdPath = Join-Path $Root $prdRel
    if (-not (Test-Path $prdPath)) {
        Add-Failure "$($statusFile.Name): PRD file missing: $prdRel"
        return
    }

    $prdText = Get-Content -Raw $prdPath
    $storyHeadings = @{}
    $matches = [regex]::Matches($prdText, '(?m)^####\s+(US-\d+):\s+(.+)$')
    foreach ($match in $matches) {
        $storyHeadings[$match.Groups[1].Value] = $match.Groups[2].Value.Trim()
    }

    $stories = @($status.stories)
    $epics = @($status.epics)
    $storyIds = @{}

    foreach ($story in $stories) {
        if ($storyIds.ContainsKey($story.id)) {
            Add-Failure "$($statusFile.Name): duplicate story id $($story.id)"
        } else {
            $storyIds[$story.id] = $true
        }

        if (-not $storyHeadings.ContainsKey($story.id)) {
            Add-Failure "$($statusFile.Name): story $($story.id) missing from $prdRel"
        }

        if ($story.reviewed_at -and $story.status -ne 'DONE') {
            Add-Failure "$($statusFile.Name): story $($story.id) has reviewed_at but status $($story.status)"
        }

        if ($story.status -eq 'DONE' -and -not $story.completed_at) {
            Add-Failure "$($statusFile.Name): story $($story.id) is DONE without completed_at"
        }

        if ($story.completed_at -and -not $story.started_at) {
            Add-Failure "$($statusFile.Name): story $($story.id) has completed_at without started_at"
        }
    }

    foreach ($headingId in $storyHeadings.Keys) {
        if (-not $storyIds.ContainsKey($headingId)) {
            Add-Failure "$($statusFile.Name): PRD story $headingId missing from status JSON"
        }
    }

    foreach ($epic in $epics) {
        $epicStories = @($stories | Where-Object { $_.epic -eq $epic.id })
        $doneStories = @($epicStories | Where-Object { $_.status -eq 'DONE' })

        if ([int]$epic.stories_total -ne $epicStories.Count) {
            Add-Failure "$($statusFile.Name): epic $($epic.id) stories_total=$($epic.stories_total), actual=$($epicStories.Count)"
        }

        if ([int]$epic.stories_done -ne $doneStories.Count) {
            Add-Failure "$($statusFile.Name): epic $($epic.id) stories_done=$($epic.stories_done), actual=$($doneStories.Count)"
        }

        if ($epic.status -eq 'DONE' -and $doneStories.Count -ne $epicStories.Count) {
            Add-Failure "$($statusFile.Name): epic $($epic.id) is DONE but has incomplete stories"
        }
    }

    $allDone = @($stories | Where-Object { $_.status -eq 'DONE' }).Count -eq $stories.Count
    if ($status.prd.status -eq 'DONE' -and -not $allDone) {
        Add-Failure "$($statusFile.Name): PRD is DONE but not all stories are DONE"
    }

    if ($status.prd.status -eq 'DONE' -and $prdText -match '- \[ \]') {
        Add-Failure "$($statusFile.Name): PRD is DONE but markdown still has open checklist items"
    }
}

$flowPath = Join-Path $examplesDir 'review-pipeline.flow.toml'
$taskPath = Join-Path $examplesDir 'TASK.md'
if (Test-Path $flowPath) {
    $flowText = Get-Content -Raw $flowPath
    if ($flowText -match 'TASK\.md' -and -not (Test-Path $taskPath)) {
        Add-Failure 'examples/review-pipeline.flow.toml references TASK.md but examples/TASK.md is missing'
    }

    if ($flowText -match 'TASK\.md' -and $flowText -match '(?m)^\s*cwd\s*=\s*"\."\s*$') {
        Add-Failure 'examples/review-pipeline.flow.toml references TASK.md but still uses cwd = "."'
    }
}

if ($failures.Count -gt 0) {
    foreach ($failure in $failures) {
        [Console]::Error.WriteLine($failure)
    }
    exit 1
}

Write-Host "task artifact validation passed"
