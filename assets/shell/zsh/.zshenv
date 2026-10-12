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
  __kuterm_precmd() {
    local last=$?
    [[ -n "$__kuterm_ran" ]] && print -n "\e]133;D;$last\a"
    __kuterm_ran=
    print -n "\e]7;file://$HOST$PWD\a\e]133;A\a"
    [[ "$PS1" == *'133;B'* ]] || PS1="$PS1%{"$'\e]133;B\a'"%}"
  }
  __kuterm_preexec() {
    __kuterm_ran=1
    print -n "\e]133;C\a"
  }
  add-zsh-hook precmd __kuterm_precmd
  add-zsh-hook preexec __kuterm_preexec
fi
