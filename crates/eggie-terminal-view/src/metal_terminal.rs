mod atlas;
use atlas::*;
pub(crate) use atlas::{GlyphPreparation, GlyphRasterCache};
mod backend;
pub(crate) use backend::MetalBackend;
#[cfg(test)]
use backend::*;
mod rasterizer;
use crate::input_latency::MetalFrameStats;
use crate::terminal_renderer::{
    MetalCommand, PreparedMetalTerminal, TerminalImageData, TerminalTextureKey,
};
use crate::terminal_sprites;
use anyhow::{Result, anyhow};
use core_foundation::{
    array::CFArray,
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    boolean::CFBoolean,
    dictionary::CFDictionary,
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use core_graphics::{
    base::{kCGImageAlphaNone, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    context::CGContext,
    font::CGGlyph,
    geometry::CGPoint,
};
use core_text::{
    font::{self, CTFont, CTFontRef},
    font_descriptor::{
        kCTFontBoldTrait, kCTFontColorGlyphsTrait, kCTFontFeatureSettingsAttribute,
        kCTFontItalicTrait, kCTFontOrientationDefault, kCTFontVariationAttribute,
    },
    line::CTLine,
    string_attributes::{kCTFontAttributeName, kCTForegroundColorFromContextAttributeName},
};
use gpui::MetalRenderContext;
use memmap2::Mmap;
#[cfg(test)]
use metal::CompileOptions;
use metal::{
    Buffer, Device, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLPixelFormat, MTLPrimitiveType,
    MTLRegion, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLStorageMode,
    MTLTextureUsage, NSUInteger, RenderPipelineDescriptor, RenderPipelineState, SamplerDescriptor,
    SamplerState, Texture, TextureDescriptor,
};
use parking_lot::Mutex;
use rasterizer::*;
pub(crate) use rasterizer::{font_metrics, shape_terminal_run};
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    cell::RefCell,
    collections::HashSet,
    hash::Hash,
    mem,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use metal::{MTLClearColor, MTLLoadAction, MTLStoreAction, RenderPassDescriptor};

const MASK_ATLAS_SIZE: usize = 4096;
const COLOR_ATLAS_SIZE: usize = 2048;
const INITIAL_ATLAS_SIZE: usize = 512;
const GLYPH_QUEUE_CAPACITY: usize = 32_768;
/// Hard ceiling on cached CPU glyph rasters. When the cache grows past this, `prepare_terminal`
/// falls back to keeping only the visible working set plus in-flight jobs, regardless of recency.
const CPU_GLYPH_CACHE_LIMIT: usize = 65_536;
/// Soft threshold above which idle glyphs are reclaimed. Below this the cache never evicts, so
/// ordinary sessions (a few thousand distinct glyphs) keep everything warm. A graphics-heavy
/// workload (e.g. a demo that rasterizes tens of thousands of distinct glyphs) blows past it and
/// then sheds anything not drawn recently, so process memory falls back once the burst ends.
const CPU_GLYPH_CACHE_IDLE_WATERMARK: usize = 4_096;
/// A `Ready` glyph untouched for this many prepared frames is eligible for idle reclamation. At
/// ~60fps this is roughly 10 seconds — long enough that tab switches and scrollback revisits still
/// hit the cache, short enough that a finished burst is reclaimed promptly.
const CPU_GLYPH_CACHE_IDLE_FRAMES: u64 = 600;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontCreateForString(
        current_font: CTFontRef,
        string: CFStringRef,
        range: CFRange,
    ) -> CTFontRef;

    // OpenType feature dictionary keys. `core-text` 21 doesn't re-export these, so link them here.
    static kCTFontOpenTypeFeatureTag: CFStringRef;
    static kCTFontOpenTypeFeatureValue: CFStringRef;
}

#[cfg(test)]
const SHADER: &str = include_str!("metal_terminal/terminal.metal");
const SHADER_LIBRARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/terminal.metallib"));

// Keep this layout in lockstep with the Metal `Instance` above. The final uint4 makes the
// CPU/GPU stride unambiguously 64 bytes; a trailing `uint` + `uint3` is 80 bytes in Metal because
// `uint3` has 16-byte alignment.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Instance {
    origin: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    uv_origin: [f32; 2],
    uv_size: [f32; 2],
    metadata: [u32; 4],
}

