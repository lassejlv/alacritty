# OSC 7 and OSC 133 shell integration. Safe to source more than once.
[[ -o interactive ]] || return
[[ ${_alacritty_integrated-} == 1 ]] && return
typeset -g _alacritty_integrated=1
typeset -g _alacritty_command_running=0

_alacritty_precmd() {
    local command_status=$? prompt_percent=$options[promptpercent]
    emulate -L zsh
    if (( _alacritty_command_running )); then
        builtin printf '\e]133;D;%d\a' "$command_status"
        _alacritty_command_running=0
    fi
    # Encode UTF-8 bytes, including spaces, semicolons and control characters.
    local LC_ALL=C encoded='' character hex
    local -i i
    for (( i=1; i <= ${#PWD}; i++ )); do
        character=${PWD[i]}
        case $character in
            [a-zA-Z0-9/._~-]) encoded+=$character ;;
            *) builtin printf -v hex '%%%02X' "'$character"; encoded+=$hex ;;
        esac
    done
    builtin printf '\e]7;file://%s%s\a' "$HOST" "$encoded"
    if [[ $prompt_percent == on ]]; then
        PS1=${PS1//$'%{\e]133;A\a%}'}
        PS1=${PS1//$'%{\e]133;B\a%}'}
        PS1=$'%{\e]133;A\a%}'$PS1$'%{\e]133;B\a%}'
        PS2=${PS2//$'%{\e]133;A;k=s\a%}'}
        PS2=$'%{\e]133;A;k=s\a%}'$PS2
    else
        builtin printf '\e]133;A\a'
    fi
    return $command_status
}

_alacritty_preexec() {
    builtin printf '\e]133;C\a'
    _alacritty_command_running=1
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _alacritty_precmd
add-zsh-hook preexec _alacritty_preexec
