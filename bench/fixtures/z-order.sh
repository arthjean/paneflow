#!/bin/sh
set -eu

below='wDAw/8AwMP/AMDD/wDAw/w=='
above='MGDA/zBgwP8wYMD/MGDA/w=='

printf '\033[2J\033[H'
printf 'z-order fixture: compare every line against the same script on main\r\n\r\n'
printf 'backgrounds   \033[41m red \033[42m green \033[44m blue \033[45m magenta \033[0m \033[7m inverse \033[0m\r\n'
printf 'blocks        \342\226\210\342\226\210\342\226\210\342\226\223\342\226\223\342\226\222\342\226\222\342\226\221\342\226\221  \342\224\214\342\224\200\342\224\200\342\224\254\342\224\200\342\224\200\342\224\220 \342\224\202 \342\224\224\342\224\200\342\224\200\342\224\264\342\224\200\342\224\200\342\224\230\r\n'
printf 'decorations   \033[9mstruck through\033[0m \033[4msingle underline\033[0m \033[4:2mdouble\033[0m \033[4:3m\033[58:2::255:80:80mcurly red\033[0m \033[41m\033[9mstruck on red\033[0m\r\n'
printf 'hyperlink     \033]8;;https://paneflow.dev\033\\https://paneflow.dev\033]8;;\033\\ (hover it to see the underline)\r\n'
printf 'selection     drag the mouse across this line: the selection sits under the text\r\n'
printf '\r\n'
printf 'kitty below   '
printf '\033_Ga=T,f=32,s=2,v=2,c=24,r=1,z=-1,C=1,q=2;%s\033\\' "$below"
printf 'text drawn over a red image\r\n'
printf 'kitty above   '
printf '\033_Ga=T,f=32,s=2,v=2,c=24,r=1,z=1,C=1,q=2;%s\033\\' "$above"
printf 'text hidden under a blue image\r\n'
printf '\r\n'
printf '\033[2 qblock cursor after the prompt, press Enter to quit: '
read -r _
printf '\033[0 q'
