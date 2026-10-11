# Eggie fish shell integration — OSC 133 semantic-prompt marker injector.
#
# Authored by the Eggie project from the public OSC 133 semantic-prompt protocol
# (FinalTerm / iTerm2 / kitty de-facto standard). This file is NOT derived from
# any GPL-licensed integration script.
#
# Eggie launches fish with XDG_DATA_DIRS prepended with the integration root,
# so fish sources this file from `fish/vendor_conf.d` at startup without
# touching the user's own config. It installs OSC 133 hooks so Eggie knows
# which grid line a prompt starts on and where command output runs.
#
# OSC 133 marks used:
#   A;cl=line (FreshLineAndPrompt) — start of a fresh prompt (cursor on the
#       prompt's first line). `cl=line` marks the region as clear-and-redraw-safe.
#   C (OutputStart)               — command started producing output, carrying the
#       command line base64-encoded (cmdline_b64) for the history panel.
#   D;<status> (CommandFinished)  — previous command finished with exit status.
#
# B (InputStart) is emitted by the wrapped prompt after its visible content so
# the daemon can distinguish editable input from prompt text on fish 3.x too.
#
# fish 4.x+ natively emits OSC 133 A/B/C/D (mark-prompt feature, default on), so
# on 4.x this script double-emits. The daemon's tracker absorbs the duplicates
# harmlessly: a second A in Prompt phase doesn't move the start line, a second C
# updates the command line (our cmdline_b64 wins over fish's cmdline_url), and a
# second D in None phase is ignored. On fish 3.x (no native marks) this script
# is the sole emitter.
#
# OSC 7 reports the working directory so Eggie tracks the shell's cwd directly
# (more accurate than the foreground-process fallback). fish does not natively
# emit OSC 7, so this is needed on all versions.

# vendor_conf.d is only sourced for interactive shells, but guard anyway so a
# manual `source` in a script can't install hooks. `return` exits this sourced
# file (never `exit`, which would kill the shell).
status is-interactive || return 0

# Run only once per shell.
set -q EGGIE_FISH_INTEGRATION && return 0
set -g EGGIE_FISH_INTEGRATION 1

# path feature: when EGGIE_SHELL_FEATURES contains the `path` token and Eggie
# told us where its binary lives (EGGIE_BIN_DIR), append that directory to PATH
# (skipping if already present) so `eggie +version` and the other CLI actions
# are runnable from this shell. `string match` is full-string glob, so the
# pattern needs wildcards on both sides to match a token in a comma list.
if string match -q '*,path,*' ",$EGGIE_SHELL_FEATURES,"
    and test -n "$EGGIE_BIN_DIR"
    and not string match -q "*:$EGGIE_BIN_DIR:*" ":$PATH:"
    set -gx PATH "$PATH:$EGGIE_BIN_DIR"
end

function __eggie_preexec --on-event fish_preexec
    # $argv[1] is the command line about to run. Skip empty command lines
    # (Enter on an empty prompt): zsh's PS0 does not fire for those either, and
    # an empty record would only clutter the history panel.
    set -l cmdline $argv[1]
    test -n "$cmdline" || return 0

    # Attach the command line base64-encoded so Eggie can show real command text
    # in its history panel; base64 is robust for UTF-8 (CJK), semicolons, and
    # newlines that would otherwise break the OSC 133 sequence.
    set -l encoded (printf '%s' $cmdline | base64 | tr -d '\n')
    if test -n "$encoded"
        printf '\e]133;C;cmdline_b64=%s\a' "$encoded"
    else
        printf '\e]133;C\a'
    end
    set -g __eggie_running 1
end

function __eggie_postexec --on-event fish_postexec
    # Capture $status FIRST: the `if set -q` guard below is itself a command that
    # overwrites $status with 0, so reading it after the guard would report every
    # command as successful. Mirrors the zsh/bash pattern of capturing $? on line 1.
    set -l cmd_status $status
    # Close the previous command's output region (if one is open) with its exit
    # status. __eggie_running is set by __eggie_preexec; the guard also covers
    # the theoretical case where postexec fires without a preceding preexec.
    if set -q __eggie_running
        set -e __eggie_running
        printf '\e]133;D;%s\a' "$cmd_status"
    end
end

