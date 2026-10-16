# kuterm shell integration for zsh, found through ZDOTDIR. the user's ZDOTDIR is put back
# first, so their own files are read as usual
if [[ -n "$KUTERM_ZDOTDIR" ]]; then
  ZDOTDIR="$KUTERM_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset KUTERM_ZDOTDIR
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"

if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  if [[ -n "$KUTERM_INCOGNITO" ]]; then
    # .zshrc runs later and may set HISTFILE, so nothing is added to the history at all
    __kuterm_nohistory() { return 1 }
    add-zsh-hook zshaddhistory __kuterm_nohistory
    unset HISTFILE
  fi
  __kuterm_precmd() {
    local last=$?
    [[ -n "$__kuterm_ran" ]] && print -n "\e]133;D;$last\a"
    __kuterm_ran=
    print -n "\e]7;file://$HOST$PWD\a\e]133;A\a"
    [[ "$PS1" == *'133;B'* ]] || PS1="$PS1%{"$'\e]133;B\a'"%}"
  }
  __kuterm_preexec() {
    __kuterm_ran=1
    # the command line for osc 133;E, \ and ; and control chars escaped as \\, \x3b, \xNN
    local c=$1
    c=${c//\\/\\\\}
    c=${c//;/\\x3b}
    c=${c//$'\n'/\\x0a}
    c=${c//$'\r'/\\x0d}
    c=${c//$'\e'/\\x1b}
    c=${c//$'\a'/\\x07}
    print -rn -- $'\e]133;E;'"$c"$'\a\e]133;C\a'
  }
  add-zsh-hook precmd __kuterm_precmd
  add-zsh-hook preexec __kuterm_preexec

  # autocomplete: the line editor reports its line to kuterm over the tab's private socket,
  # and kuterm sends picked completions back, which are put into the line here. without the
  # socket module or zle hooks (zsh before 5.3) it stays off and nothing else changes
  if [[ -n "$KUTERM_COMPLETION_SOCKET" ]] && zmodload zsh/net/socket 2>/dev/null \
    && autoload -Uz add-zle-hook-widget 2>/dev/null; then
    typeset -gi __kuterm_cfd=-1 __kuterm_cseq=0
    typeset -g __kuterm_csent= __kuterm_cpath=

    # $1 as a json string in REPLY. other control characters become one char each, so the
    # cursor still counts the same chars as the line
    __kuterm_cjson() {
      local s=$1
      s=${s//\\/\\\\}
      s=${s//\"/\\\"}
      s=${s//$'\n'/\\n}
      s=${s//$'\t'/\\t}
      s=${s//[[:cntrl:]]/\\ufffd}
      REPLY="\"$s\""
    }

    __kuterm_cclose() {
      zle -F $__kuterm_cfd 2>/dev/null
      exec {__kuterm_cfd}>&- 2>/dev/null
      __kuterm_cfd=-1
    }

    __kuterm_csend() {
      (( __kuterm_cfd >= 0 )) || return
      # a closed socket must not kill the shell with SIGPIPE, the trap is back on return so
      # programs started later get the usual one
      setopt localoptions localtraps
      trap '' PIPE
      print -r -- "$1" >&$__kuterm_cfd 2>/dev/null || __kuterm_cclose
    }

    # a picked completion: "1 accept <seq> <start> <end> <text>", the text runs to the end
    __kuterm_creceive() {
      local line
      if ! IFS= read -r -u $1 line; then
        # kuterm went away
        __kuterm_cclose
        return
      fi
      local -a f
      f=("${(@s: :)line}")
      [[ $f[1] == 1 && $f[2] == accept ]] || return
      # an answer to an older line would edit the wrong text
      [[ $f[3] == $__kuterm_cseq ]] || return
      local start=$f[4] end=$f[5] text=${line#* * * * * }
      [[ $start == <-> && $end == <-> ]] || return
      (( start <= end && end <= $#BUFFER )) || return
      BUFFER="${BUFFER[1,start]}$text${BUFFER[end+1,-1]}"
      CURSOR=$(( start + $#text ))
      # a redraw from here runs no line-pre-redraw hook, so the new line is reported here
      __kuterm_creport
      zle -R
    }
    zle -N __kuterm_creceive

    __kuterm_creport() {
      if (( __kuterm_cfd < 0 )); then
        zsocket "$KUTERM_COMPLETION_SOCKET" 2>/dev/null || return
        __kuterm_cfd=$REPLY
        zle -F -w $__kuterm_cfd __kuterm_creceive
        __kuterm_cpath=
        __kuterm_csend '{"v":1,"type":"hello","shell":"zsh"}'
      fi
      if [[ "$PATH" != "$__kuterm_cpath" ]]; then
        __kuterm_cpath=$PATH
        __kuterm_cjson "$PATH"
        __kuterm_csend "{\"v\":1,\"type\":\"env\",\"path\":$REPLY}"
      fi
      # redraws without a change send nothing
      [[ "$CURSOR:$BUFFER" == "$__kuterm_csent" ]] && return
      __kuterm_csent="$CURSOR:$BUFFER"
      (( ++__kuterm_cseq ))
      local buffer
      __kuterm_cjson "$BUFFER"
      buffer=$REPLY
      __kuterm_cjson "$PWD"
      __kuterm_csend "{\"v\":1,\"type\":\"line\",\"seq\":$__kuterm_cseq,\"cursor\":$CURSOR,\"buffer\":$buffer,\"cwd\":$REPLY}"
    }

    __kuterm_cfinish() {
      __kuterm_csent=
      __kuterm_csend '{"v":1,"type":"end"}'
    }

    # line-init hooks added this early never run, so they go in before the first prompt
    __kuterm_chooks() {
      add-zsh-hook -d precmd __kuterm_chooks
      add-zle-hook-widget line-init __kuterm_creport
      add-zle-hook-widget line-pre-redraw __kuterm_creport
      add-zle-hook-widget line-finish __kuterm_cfinish
    }
    add-zsh-hook precmd __kuterm_chooks
  fi
fi
