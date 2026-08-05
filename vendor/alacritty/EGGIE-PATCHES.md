# Eggie patches

This is the Alacritty terminal engine from revision `4c129667ce56611becdc82de6e28218c80e2e88f`.

Eggie adds DEC private mode 2027 and Unicode extended-grapheme storage so emoji ZWJ,
regional-indicator, variation-selector, and modifier sequences occupy their final terminal width
before reaching the Metal renderer.

The bundled VTE parser also handles OSC 9;4 progress reports (including BEL, ESC ST, and raw C1
ST terminators) and forwards them as terminal events. Eggie keeps this parser local so progress is
decoded by the same state machine as every other terminal escape sequence.
