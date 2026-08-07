# Eggie zsh shell integration — ZDOTDIR entry point.
#
# Authored by the Eggie project from the public OSC 133 semantic-prompt protocol
# (FinalTerm / iTerm2 / kitty de-facto standard). This file is NOT derived from
# any GPL-licensed integration script.
#
# Eggie launches zsh with ZDOTDIR pointing at the directory containing this file,
# so zsh sources it first for every shell. Its job is to (1) restore the user's
# real ZDOTDIR before any other startup file runs, (2) source the user's own
# .zshenv, and (3) load the Eggie integration function in interactive shells.
# Because ZDOTDIR is restored here, the rest of the startup chain (.zprofile,
# .zshrc, .zlogin — e.g. oh-my-zsh) loads exactly as it would without Eggie.

# Capture this file's directory before ZDOTDIR is rewritten below, so we can find
# eggie-integration next to it afterwards.
_eggie_zdotdir="${${(%):-%x}:A:h}"

# Restore the user's real ZDOTDIR. If they had one, EGGIE_ZDOTDIR_ORIG holds it;
# otherwise unset ZDOTDIR so zsh falls back to $HOME (its default).
if [[ -n "${EGGIE_ZDOTDIR_ORIG+x}" ]]; then
    builtin export ZDOTDIR="$EGGIE_ZDOTDIR_ORIG"
    builtin unset EGGIE_ZDOTDIR_ORIG
else
    builtin unset ZDOTDIR
fi

# Source the user's own .zshenv (from the now-restored location) if present, then
# load our integration. The `always` block runs even if the user's .zshenv fails,
# and preserves the exit status.
{
    _eggie_user_zshenv="${ZDOTDIR:-$HOME}/.zshenv"
    [[ -r "$_eggie_user_zshenv" ]] && builtin source -- "$_eggie_user_zshenv"
} always {
    builtin unset _eggie_user_zshenv
    if [[ -o interactive && -r "$_eggie_zdotdir/eggie-integration" ]]; then
        builtin autoload -Uz -- "$_eggie_zdotdir/eggie-integration"
        eggie-integration
        builtin unfunction -- eggie-integration 2>/dev/null
    fi
    builtin unset _eggie_zdotdir
}
