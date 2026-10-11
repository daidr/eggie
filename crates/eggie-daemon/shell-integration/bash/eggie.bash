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

    __eggie_escape() {
        REPLY=$1
        REPLY=${REPLY//\%/%25}; REPLY=${REPLY//;/%3B}
        REPLY=${REPLY//$'\e'/%1B}; REPLY=${REPLY//$'\a'/%07}
        REPLY=${REPLY//$'\n'/%0A}; REPLY=${REPLY//$'\r'/%0D}; REPLY=${REPLY//$'\t'/%09}
    }

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
        local completion_cwd completion_path completion_home completion_native=0
        ((BASH_VERSINFO[0]>=4)) && completion_native=1
        __eggie_escape "$PWD"; completion_cwd=$REPLY
        __eggie_escape "$PATH"; completion_path=$REPLY
        __eggie_escape "$HOME"; completion_home=$REPLY
        printf '\e]1337;EggieShell=bash;%s;%s;%s;%s\a' "$completion_native" "$completion_cwd" "$completion_path" "$completion_home"

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

        # Report the working directory on every prompt, so a remote session's cwd is not mistaken for
        # a local path. Only literal `%` needs encoding (as %25) to survive the round-trip.
        local encoded_pwd="${PWD//\%/%25}"
        printf '\e]7;file://%s%s\a' "$(hostname)" "$encoded_pwd" >&$_eggie_fd
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

# Readline invokes this only on Eggie's private completion chord; ordinary bindings remain intact.
if [[ $- == *i* && ${BASH_VERSINFO[0]} -ge 4 ]]; then
    __eggie_report_input() {
        local text=$READLINE_LINE before cursor cwd path_value home_value encoded
        local LC_ALL=C
        before=${text:0:READLINE_POINT}
        unset LC_ALL
        cursor=${#before}
        __eggie_escape "$PWD"; cwd=$REPLY
        __eggie_escape "$PATH"; path_value=$REPLY
        __eggie_escape "$HOME"; home_value=$REPLY
        __eggie_escape "$text"; encoded=$REPLY
        printf '\e]1337;EggieInput=bash;%s;%s;%s;%s;1;%s\a' "$cursor" "$cwd" "$path_value" "$home_value" "$encoded"
    }
    __eggie_complete() {
        local text=$READLINE_LINE byte_cursor=$READLINE_POINT before cursor cwd path_value home_value packet value
        local LC_ALL=C
        before=${text:0:byte_cursor}
        unset LC_ALL
        cursor=${#before}
        __eggie_escape "$PWD"; cwd=$REPLY
        __eggie_escape "$PATH"; path_value=$REPLY
        __eggie_escape "$HOME"; home_value=$REPLY
        __eggie_escape "$text"; packet=$REPLY
        printf '\e]1337;EggieInput=bash;%s;%s;%s;%s;1;%s\a' "$cursor" "$cwd" "$path_value" "$home_value" "$packet"
        packet+=";$cursor"
        local COMP_LINE=$text COMP_POINT=$byte_cursor COMP_TYPE=9 COMP_KEY=9 COMP_CWORD
        local -a COMP_WORDS COMPREPLY spec
        local word='' quote='' character escaped=0 i
        for ((i=0;i<${#before};i++)); do
            character=${before:i:1}
            if ((escaped)); then word+="$character"; escaped=0
            elif [[ $character == \\ && $quote != "'" ]]; then escaped=1
            elif [[ -n $quote ]]; then
                if [[ $character == "$quote" ]]; then quote=''; else word+="$character"; fi
            elif [[ $character == "'" || $character == '"' ]]; then quote=$character
            elif [[ $character == ' ' || $character == $'\t' || $character == $'\n' ]]; then
                if [[ $character == $'\n' ]]; then COMP_WORDS=(); else [[ -n $word ]] && COMP_WORDS+=("$word"); fi; word=''
            elif [[ $character == '|' || $character == ';' || $character == '&' ]]; then
                COMP_WORDS=(); word=''
            else word+="$character"; fi
        done
        COMP_WORDS+=("$word")
        COMP_CWORD=$((${#COMP_WORDS[@]}-1))
        local current=${COMP_WORDS[COMP_CWORD]-} cmd=${COMP_WORDS[0]-} previous=${COMP_WORDS[COMP_CWORD-1]-}
        if (( COMP_CWORD > 0 )); then
            declare -F _completion_loader >/dev/null && _completion_loader "$cmd" 2>/dev/null
            local definition
            definition=$(complete -p -- "$cmd" 2>/dev/null)
            # complete -p produces shell-quoted definitions from the user's shell config.
            [[ -n $definition ]] && eval "spec=(${definition#complete })"
            local fn='' i
            for ((i=0;i<${#spec[@]};i++)); do [[ ${spec[i]} == -F ]] && fn=${spec[i+1]}; done
            if [[ -n $fn ]] && declare -F "$fn" >/dev/null; then "$fn" "$cmd" "$current" "$previous" 2>/dev/null
            elif ((${#spec[@]})); then mapfile -t COMPREPLY < <(compgen "${spec[@]:0:${#spec[@]}-1}" -- "$current" 2>/dev/null); fi
        fi
        if ((${#COMPREPLY[@]} == 0)); then
            if (( COMP_CWORD == 0 )); then mapfile -t COMPREPLY < <(compgen -c -- "$current")
            elif [[ $cmd == cd || $cmd == pushd ]]; then mapfile -t COMPREPLY < <(compgen -d -- "$current")
            else mapfile -t COMPREPLY < <(compgen -f -- "$current"); fi
        fi
        for value in "${COMPREPLY[@]:0:2000}"; do
            [[ -d $value ]] && value+=/
            __eggie_escape "$value"; packet+=";$REPLY"
        done
        printf '\e]1337;EggieCompletions=%s\a' "$packet"
        READLINE_LINE=$text; READLINE_POINT=$byte_cursor
    }
    bind -m emacs-standard -x '"\C-x\C-_":__eggie_complete' 2>/dev/null
    bind -m vi-insertion -x '"\C-x\C-_":__eggie_complete' 2>/dev/null
fi

if [[ $- == *i* && ${BASH_VERSINFO[0]} -ge 4 ]]; then
    __eggie_apply_completion() {
        local packet character expected position from to insert left right
        while IFS= read -r -s -n 1 -t 1 character; do
            [[ -z $character ]] && break
            packet+="$character"
            ((${#packet}>200000)) && return
        done
        IFS=';' read -r expected position from to insert <<< "$packet"
        expected=${expected//\\/\\\\}; expected=${expected//\%/\\x}
        printf -v expected '%b' "$expected"
        [[ $position$from$to =~ ^[0-9]+$ ]] || return
        [[ $READLINE_LINE == "$expected" ]] || return
        local LC_ALL=C
        left=${READLINE_LINE:0:READLINE_POINT}
        unset LC_ALL
        ((${#left}==position && from<=to && to<=${#READLINE_LINE})) || return
        insert=${insert//\\/\\\\}; insert=${insert//\%/\\x}
        printf -v insert '%b' "$insert"
        left=${READLINE_LINE:0:from}; right=${READLINE_LINE:to}
        READLINE_LINE="$left$insert$right"
        LC_ALL=C
        READLINE_POINT=$((${#left}+${#insert}))
        unset LC_ALL
        __eggie_report_input
    }
    bind -m emacs-standard -x '"\C-x\C-^":__eggie_apply_completion' 2>/dev/null
    bind -m vi-insertion -x '"\C-x\C-^":__eggie_apply_completion' 2>/dev/null
fi
