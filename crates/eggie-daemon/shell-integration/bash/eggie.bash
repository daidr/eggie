# Eggie bash shell integration.
#
# Authored by the Eggie project from the public OSC 133 semantic-prompt protocol
# (FinalTerm / iTerm2 / kitty de-facto standard). This file is NOT derived from
# any GPL-licensed integration script.
#
# Eggie launches bash as `bash --posix` with ENV pointing at this file, so bash
# sources it at startup. We use POSIX mode purely as an injection hook: this
# script leaves POSIX mode, reloads the normal bash startup files the user would
# otherwise get, restores their original ENV, and installs OSC 133 hooks.
#
# OSC 133 marks used:
#   A;redraw=last (FreshLineAndPrompt) — start of a fresh prompt. redraw=last
#       tells Eggie bash only reprints the LAST line of a multiline prompt on
#       resize, so only that line should be cleared.
#   B (InputStart)               — end of prompt / start of the input area
#   C (OutputStart)              — command started producing output
#   D;<status> (CommandFinished) — previous command finished with exit status
#   P;k=s (PromptStart, secondary) — continuation line of a multiline prompt

# Only proceed when injected by Eggie (EGGIE_BASH_INJECT is set by the daemon) and
# in an interactive shell.
if [[ -n "${EGGIE_BASH_INJECT-}" && "$-" == *i* ]]; then
    # Leave POSIX mode so the shell behaves like a normal interactive bash.
    builtin set +o posix

    # Restore the user's original ENV (we overrode it to point here).
    if [[ -n "${EGGIE_BASH_ENV+x}" ]]; then
        builtin export ENV="$EGGIE_BASH_ENV"
        builtin unset EGGIE_BASH_ENV
    else
        builtin unset ENV
    fi

    # Reload the startup files bash would normally read for an interactive shell,
    # honoring the flags the user passed (--norc / --noprofile / --rcfile), which
    # the daemon forwarded via EGGIE_BASH_INJECT / EGGIE_BASH_RCFILE.
    _eggie_inject="$EGGIE_BASH_INJECT"
    builtin unset EGGIE_BASH_INJECT
    if [[ "$_eggie_inject" != *"--norc"* ]]; then
        if [[ -n "${EGGIE_BASH_RCFILE+x}" ]]; then
            [[ -r "$EGGIE_BASH_RCFILE" ]] && builtin source "$EGGIE_BASH_RCFILE"
        elif [[ -r "$HOME/.bashrc" ]]; then
            builtin source "$HOME/.bashrc"
        fi
    fi
    builtin unset EGGIE_BASH_RCFILE 2>/dev/null
    builtin unset _eggie_inject

    # path feature: append Eggie's binary directory to PATH (if the `path` token is set and not
    # already present) so `eggie +...` is runnable here. Done *after* reloading the user's rc files
    # above, so a .bashrc that resets PATH can't drop our directory. Comma-delimited token match.
    if [[ ",${EGGIE_SHELL_FEATURES-}," == *",path,"* && -n "${EGGIE_BIN_DIR-}" ]]; then
        if [[ ":$PATH:" != *":$EGGIE_BIN_DIR:"* ]]; then
            builtin export PATH="$PATH:$EGGIE_BIN_DIR"
        fi
    fi

    _eggie_fd=1
    _eggie_running=""

    __eggie_precmd() {
        local ret="$?"

        # Close the previous command's output region with its exit status.
        if [[ -n "$_eggie_running" ]]; then
            builtin printf '\e]133;D;%s\a' "$ret" >&$_eggie_fd
            _eggie_running=""
        fi

        # Weave prompt-start / input-start marks into PS1 (and continuation marks
        # into PS2) as \[...\] (non-printing) so readline's column math is correct.
        # Save and restore around re-marking so marks never accumulate.
        if [[ -n "${_eggie_marked_ps1+x}" && "$PS1" == "$_eggie_marked_ps1" ]]; then
            PS1="$_eggie_saved_ps1"
            PS2="$_eggie_saved_ps2"
        fi
        _eggie_saved_ps1="$PS1"
        _eggie_saved_ps2="$PS2"

        PS1='\[\e]133;A;redraw=last;cl=line\a\]'"$PS1"'\[\e]133;B\a\]'
        # Mark each continuation line of a multiline PS1 as a secondary prompt.
        if [[ "$PS1" == *'\n'* ]]; then
            PS1="${PS1//\\n/\\n\\[\\e]133;P;k=s\\a\\]}"
        fi
        PS2='\[\e]133;P;k=s\a\]'"$PS2"'\[\e]133;B\a\]'

        _eggie_marked_ps1="$PS1"
    }

    # PS0 is expanded after reading a command, just before it runs: mark output
    # start there. \e]133;C
    PS0='\[\e]133;C\a\]'"${PS0-}"
    # Track that a command is running so __eggie_precmd can close it.
    __eggie_mark_running() { _eggie_running=1; }
    PS0="$PS0"'$(__eggie_mark_running)'

    # Chain our precmd onto any existing PROMPT_COMMAND.
    if [[ -z "${PROMPT_COMMAND-}" ]]; then
        PROMPT_COMMAND=__eggie_precmd
    elif [[ "$PROMPT_COMMAND" != *__eggie_precmd* ]]; then
        PROMPT_COMMAND=$'__eggie_precmd\n'"$PROMPT_COMMAND"
    fi
fi
