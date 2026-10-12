# kuterm shell integration for fish, found in XDG_DATA_DIRS as a vendor config
status is-interactive; or exit

function __kuterm_prompt --on-event fish_prompt
    printf '\e]7;file://%s%s\a\e]133;A\a' (hostname) "$PWD"
end

function __kuterm_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end

function __kuterm_postexec --on-event fish_postexec
    printf '\e]133;D;%s\a' $status
end
