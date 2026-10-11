# Shell completion

Command suggestions belong to the focused terminal's editable prompt. The panel stays
inside that window so navigation never transfers keyboard focus to another native window.
It uses a virtual list, a bounded result set and the existing UI palette and continuous corners.

## Boundaries

- `eggie-completion` owns the UTF-8 input contract, shell word parsing, safe edits and ranking.
  Its `local-providers` feature is enabled by the daemon; UI and protocol consumers do not
  run filesystem scans or subprocesses.
- The daemon owns PATH, filesystem, Git refs, package scripts, SSH host aliases and session
  history providers. Its shared cache is capped at 4 MiB and 64 entries. External providers
  have bounded output and a 350 ms deadline.
- Shell integration owns native completion, including aliases, functions and user-installed
  completion rules. zsh uses its completion widgets, bash uses programmable completion and
  compgen, and fish uses `complete --do-complete`.
- UI owns debounce, request cancellation, selection, viewport placement and keyboard routing.
  Results carry the original text and cursor; an obsolete request cannot reopen the panel.

## Input and edits

OSC 133 anchors identify editable input. `OSC 1337;EggieShell` reports shell capabilities and
cwd/PATH/HOME. `EggieInput` reports the actual buffer and character cursor; the daemon converts
that cursor to a UTF-8 byte offset. Grid reconstruction supplies input for shells without
per-redraw reports. Fish's
unaccepted autosuggestions are excluded; a native query reports the complete buffer when the
cursor has a real suffix. This metadata travels with socket deltas and shared grid frames.

Native queries use a reserved Ctrl-X chord. `EggieCompletions` returns percent-encoded native
candidates without inserting them or opening a shell menu. Cached native results are reused
until another input event. Native requests only enqueue the shell query and return immediately;
the native-result revision refreshes the panel when candidates arrive. Queries for the same
edit are coalesced while pending. Remote sessions use their remote shell integration and never scan
local files or local PATH for a remote prompt. Remote integration is updated by the existing
SSH bootstrap on connection.

Acceptance sends one checked edit: expected buffer, expected cursor, replacement range and
quoted insertion. The shell checks the buffer before changing its editor state. No guessed
backspace count is used for native editors. Fish reads a NUL-terminated packet without opening
a nested interactive reader; zsh and bash read a newline-terminated packet. Enter and native
queries share an input gate: once a command is submitted, queries and checked edits are blocked
until a new prompt. IME composition, paste, alternate screens, scrollback and loss of focus
close the panel.

## Controls

Suggestions appear while typing. Ctrl-Space requests them explicitly; it is configurable as
`terminal_suggest` through the action registry. Tab inserts the selected candidate. Down starts
list navigation, Up retains shell history until list navigation has started, Page Up/Down moves
by a page, Shift-Tab moves upward, and Escape dismisses. Enter submits the original command
unless the user has navigated the list; accepting a candidate never executes it. Clicking outside
closes the panel. Settings → Advanced → Command completion disables the feature and restores
native Tab handling.

## Responsiveness

Local queries use a 16 ms debounce. Native queries independently start after 120 ms of idle
input, so a slow filesystem provider does not delay shell completion. Explicit requests skip
both timers. Native-result refreshes also skip debounce and never trigger a second native query.

The panel keeps one immutable candidate pool and refilters indices while typing. Backspacing
can recover candidates from that pool; only accepting an item rebuilds its quoted edit for the
current input. Selection moves use nearest scrolling, leaving visible rows in place. Identical
results do not notify the UI. Native RPCs do not poll, sleep or repeat local providers; local
queries copy only the last 200 history commands and reuse already-ranked results. Concurrent
misses for the same provider cache key share one fill, while unrelated keys remain independent.

Frames distinguish an editable prompt from a temporarily missing editor report. ZLE reports
its buffer before every redraw, including no-op keys; the daemon does not reconstruct a
half-painted ZLE grid. Missing reports and new completion contexts retain the visible panel
while canceling obsolete work. Candidates retained from an older edit cannot be accepted.
An empty local result waits for the native provider; a native result or its two-second deadline
settles that request. Empty buffers, command submission and explicit dismissal still close
immediately. The panel stays anchored at the current word's start throughout an edit.

Run the reproducible hot-path benchmark with:

```sh
cargo run -p eggie-completion --features local-providers --example benchmark
```

## Verification

```sh
cargo test -p eggie-completion -p eggie-protocol -p eggie-ipc -p eggie-daemon -p eggie-ui --lib
python3 scripts/tests/shell-completion.py
python3 scripts/check-architecture.py
```

PTY tests exercise real zsh, bash and fish when installed: aliases, programmable word rules,
Git branches, paths with spaces, Unicode, stale edits, mid-word replacement and running-program
isolation. Protocol tests cover metadata updates and clearing over deltas and shared memory.
The shell fixture uses an isolated HOME and does not change user shell configuration.
