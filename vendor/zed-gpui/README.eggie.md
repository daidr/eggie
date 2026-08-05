# Eggie GPUI fork

This directory contains the minimal Cargo dependency closure used by Eggie from Zed revision
`35cb7558a9d9a6f2eaf31ce2e4dce4a0575820ef`.

Eggie carries this source locally because its terminal renderer needs a compositor-level Metal
primitive that is not part of GPUI's public API at that revision. The fork adds:

- a `PaintMetal` scene primitive with normal GPUI draw ordering and content masks;
- a macOS `MetalPrimitiveRenderer` callback encoded into GPUI's current command encoder;
- access to GPUI's pooled per-frame instance buffer for zero-extra-allocation instance uploads.

The terminal implementation remains in `crates/eggie-ui`; this fork only exposes a generic custom
Metal primitive. Upstream files retain their original Apache-2.0/GPL licensing.
