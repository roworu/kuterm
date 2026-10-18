# kuterm shell integration for bash, read with --rcfile in place of ~/.bashrc.
# marks prompts and commands (osc 133) and reports the folder (osc 7)
if [ -f ~/.bashrc ]; then . ~/.bashrc; fi

__kuterm_status() {
  __kuterm_last=$?
}

__kuterm_prompt() {
  # kuterm only counts an end after a start, so an empty command line ends nothing
  printf '\e]133;D;%s\a\e]7;file://%s%s\a' "$__kuterm_last" "$HOSTNAME" "$PWD"
  # prompt tools like starship rebuild PS1 every time, so the marks are put back
  case "$PS1" in
    *'133;A'*) ;;
    *) PS1="\[\e]133;A\a\]$PS1\[\e]133;B\a\]" ;;
  esac
}

# the command line about to run for osc 133;E, from history: \ and ; and control chars
# escaped as \\, \x3b and \xNN. a line history skips would report the one before, so then
# kuterm reads the line from the screen instead
__kuterm_cmdline() {
  [[ -o history && "$HISTCONTROL" != *ignore[sb]* ]] || return
  local c
  c=$(HISTTIMEFORMAT= builtin history 1) || return
  c="${c#"${c%%[![:space:]]*}"}"
  c="${c#*[0-9]  }"
  c=${c//\\/\\\\}
  c=${c//;/\\x3b}
  c=${c//$'\n'/\\x0a}
  c=${c//$'\r'/\\x0d}
  c=${c//$'\e'/\\x1b}
  c=${c//$'\a'/\\x07}
  printf '\e]133;E;%s\a' "$c"
}

# shown after a command is read, right before it runs
PS0="${PS0}\$(__kuterm_cmdline)\e]133;C\a"
PROMPT_COMMAND="__kuterm_status; ${PROMPT_COMMAND:+$PROMPT_COMMAND; }__kuterm_prompt"
