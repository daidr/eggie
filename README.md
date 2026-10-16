# Eggie

Eggie is a macOS-first terminal workspace built with Rust, GPUI, and
Alacritty's terminal core.

The terminal core is vendored at `vendor/alacritty` with Eggie's DEC mode 2027 patch. Extended
Unicode graphemes are stored at their final cell width before the GPUI Metal renderer receives a
snapshot, so emoji ZWJ sequences, flags, variation selectors, and skin-tone modifiers remain one
selectable terminal character.

Eggie implements the Kitty graphics protocol used by Ghostty: RGB/RGBA/PNG transmission over
direct, file, temporary-file, and POSIX shared-memory media; zlib and chunked payloads; image
queries, IDs/numbers, placements, cropping, z-ordering, deletion, scrolling, and Unicode virtual
placements. Decoded pixels are fetched from the daemon through generation-checked shared-memory mappings
and uploaded to a dedicated Metal texture cache only once, with an inline binary fallback.
Unchanged image metadata is omitted from snapshot deltas, offscreen
placements are culled from render snapshots, and consecutive placements of one texture are drawn
with one instanced Metal call. Layering and same-z ordering follow Kitty's internal image/reference
order; high-frequency snapshots carry visible descriptors and placements rather than pixel
payloads.

The application is split into a GPUI client and a persistent local daemon. The
daemon owns PTYs and terminal state, so closing or reopening the GUI can detach
without interrupting running commands. Protocol version is the compatibility
gate; build identifiers are diagnostic. Starting a binary with a new protocol
version terminates the incompatible daemon and creates a clean one.

Visible terminal text uses leased shared-memory rows; the Unix socket carries frame metadata,
control requests, and wakeups. Unchanged rows are shared across snapshots, and exhausted pools
fall back to owned socket frames. See [terminal transport](design/architecture/terminal-transport.md)
for ownership, memory bounds, diagnostics, and repeatable latency benchmarks.

## Command completion

Suggestions appear while typing in a shell prompt. Ctrl-Space opens the list, Tab inserts,
arrow keys navigate, and Escape dismisses. Enter runs the command unless you have navigated
the list. Disable completion in Settings → Advanced to use the shell's native Tab binding.
See [completion architecture and tests](design/architecture/completion.md).

## Paste

Cmd-V pastes directly when the receiving program supports bracketed paste. A line break that
may execute a command opens an editable preview. Cmd-Shift-V always opens the preview;
Return inserts a line break, Cmd-Return confirms, and Escape cancels. Settings → Advanced →
Paste preview selects automatic preview for potentially executable text, multiline shell text,
or neither. Control characters are removed while Unicode, tabs, and indentation are preserved.

Large text is sent with one bracketed-paste delimiter pair and at most 4 KiB in flight.
An overlay reports bytes written to the PTY and offers Cancel; Escape also cancels.
Typing stops the remaining paste before sending the new input. Switching tabs leaves the paste
attached to its original terminal. Applications using Kitty clipboard grants retain rich paste.

## SSH

Select Local shell or SSH in a profile's connection settings. Clearing an SSH host keeps the
profile in SSH mode and blocks launch until corrected. SSH uses the system OpenSSH client,
including its host aliases, keys and jump hosts. Invalid host, user or port input blocks launch; an unfinished edit remains in the settings window
without replacing the last saved connection. Managed connections use a 15-second connection
timeout and encrypted keepalive messages every 15 seconds, with three unanswered messages before
disconnecting. Authentication remains in the terminal.

The information panel shows the remote identity and directories. New tabs and splits on the same
target inherit the remote directory unless their profile specifies one. Discovered services appear
as compact port rows; mappings, actual local addresses and browser actions remain in that terminal's
information panel. Process and port inspection runs while the information panel or a terminal
preview is visible, with a shared cache for terminals on the same host.

SSH connection failures retain their output even when normal process exits are configured to
remove tabs. Reconnect reuses the original launch recipe and last remote directory, preserving the
tab's position and custom title. It starts a new remote shell; it does not resume a previous remote
process after a network interruption. Connecting terminals offer Cancel.

## Development

macOS builds require Xcode with its Metal compiler toolchain (`xcrun -f metal`).
GPUI and terminal shaders are compiled into the application during the build,
so opening a window does not compile shader source at runtime.

```sh
cargo run -p eggie
```

The GUI starts the daemon automatically. Use `cargo test --workspace` for the
domain and protocol tests.

Open the independent settings window from **Eggie → Settings…** or with
<kbd>⌘</kbd><kbd>,</kbd>. Appearance, the separate dark/light Ghostty themes,
terminal font, font size, minimum contrast, and horizontal/vertical terminal
padding are persisted in `~/Library/Application Support/Eggie/preferences.json`.
Changes and temporary theme hover previews are applied live to every Eggie window.

Eggie also supports a Rex-inspired, API-style Lua configuration at
`~/.config/eggie/init.lua`. Lua settings take precedence over the corresponding
GUI controls; GUI preferences remain available when a Lua setting is removed.
Use `eggie config check [path]` to validate a file and `eggie config reload`
to apply changes. `eggie do script.lua` runs a separate event script. See the
[Lua configuration guide](design/architecture/lua-configuration.md) for examples
and the current API.

The interface palette carries each terminal theme's hue into its surfaces while
deriving separate background, surface, hover, border, text, and accent colors.
Terminal colors are never rewritten. To preview every bundled theme side by side
and check UI text, muted text, accent, button contrast, and hue retention, run:

```sh
scripts/theme-palette-audit.sh
```

The generated preview is `target/theme-palette-audit.html`. Its search field
filters themes by name.

To measure interactive terminal latency, launch with
`EGGIE_INPUT_LATENCY=1 cargo run -p eggie`. Eggie reports rolling p50/p95
timings for input-to-snapshot, snapshot-to-Metal, terminal preparation, Metal
encoding, end-to-Metal, and GPUI input-to-presentation latency. It also reports
input events coalesced per frame and events received during a draw. Metrics are
disabled by default at the Eggie layer and add no mutex or allocation work to
its input path when disabled.

Run the repeatable dense-grid preparation benchmark with:

```sh
cargo test --release -p eggie-terminal-view benchmark_dense_terminal_preparation -- --ignored --nocapture
```

## Architecture

See the [design index](design/README.md) for architecture, interaction principles, feature
designs and product decisions. Agent contributors should read [agent.md](agent.md) and update
the relevant design documents in the same change as the implementation.

See [crate boundaries and UI organization](design/architecture/workspace.md).

Run `python3 scripts/check-architecture.py` to check dependency boundaries, and
`cargo test --workspace` for regression tests. The `eggie` crate owns the executable;
`eggie-ui` is the workspace library, `eggie-ui-kit` contains reusable controls, and
`eggie-terminal-view` owns terminal rendering.
