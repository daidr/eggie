# Eggie patches

This is the Alacritty terminal engine from revision `4c129667ce56611becdc82de6e28218c80e2e88f`.

Eggie adds DEC private mode 2027 and Unicode extended-grapheme storage so emoji ZWJ,
regional-indicator, variation-selector, and modifier sequences occupy their final terminal width
before reaching the Metal renderer.

The bundled VTE parser also handles OSC 9;4 progress reports (including BEL, ESC ST, and raw C1
ST terminators) and forwards them as terminal events. Eggie keeps this parser local so progress is
decoded by the same state machine as every other terminal escape sequence.

OSC 133 semantic prompt events capture the cursor, history depth and full-screen row displacement
under the terminal lock. Grid displacement is independent of retained history, so Eggie can keep
prompt anchors aligned when scrollback is disabled or saturated. The counter is per grid, excluded
from serialization, and does not change the cell layout. Eggie rebases it after resize before
allowing the shell to redraw its prompt.

The PTY loop can coalesce geometry requests and place them in the parser's byte stream through
`with_terminal_resize`. This keeps old-width bytes ahead of the resize and new-width bytes after
it. A prompt shrink retains only the latest requested width for 100 ms; input and tracked paste
commit pending geometry before writing. Stock `Msg::Resize` retains its original PTY-only behavior.