function __eggie_prompt --on-event fish_prompt
    # Emit prompt-start HERE, before the prompt function runs (fish_prompt event
    # handlers run first), so the cursor is on the prompt's first line. Eggie
    # records this line and clears from it on resize.
    printf '\e]133;A;cl=line\a'
    __eggie_report_cwd
    printf '\e]1337;EggieShell=fish;1;%s;%s;%s\a' (string escape --style=url -- "$PWD") (string escape --style=url -- (string join : $PATH)) (string escape --style=url -- "$HOME")
end

# Report the working directory on every change. --on-variable PWD fires whenever
# the shell's cwd changes (cd, pushd, ...). The initial cwd is already known to
# Eggie (it spawned us there), so no startup emit is needed.
function __eggie_report_cwd --on-variable PWD
    # Send the real hostname so a remote session's cwd is not mistaken for a local path. Eggie's OSC 7
    # decoder passes non-%XX bytes through verbatim, so only literal `%` needs encoding (as %25) to
    # survive the round-trip; spaces and other punctuation are fine raw.
    printf '\e]7;file://%s%s\a' (hostname | string trim) (string replace -a '%' '%25' -- $PWD)
end

# Remove only the builtin's output newline; retain newlines inside the editor buffer.
function __eggie_capture
    set -l value ($argv | string collect --no-trim-newlines --allow-empty)
    string match -qr '(?s)^(?<value>.*)\n$' -- "$value"; or return
    printf '%s' "$value"
end

function __eggie_report_input
    set -l buffer (__eggie_capture commandline --current-buffer | string collect --no-trim-newlines --allow-empty)
    set -l cursor (commandline --cursor)
    printf '\e]1337;EggieInput=fish;%s;%s;%s;%s;1;%s\a' $cursor (string escape --style=url -- "$PWD") (string escape --style=url -- (string join : $PATH)) (string escape --style=url -- "$HOME") (string escape --style=url -- "$buffer")
end

function __eggie_complete
    set -l buffer (__eggie_capture commandline --current-buffer | string collect --no-trim-newlines --allow-empty)
    set -l cursor (commandline --cursor)
    set -l packet (string escape --style=url -- "$buffer")';'$cursor
    __eggie_report_input
    set -l before (__eggie_capture string sub --start 1 --length $cursor -- "$buffer" | string collect --no-trim-newlines --allow-empty)
    set -l values (complete --do-complete "$before")
    for value in $values[1..2000]
        set -l parts (string split -m 1 \t -- "$value")
        set -l name (string unescape -- "$parts[1]")
        set -l description "$parts[2]"
        set packet "$packet;"(string escape --style=url -- "$name"\t"$description")
    end
    printf '\e]1337;EggieCompletions=%s\a' "$packet"
    commandline -f repaint
end
function __eggie_completion_setup --on-event fish_prompt
    bind --mode default (printf '\030\037') __eggie_complete
    bind --mode default (printf '\030\036') __eggie_apply_completion
    bind --mode insert (printf '\030\036') __eggie_apply_completion
    bind --mode insert (printf '\030\037') __eggie_complete
    # Install after user config, preserving the original prompt and its status.
    if not functions -q __eggie_original_prompt
        functions --copy fish_prompt __eggie_original_prompt
        function fish_prompt
            __eggie_original_prompt
            printf '\e]133;B\a'
        end
    end
    functions --erase __eggie_completion_setup
end

function __eggie_apply_completion
    set -l original (__eggie_capture commandline --current-buffer | string collect --no-trim-newlines --allow-empty)
    set -l original_cursor (commandline --cursor)
    set -l packet
    read --null --nchars 200001 --local packet
    test (string length -- "$packet") -le 200000; or return
    set -l fields (string split ';' -- "$packet")
    test (count $fields) -eq 5; or return
    set -l expected (__eggie_capture string unescape --style=url -- "$fields[1]" | string collect --no-trim-newlines --allow-empty)
    test "$expected" = "$original"; or return
    test "$fields[2]" = "$original_cursor"; or return
    set -l start $fields[3]
    set -l end $fields[4]
    string match -rq '^[0-9]+$' -- "$start$end"; or return
    set -l insert (string unescape --style=url -- "$fields[5]")
    set -l left (__eggie_capture string sub --start 1 --length $start -- "$original" | string collect --no-trim-newlines --allow-empty)
    set -l right (__eggie_capture string sub --start (math "$end + 1") -- "$original" | string collect --no-trim-newlines --allow-empty)
    commandline --replace -- "$left$insert$right"
    commandline --cursor (math "$start + "(string length -- "$insert"))
    commandline -f repaint
end
