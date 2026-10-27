$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$esc = [char]27
$st = "$esc\"
$below = 'wDAw/8AwMP/AMDD/wDAw/w=='
$above = 'MGDA/zBgwP8wYMD/MGDA/w=='
$out = [Console]::Out

$out.Write("$esc[2J$esc[H")
$out.Write("z-order fixture: compare every line against the same script on main`r`n`r`n")
$out.Write("backgrounds   $esc[41m red $esc[42m green $esc[44m blue $esc[45m magenta $esc[0m $esc[7m inverse $esc[0m`r`n")
$out.Write("blocks        $([char]0x2588)$([char]0x2588)$([char]0x2588)$([char]0x2593)$([char]0x2593)$([char]0x2592)$([char]0x2592)$([char]0x2591)$([char]0x2591)  $([char]0x250C)$([char]0x2500)$([char]0x2500)$([char]0x252C)$([char]0x2500)$([char]0x2500)$([char]0x2510) $([char]0x2502) $([char]0x2514)$([char]0x2500)$([char]0x2500)$([char]0x2534)$([char]0x2500)$([char]0x2500)$([char]0x2518)`r`n")
$out.Write("decorations   $esc[9mstruck through$esc[0m $esc[4msingle underline$esc[0m $esc[4:2mdouble$esc[0m $esc[4:3m$esc[58:2::255:80:80mcurly red$esc[0m $esc[41m$esc[9mstruck on red$esc[0m`r`n")
$out.Write("hyperlink     $esc]8;;https://paneflow.dev$st" + "https://paneflow.dev$esc]8;;$st (hover it to see the underline)`r`n")
$out.Write("selection     drag the mouse across this line: the selection sits under the text`r`n")
$out.Write("`r`n")
$out.Write("kitty below   ")
$out.Write("${esc}_Ga=T,f=32,s=2,v=2,c=24,r=1,z=-1,C=1,q=2;$below$st")
$out.Write("text drawn over a red image`r`n")
$out.Write("kitty above   ")
$out.Write("${esc}_Ga=T,f=32,s=2,v=2,c=24,r=1,z=1,C=1,q=2;$above$st")
$out.Write("text hidden under a blue image`r`n")
$out.Write("`r`n")
$out.Write("$esc[2 qblock cursor after the prompt, press Enter to quit: ")
$out.Flush()
[void][Console]::ReadLine()
$out.Write("$esc[0 q")
