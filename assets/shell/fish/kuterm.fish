# kuterm shell integration for fish, found in XDG_DATA_DIRS as a vendor config
status is-interactive; or exit

function __kuterm_prompt --on-event fish_prompt
    printf '\e]7;file://%s%s\a\e]133;A\a' (hostname) "$PWD"
end

function __kuterm_preexec --on-event fish_preexec
    # the command line for osc 133;E, \ and ; and control chars escaped as \\, \x3b, \xNN
    set -l c (printf '%s' $argv[1] | string replace -a '\\' '\\\\' | string replace -a ';' '\\x3b' | string replace -a \e '\\x1b' | string replace -a \a '\\x07' | string replace -a \r '\\x0d')
    printf '\e]133;E;%s\a\e]133;C\a' (string join '\\x0a' $c)
end

function __kuterm_postexec --on-event fish_postexec
    printf '\e]133;D;%s\a' $status
end
