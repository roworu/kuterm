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

# shown after a command is read, right before it runs
PS0="${PS0}\e]133;C\a"
PROMPT_COMMAND="__kuterm_status; ${PROMPT_COMMAND:+$PROMPT_COMMAND; }__kuterm_prompt"
