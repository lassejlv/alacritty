# Restore the user's startup directory before loading their configuration.
if [[ ${ALACRITTY_ZDOTDIR_SET-} == 1 ]]; then
    export ZDOTDIR=$ALACRITTY_ZDOTDIR
else
    unset ZDOTDIR
fi
unset ALACRITTY_ZDOTDIR ALACRITTY_ZDOTDIR_SET
[[ -r ${ZDOTDIR:-$HOME}/.zshenv ]] && source ${ZDOTDIR:-$HOME}/.zshenv

if [[ -o interactive && -n ${ALACRITTY_SHELL_INTEGRATION-} ]]; then
    _alacritty_init() {
        autoload -Uz add-zsh-hook
        add-zsh-hook -d precmd _alacritty_init
        source "$ALACRITTY_SHELL_INTEGRATION"
        _alacritty_precmd
        unfunction _alacritty_init
    }
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd _alacritty_init
fi