fn glyph_key_for_command(
    terminal: &PreparedMetalTerminal,
    command: &MetalCommand,
    scale: f32,
    origin: [f32; 2],
) -> Option<GlyphKey> {
    let MetalCommand::Glyph {
        rect,
        text,
        bold,
        italic,
        sprite,
        glyph_id,
        ..
    } = command
    else {
        return None;
    };
    let (_, pixel_size) = snap_device_rect(origin, *rect, scale);
    let (_, cell_pixel_size) = snap_device_rect(
        origin,
        [rect[0], rect[1], terminal.cell_width, rect[3]],
        scale,
    );
    // Resolve the per-style family and whether CoreText trait synthesis should still apply.
    // Sprites bypass the font entirely, so they keep the regular family and no synthesis.
    let (mut family, mut synth_bold, mut synth_italic) = match (&terminal.font_families, *sprite) {
        (Some(families), false) => {
            let family = Arc::clone(families.family(*bold, *italic));
            if families.has_dedicated_family(*bold, *italic) {
                // A dedicated face already provides the style; don't double-apply synthetic traits.
                (family, false, false)
            } else {
                // No dedicated face: synthesize only if allowed for this style (else fall back to
                // the plain regular face with no faux-bold/italic).
                let allow = families.allows_synthesis(*bold, *italic);
                (family, *bold && allow, *italic && allow)
            }
        }
        _ => (Arc::clone(&terminal.family), *bold, *italic),
    };
    // `font-codepoint-map`: a codepoint-range override wins over the style family. Sprites are
    // hand-drawn and never remapped.
    if !*sprite
        && !terminal.font_codepoint_map.is_empty()
        && let Some(character) = text.chars().next()
        && let Some(entry) = terminal
            .font_codepoint_map
            .iter()
            .find(|entry| entry.contains(character as u32))
    {
        family = Arc::from(entry.family.as_str());
        // The override family provides its own glyphs; don't synthesize on top of it.
        synth_bold = false;
        synth_italic = false;
    }
    Some(GlyphKey {
        text: Arc::clone(text),
        family,
        font_size_bits: (terminal.font_size * scale).to_bits(),
        bold: synth_bold,
        italic: synth_italic,
        cell_width: (cell_pixel_size[0].max(1.) as usize).min(u16::MAX as usize) as u16,
        width: (pixel_size[0].max(1.) as usize).min(u16::MAX as usize) as u16,
        height: (pixel_size[1].max(1.) as usize).min(u16::MAX as usize) as u16,
        sprite: *sprite,
        // Baseline only shifts text glyphs; box thickness only affects hand-drawn sprites. Keep the
        // irrelevant modifier out of each key so the cache doesn't split on a no-op adjustment.
        baseline: if *sprite {
            None
        } else {
            terminal.baseline_adjustment.map(|m| m.prescale(scale))
        },
        box_thickness: if *sprite {
            terminal.box_thickness_adjustment.map(|m| m.prescale(scale))
        } else {
            None
        },
        // Icon-height only scales Nerd Font icon glyphs. Gate on the codepoint (not just `!sprite`)
        // so setting `adjust-icon-height` never splits the glyph cache for ordinary text — only
        // real icon cells carry the modifier.
        icon_height: if *sprite {
            None
        } else if text.chars().next().is_some_and(is_nerd_icon_codepoint) {
            terminal.icon_height_adjustment.map(|m| m.prescale(scale))
        } else {
            None
        },
        features: if *sprite {
            Arc::from(&[][..])
        } else {
            Arc::clone(&terminal.font_features)
        },
        glyph_id: if *sprite { None } else { *glyph_id },
        variations: if *sprite {
            Arc::from(&[][..])
        } else {
            Arc::clone(&terminal.font_variations)
        },
        thicken: if *sprite { None } else { terminal.font_thicken },
    })
}

fn snap_device_rect(origin: [f32; 2], rect: [f32; 4], scale: f32) -> ([f32; 2], [f32; 2]) {
    let left = (origin[0] + rect[0] * scale).round();
    let top = (origin[1] + rect[1] * scale).round();
    let right = (origin[0] + (rect[0] + rect[2]) * scale).round();
    let bottom = (origin[1] + (rect[1] + rect[3]) * scale).round();
    (
        [left, top],
        [(right - left).max(1.), (bottom - top).max(1.)],
    )
}

#[cfg(test)]
#[path = "metal_terminal/tests.rs"]
mod tests;
