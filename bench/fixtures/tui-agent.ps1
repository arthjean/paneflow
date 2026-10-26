<#
.SYNOPSIS
Animates an agent-like terminal interface for the performance scenarios.

.DESCRIPTION
Prints 40 lines of text, then rewrites the three bottom lines every 50 ms
inside a DEC 2026 synchronized output block and changes the OSC title every
100 ms, for the given number of seconds. bench/fixtures/tui-agent.sh is the
POSIX shell twin.

.PARAMETER Seconds
How long the interface animates.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 86400)]
    [int]$Seconds
)

$ErrorActionPreference = "Stop"
$esc = [char]27
$bel = [char]7
$rows = [Math]::Max(4, [Console]::WindowHeight)
$status = $rows - 2
$tokens = $rows - 1
$glyphs = @("|", "/", "-", "\")
$titleGlyphs = @([char]0x280B, [char]0x2819, [char]0x2839, [char]0x2838)

[Console]::Write("$esc[2J$esc[H")
for ($line = 1; $line -le 40; $line++) {
    [Console]::Write(("tui fixture line {0:D2}: the transcript an agent keeps above its animated status`r`n" -f $line))
}

$frames = $Seconds * 20
for ($frame = 0; $frame -lt $frames; $frame++) {
    $glyph = $glyphs[$frame % 4]
    $elapsed = [Math]::Floor($frame / 20)
    [Console]::Write("$esc[?2026h$esc[$status;1H$esc[2K$glyph Thinking for ${elapsed}s (esc to interrupt)$esc[$tokens;1H$esc[2K  $($frame * 7) tokens$esc[$rows;1H$esc[2K> $esc[?2026l")
    if ($frame % 2 -eq 0) {
        [Console]::Write("$esc]2;$($titleGlyphs[[Math]::Floor($frame / 2) % 4]) tui fixture$bel")
    }
    Start-Sleep -Milliseconds 50
}
[Console]::Write("`r`ntui fixture done`r`n")
