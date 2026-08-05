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
placements. Decoded pixels are fetched from the daemon in generation-checked chunks and uploaded
to a dedicated Metal texture cache only once. The daemon wire carries image chunks as MessagePack
binary rather than Base64, unchanged image metadata is omitted from snapshot deltas, offscreen
placements are culled from render snapshots, and consecutive placements of one texture are drawn
with one instanced Metal call. Layering and same-z ordering follow Kitty's internal image/reference
order; high-frequency snapshots carry visible descriptors and placements rather than pixel
payloads.

The application is split into a GPUI client and a persistent local daemon. The
daemon owns PTYs and terminal state, so closing or reopening the same build can
detach without interrupting running commands. During development, the UI embeds
a source build identifier; starting a rebuilt binary terminates a daemon with a
different identifier and starts a clean one. This intentionally trades session
continuity for deterministic renderer/terminal-core testing until the update
compatibility contract is finalized.

## Development

```sh
cargo run -p eggie-ui
```

The GUI starts the daemon automatically. Use `cargo test --workspace` for the
domain and protocol tests.

Open the independent settings window from **Eggie → Settings…** or with
<kbd>⌘</kbd><kbd>,</kbd>. Appearance, the separate dark/light Ghostty themes,
terminal font, font size, minimum contrast, and horizontal/vertical terminal
padding are persisted in `~/Library/Application Support/Eggie/settings.json`.
Every change is shown in the fixed preview and applied live to every Eggie window.

To measure interactive terminal latency, launch with
`EGGIE_INPUT_LATENCY=1 cargo run -p eggie-ui`. Eggie reports rolling p50/p95
timings for input-to-snapshot, snapshot-to-Metal, terminal preparation, Metal
encoding, end-to-Metal, and GPUI input-to-presentation latency. It also reports
input events coalesced per frame and events received during a draw. Metrics are
disabled by default at the Eggie layer and add no mutex or allocation work to
its input path when disabled.

Run the repeatable dense-grid preparation benchmark with:

```sh
cargo test --release -p eggie-ui benchmark_dense_terminal_preparation -- --ignored --nocapture
```
