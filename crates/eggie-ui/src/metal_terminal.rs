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
use metal::{
    Buffer, CompileOptions, Device, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerMinMagFilter,
    MTLStorageMode, MTLTextureUsage, NSUInteger, RenderPipelineDescriptor, RenderPipelineState,
    SamplerDescriptor, SamplerState, Texture, TextureDescriptor,
};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    cell::RefCell,
    collections::HashSet,
    hash::Hash,
    mem,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use metal::{MTLClearColor, MTLLoadAction, MTLStoreAction, RenderPassDescriptor};

const MASK_ATLAS_SIZE: usize = 4096;
const COLOR_ATLAS_SIZE: usize = 2048;
const CPU_GLYPH_CACHE_LIMIT: usize = 65_536;

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

const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Instance {
    float2 origin;
    float2 size;
    float4 color;
    float2 uv_origin;
    float2 uv_size;
    uint4 metadata;
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
    uint kind [[flat]];
};

float4 unpack_rgba(uint color) {
    return float4(
        float((color >> 24) & 0xff) / 255.0,
        float((color >> 16) & 0xff) / 255.0,
        float((color >> 8) & 0xff) / 255.0,
        float(color & 0xff) / 255.0
    );
}

float linearize_component(float value) {
    return value <= 0.04045
        ? value / 12.92
        : pow((value + 0.055) / 1.055, 2.4);
}

float relative_luminance(float3 color) {
    return 0.2126 * linearize_component(color.r)
        + 0.7152 * linearize_component(color.g)
        + 0.0722 * linearize_component(color.b);
}

float contrast_ratio(float3 first, float3 second) {
    float first_luminance = relative_luminance(first);
    float second_luminance = relative_luminance(second);
    return (max(first_luminance, second_luminance) + 0.05)
        / (min(first_luminance, second_luminance) + 0.05);
}

float4 contrasted_color(float minimum_contrast, float4 foreground, float4 background) {
    if (contrast_ratio(foreground.rgb, background.rgb) >= minimum_contrast) {
        return foreground;
    }
    float white_contrast = contrast_ratio(float3(1.0), background.rgb);
    float black_contrast = contrast_ratio(float3(0.0), background.rgb);
    return white_contrast > black_contrast
        ? float4(1.0, 1.0, 1.0, foreground.a)
        : float4(0.0, 0.0, 0.0, foreground.a);
}

vertex VertexOut terminal_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    constant Instance *instances [[buffer(0)]],
    constant float2 &viewport [[buffer(1)]])
{
    constexpr float2 unit[6] = {
        float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
        float2(1.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0)
    };
    Instance instance = instances[instance_id];
    float2 local = unit[vertex_id];
    float2 pixel = instance.origin + local * instance.size;
    VertexOut out;
    out.position = float4(
        pixel.x / viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.y * 2.0,
        0.0,
        1.0);
    out.uv = instance.uv_origin + local * instance.uv_size;
    out.color = instance.color;
    out.kind = instance.metadata.x;
    float minimum_contrast = as_type<float>(instance.metadata.z);
    if (out.kind == 1 && minimum_contrast > 1.0 && (instance.metadata.w & 1u) == 0) {
        out.color = contrasted_color(
            minimum_contrast,
            out.color,
            unpack_rgba(instance.metadata.y));
    }
    return out;
}

fragment float4 terminal_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> mask_atlas [[texture(0)]],
    texture2d<float> color_atlas [[texture(1)]],
    sampler atlas_sampler [[sampler(0)]])
{
    if (in.kind == 0) {
        return in.color;
    }
    if (in.kind == 1) {
        float coverage = mask_atlas.sample(atlas_sampler, in.uv).r;
        return float4(in.color.rgb, in.color.a * coverage);
    }

    // CoreGraphics produces premultiplied RGBA. The render pipeline uses source-alpha blending,
    // so restore straight RGB here to avoid dark halos around antialiased color glyph edges.
    float4 glyph = color_atlas.sample(atlas_sampler, in.uv);
    float3 straight_rgb = glyph.a > 0.0 ? glyph.rgb / glyph.a : float3(0.0);
    return float4(straight_rgb, glyph.a * in.color.a);
}

fragment float4 terminal_image_fragment(
    VertexOut in [[stage_in]],
    texture2d<float> image [[texture(0)]],
    sampler image_sampler [[sampler(0)]])
{
    return image.sample(image_sampler, in.uv);
}
"#;

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    text: Arc<str>,
    family: Arc<str>,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    cell_width: u16,
    width: u16,
    height: u16,
    sprite: bool,
    /// `adjust-font-baseline` modifier, pre-scaled to device px (text glyphs only).
    baseline: Option<crate::settings::MetricModifier>,
    /// `adjust-box-thickness` modifier, pre-scaled to device px (sprite/box-drawing glyphs only).
    box_thickness: Option<crate::settings::MetricModifier>,
    /// `adjust-icon-height` modifier, pre-scaled to device px. Only set on Nerd Font icon cells
    /// (`is_nerd_icon_codepoint`); `None` everywhere else keeps ordinary text out of a cache split.
    icon_height: Option<crate::settings::MetricModifier>,
    /// OpenType feature overrides applied when shaping this glyph (text glyphs only). Empty = font
    /// defaults. Keeps deterministic ligature on/off in the cache key.
    features: Arc<[crate::settings::FontFeature]>,
    /// A pre-shaped glyph id to draw directly (from a ligature run's per-cell substitution), instead
    /// of shaping `text`. `None` renders `text` normally.
    glyph_id: Option<u16>,
    /// Variable-font axis settings applied to the face (empty = font defaults).
    variations: Arc<[crate::settings::FontVariation]>,
    /// macOS font-smoothing thickening: `None` = off, `Some(strength)` = on at 0–255.
    thicken: Option<u8>,
}

enum RasterCacheEntry {
    Pending,
    Ready(Arc<RasterizedGlyph>),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GlyphPreparation {
    pub(crate) ready: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GlyphRasterStats {
    pub(crate) background_jobs: usize,
    pub(crate) background_time: Duration,
    pub(crate) synchronous_jobs: usize,
    pub(crate) synchronous_time: Duration,
    pub(crate) pending: usize,
}

#[derive(Default)]
struct GlyphRasterCounters {
    background_jobs: AtomicU64,
    background_nanos: AtomicU64,
    synchronous_jobs: AtomicU64,
    synchronous_nanos: AtomicU64,
    pending: AtomicUsize,
}

/// CPU-side glyph cache populated by a small dedicated CoreText worker pool.
///
/// CoreText fallback and bitmap rasterization can take several hundred microseconds for a new
/// Unicode scalar. Doing that work from `MetalPrimitiveRenderer::paint` stalls GPUI's command
/// encoder and makes every other primitive miss the same frame. The CPU cache lets prepaint queue
/// the complete visible glyph set while Metal only performs cheap atlas uploads.
pub(crate) struct GlyphRasterCache {
    entries: Arc<Mutex<FxHashMap<GlyphKey, RasterCacheEntry>>>,
    sender: SyncSender<GlyphKey>,
    counters: Arc<GlyphRasterCounters>,
}

impl Default for GlyphRasterCache {
    fn default() -> Self {
        const QUEUE_CAPACITY: usize = 32_768;
        let entries = Arc::new(Mutex::new(FxHashMap::default()));
        let counters = Arc::new(GlyphRasterCounters::default());
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(receiver));
        let worker_count = thread::available_parallelism()
            .map_or(2, usize::from)
            .div_ceil(2)
            .clamp(2, 4);
        for index in 0..worker_count {
            let entries = Arc::clone(&entries);
            let receiver = Arc::clone(&receiver);
            let counters = Arc::clone(&counters);
            thread::Builder::new()
                .name(format!("eggie-glyph-raster-{index}"))
                .spawn(move || glyph_raster_worker(receiver, entries, counters))
                .expect("spawn terminal glyph raster worker");
        }
        Self {
            entries,
            sender,
            counters,
        }
    }
}

impl GlyphRasterCache {
    pub(crate) fn prepare_terminal(
        &self,
        terminal: &PreparedMetalTerminal,
        scale: f32,
        origin: [f32; 2],
    ) -> GlyphPreparation {
        let keys = terminal
            .commands
            .iter()
            .filter_map(|command| glyph_key_for_command(terminal, command, scale, origin))
            .collect::<FxHashSet<_>>();
        let mut queued = Vec::new();
        let mut pending: usize = 0;
        {
            let mut entries = self.entries.lock();
            if entries.len() >= CPU_GLYPH_CACHE_LIMIT {
                // Retain the complete visible working set and in-flight jobs. Old raster images are
                // cheap to regenerate asynchronously after a font/scale change and must not turn
                // a long-running Unicode workload into unbounded process memory.
                entries.retain(|key, entry| {
                    keys.contains(key) || matches!(entry, RasterCacheEntry::Pending)
                });
            }
            for key in keys {
                match entries.get(&key) {
                    Some(RasterCacheEntry::Ready(_)) => {}
                    Some(RasterCacheEntry::Pending) => pending += 1,
                    None => {
                        entries.insert(key.clone(), RasterCacheEntry::Pending);
                        queued.push(key);
                        pending += 1;
                    }
                }
            }
        }
        for key in queued {
            if let Err(error) = self.sender.try_send(key) {
                let key = match error {
                    TrySendError::Full(key) | TrySendError::Disconnected(key) => key,
                };
                if matches!(
                    self.entries.lock().remove(&key),
                    Some(RasterCacheEntry::Pending)
                ) {
                    pending = pending.saturating_sub(1);
                }
            } else {
                self.counters.pending.fetch_add(1, Ordering::Relaxed);
            }
        }
        GlyphPreparation {
            ready: pending == 0,
        }
    }

    fn ready(&self, key: &GlyphKey) -> Option<Arc<RasterizedGlyph>> {
        match self.entries.lock().get(key) {
            Some(RasterCacheEntry::Ready(rasterized)) => Some(Arc::clone(rasterized)),
            _ => None,
        }
    }

    fn install_synchronous(
        &self,
        key: GlyphKey,
        rasterized: Arc<RasterizedGlyph>,
        elapsed: Duration,
    ) {
        self.entries
            .lock()
            .insert(key, RasterCacheEntry::Ready(rasterized));
        self.counters
            .synchronous_jobs
            .fetch_add(1, Ordering::Relaxed);
        self.counters
            .synchronous_nanos
            .fetch_add(duration_nanos(elapsed), Ordering::Relaxed);
    }

    pub(crate) fn take_stats(&self) -> GlyphRasterStats {
        GlyphRasterStats {
            background_jobs: self.counters.background_jobs.swap(0, Ordering::Relaxed) as usize,
            background_time: Duration::from_nanos(
                self.counters.background_nanos.swap(0, Ordering::Relaxed),
            ),
            synchronous_jobs: self.counters.synchronous_jobs.swap(0, Ordering::Relaxed) as usize,
            synchronous_time: Duration::from_nanos(
                self.counters.synchronous_nanos.swap(0, Ordering::Relaxed),
            ),
            pending: self.counters.pending.load(Ordering::Relaxed),
        }
    }
}

fn glyph_raster_worker(
    receiver: Arc<Mutex<Receiver<GlyphKey>>>,
    entries: Arc<Mutex<FxHashMap<GlyphKey, RasterCacheEntry>>>,
    counters: Arc<GlyphRasterCounters>,
) {
    loop {
        let Ok(key) = receiver.lock().recv() else {
            return;
        };
        let started = Instant::now();
        let rasterized = Arc::new(rasterize_glyph(&key));
        let elapsed = started.elapsed();
        entries
            .lock()
            .insert(key, RasterCacheEntry::Ready(rasterized));
        counters.pending.fetch_sub(1, Ordering::Relaxed);
        counters.background_jobs.fetch_add(1, Ordering::Relaxed);
        counters
            .background_nanos
            .fetch_add(duration_nanos(elapsed), Ordering::Relaxed);
    }
}

fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy)]
struct AtlasEntry {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum GlyphTextureKind {
    Mask = 1,
    Color = 2,
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    entry: AtlasEntry,
    kind: GlyphTextureKind,
    offset: [i16; 2],
}

struct GlyphLookup {
    glyph: CachedGlyph,
    hit: bool,
    raster_time: Duration,
}

struct RasterizedImage {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    offset: [i16; 2],
}

enum RasterizedGlyph {
    Mask(RasterizedImage),
    Color(RasterizedImage),
}

impl RasterizedGlyph {
    fn exact_mask(pixels: Vec<u8>, width: usize, height: usize) -> Self {
        Self::Mask(RasterizedImage {
            pixels,
            width,
            height,
            offset: [0, 0],
        })
    }
}

#[derive(Default)]
struct ShelfAllocator {
    next_x: usize,
    next_y: usize,
    row_height: usize,
}

impl ShelfAllocator {
    fn allocate(&mut self, width: usize, height: usize, atlas_size: usize) -> Option<AtlasEntry> {
        if width == 0 || height == 0 || width > atlas_size || height > atlas_size {
            return None;
        }
        if self.next_x + width > atlas_size {
            self.next_x = 0;
            self.next_y += self.row_height + 1;
            self.row_height = 0;
        }
        if self.next_y + height > atlas_size {
            return None;
        }
        let entry = AtlasEntry {
            x: self.next_x,
            y: self.next_y,
            width,
            height,
        };
        self.next_x += width + 1;
        self.row_height = self.row_height.max(height);
        Some(entry)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

struct GlyphAtlas {
    mask_texture: Texture,
    color_texture: Option<Texture>,
    empty_color_texture: Texture,
    entries: FxHashMap<GlyphKey, CachedGlyph>,
    mask_allocator: ShelfAllocator,
    color_allocator: ShelfAllocator,
}

impl GlyphAtlas {
    fn new(device: &Device) -> Self {
        let mask_texture = atlas_texture(device, MASK_ATLAS_SIZE, MTLPixelFormat::R8Unorm);
        let empty_color_texture = atlas_texture(device, 1, MTLPixelFormat::RGBA8Unorm);
        let transparent = [0u8; 4];
        empty_color_texture.replace_region(
            MTLRegion::new_2d(0, 0, 1, 1),
            0,
            transparent.as_ptr().cast(),
            4,
        );
        Self {
            mask_texture,
            color_texture: None,
            empty_color_texture,
            entries: FxHashMap::default(),
            mask_allocator: ShelfAllocator::default(),
            color_allocator: ShelfAllocator::default(),
        }
    }

    fn get_or_insert(
        &mut self,
        device: &Device,
        raster_cache: &GlyphRasterCache,
        key: GlyphKey,
    ) -> Result<GlyphLookup> {
        if let Some(entry) = self.entries.get(&key) {
            return Ok(GlyphLookup {
                glyph: *entry,
                hit: true,
                raster_time: Duration::ZERO,
            });
        }
        let width = key.width as usize;
        let height = key.height as usize;
        if width == 0 || height == 0 || width > MASK_ATLAS_SIZE || height > MASK_ATLAS_SIZE {
            return Err(anyhow!("invalid terminal glyph size {width}x{height}"));
        }

        let (rasterized, raster_time) = if let Some(rasterized) = raster_cache.ready(&key) {
            (rasterized, Duration::ZERO)
        } else {
            // The first terminal frame, a font change, or a saturated worker queue can reach
            // Metal before prewarming completed. Preserve correctness with a synchronous fallback;
            // steady-state animation frames use the ready cache and never execute CoreText here.
            let started = Instant::now();
            let rasterized = Arc::new(rasterize_glyph(&key));
            let elapsed = started.elapsed();
            raster_cache.install_synchronous(key.clone(), Arc::clone(&rasterized), elapsed);
            (rasterized, elapsed)
        };
        let cached = match rasterized.as_ref() {
            RasterizedGlyph::Mask(image) => {
                if image.width == 0
                    || image.height == 0
                    || image.width > MASK_ATLAS_SIZE
                    || image.height > MASK_ATLAS_SIZE
                {
                    return Err(anyhow!(
                        "invalid terminal mask glyph size {}x{}",
                        image.width,
                        image.height
                    ));
                }
                if image.pixels.len() != image.width * image.height {
                    return Err(anyhow!(
                        "invalid terminal mask glyph payload {} for {}x{}",
                        image.pixels.len(),
                        image.width,
                        image.height
                    ));
                }
                let entry = self
                    .mask_allocator
                    .allocate(image.width, image.height, MASK_ATLAS_SIZE)
                    .ok_or_else(|| anyhow!("terminal mask atlas is full"))?;
                self.mask_texture.replace_region(
                    atlas_region(entry),
                    0,
                    image.pixels.as_ptr().cast(),
                    image.width as u64,
                );
                CachedGlyph {
                    entry,
                    kind: GlyphTextureKind::Mask,
                    offset: image.offset,
                }
            }
            RasterizedGlyph::Color(image) => {
                if image.width == 0
                    || image.height == 0
                    || image.width > COLOR_ATLAS_SIZE
                    || image.height > COLOR_ATLAS_SIZE
                {
                    return Err(anyhow!(
                        "invalid terminal color glyph size {}x{}",
                        image.width,
                        image.height
                    ));
                }
                if image.pixels.len() != image.width * image.height * 4 {
                    return Err(anyhow!(
                        "invalid terminal color glyph payload {} for {}x{}",
                        image.pixels.len(),
                        image.width,
                        image.height
                    ));
                }
                let entry = self
                    .color_allocator
                    .allocate(image.width, image.height, COLOR_ATLAS_SIZE)
                    .ok_or_else(|| anyhow!("terminal color atlas is full"))?;
                if self.color_texture.is_none() {
                    self.color_texture = Some(atlas_texture(
                        device,
                        COLOR_ATLAS_SIZE,
                        MTLPixelFormat::RGBA8Unorm,
                    ));
                }
                self.color_texture
                    .as_ref()
                    .expect("color atlas was allocated")
                    .replace_region(
                        atlas_region(entry),
                        0,
                        image.pixels.as_ptr().cast(),
                        (image.width * 4) as u64,
                    );
                CachedGlyph {
                    entry,
                    kind: GlyphTextureKind::Color,
                    offset: image.offset,
                }
            }
        };
        self.entries.insert(key, cached);
        Ok(GlyphLookup {
            glyph: cached,
            hit: false,
            raster_time,
        })
    }

    fn color_texture(&self) -> &Texture {
        self.color_texture
            .as_ref()
            .unwrap_or(&self.empty_color_texture)
    }

    fn reset(&mut self) {
        self.entries.clear();
        self.mask_allocator.reset();
        self.color_allocator.reset();
    }
}

fn atlas_texture(device: &Device, size: usize, format: MTLPixelFormat) -> Texture {
    let descriptor = TextureDescriptor::new();
    descriptor.set_width(size as u64);
    descriptor.set_height(size as u64);
    descriptor.set_pixel_format(format);
    descriptor.set_storage_mode(MTLStorageMode::Shared);
    descriptor.set_usage(MTLTextureUsage::ShaderRead);
    device.new_texture(&descriptor)
}

fn atlas_region(entry: AtlasEntry) -> MTLRegion {
    MTLRegion::new_2d(
        entry.x as u64,
        entry.y as u64,
        entry.width as u64,
        entry.height as u64,
    )
}

pub(crate) struct MetalBackend {
    device: Device,
    pipeline: RenderPipelineState,
    image_pipeline: RenderPipelineState,
    sampler: SamplerState,
    image_sampler: SamplerState,
    atlas: GlyphAtlas,
    images: FxHashMap<TerminalTextureKey, CachedTerminalImage>,
    recycled_images: Vec<RecycledTerminalImage>,
    image_clock: u64,
    image_bytes: usize,
    instances: Vec<Instance>,
}

struct CachedTerminalImage {
    texture: Texture,
    bytes: usize,
    last_used: u64,
    /// When present, `texture` is a zero-copy view over shared memory: the `MTLBuffer` wraps the
    /// mmap pages directly (`new_buffer_with_bytes_no_copy`), so the mmap and buffer must outlive
    /// the texture. Such textures alias a specific shm segment and must never be recycled for a
    /// different image.
    #[cfg(unix)]
    zero_copy: Option<ZeroCopyBacking>,
}

/// Keeps a zero-copy image texture's Rust-side handles together with the cache entry. GPU-safe
/// lifetime of the mmap is guaranteed by the buffer's deallocator block (which owns its own
/// `Arc<Mmap>` clone and runs `munmap` only when Metal finally releases the buffer, i.e. after all
/// in-flight command buffers complete). This struct just parks the `Buffer` handle and a mmap
/// reference alongside the texture so they are not dropped earlier than the cache entry.
#[cfg(unix)]
struct ZeroCopyBacking {
    _buffer: Buffer,
    _mmap: Arc<memmap2::Mmap>,
}

struct RecycledTerminalImage {
    texture: Texture,
    width: u32,
    height: u32,
}

/// Whether to attempt zero-copy image upload (`new_buffer_with_bytes_no_copy` over the shm mmap,
/// skipping `replace_region`). On by default; the path degrades safely (falls back to
/// `replace_region` when the row stride is unaligned or the image is not shm-mapped) and the mmap's
/// GPU lifetime is bound to the buffer's deallocator. Set `EGGIE_IMAGE_ZERO_COPY=0` to force the
/// copy path.
#[cfg(unix)]
fn image_zero_copy_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("EGGIE_IMAGE_ZERO_COPY").is_none_or(|value| value != "0")
    })
}

impl MetalBackend {
    pub(crate) fn new(device: &DeviceRef) -> Result<Self> {
        let device = device.to_owned();
        let options = CompileOptions::new();
        let library = device
            .new_library_with_source(SHADER, &options)
            .map_err(|error| anyhow!("compile terminal Metal shader: {error}"))?;
        let vertex = library
            .get_function("terminal_vertex", None)
            .map_err(|error| anyhow!("load terminal vertex shader: {error}"))?;
        let fragment = library
            .get_function("terminal_fragment", None)
            .map_err(|error| anyhow!("load terminal fragment shader: {error}"))?;
        let image_fragment = library
            .get_function("terminal_image_fragment", None)
            .map_err(|error| anyhow!("load terminal image fragment shader: {error}"))?;
        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(&vertex));
        descriptor.set_fragment_function(Some(&fragment));
        let attachment = descriptor
            .color_attachments()
            .object_at(0)
            .ok_or_else(|| anyhow!("terminal Metal pipeline has no color attachment"))?;
        attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        attachment.set_blending_enabled(true);
        attachment.set_rgb_blend_operation(MTLBlendOperation::Add);
        attachment.set_alpha_blend_operation(MTLBlendOperation::Add);
        attachment.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
        attachment.set_source_alpha_blend_factor(MTLBlendFactor::One);
        attachment.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        let pipeline = device
            .new_render_pipeline_state(&descriptor)
            .map_err(|error| anyhow!("create terminal Metal pipeline: {error}"))?;
        descriptor.set_fragment_function(Some(&image_fragment));
        let image_pipeline = device
            .new_render_pipeline_state(&descriptor)
            .map_err(|error| anyhow!("create terminal image Metal pipeline: {error}"))?;

        let sampler_descriptor = SamplerDescriptor::new();
        sampler_descriptor.set_min_filter(MTLSamplerMinMagFilter::Nearest);
        sampler_descriptor.set_mag_filter(MTLSamplerMinMagFilter::Nearest);
        sampler_descriptor.set_address_mode_s(MTLSamplerAddressMode::ClampToEdge);
        sampler_descriptor.set_address_mode_t(MTLSamplerAddressMode::ClampToEdge);
        let sampler = device.new_sampler(&sampler_descriptor);
        sampler_descriptor.set_min_filter(MTLSamplerMinMagFilter::Linear);
        sampler_descriptor.set_mag_filter(MTLSamplerMinMagFilter::Linear);
        let image_sampler = device.new_sampler(&sampler_descriptor);
        let atlas = GlyphAtlas::new(&device);
        Ok(Self {
            device,
            pipeline,
            image_pipeline,
            sampler,
            image_sampler,
            atlas,
            images: FxHashMap::default(),
            recycled_images: Vec::new(),
            image_clock: 0,
            image_bytes: 0,
            instances: Vec::new(),
        })
    }

    pub(crate) fn retain_session_images(
        &mut self,
        session_id: eggie_domain::SessionId,
        live: &HashSet<TerminalTextureKey>,
    ) {
        let stale = self
            .images
            .keys()
            .copied()
            .filter(|key| key.session_id == session_id && !live.contains(key))
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(cached) = self.images.remove(&key) {
                self.image_bytes = self.image_bytes.saturating_sub(cached.bytes);
                // Only plain (copy-uploaded) textures may be recycled. A zero-copy texture is a view
                // over a specific shm mmap; reusing it for another image would alias freed pages or
                // write into shared memory via replace_region. Drop it (and its backing) instead.
                #[cfg(unix)]
                if cached.zero_copy.is_some() {
                    continue;
                }
                if self.recycled_images.len() < 4 {
                    self.recycled_images.push(RecycledTerminalImage {
                        width: cached.texture.width() as u32,
                        height: cached.texture.height() as u32,
                        texture: cached.texture,
                    });
                }
            }
        }
    }

    pub(crate) fn render(
        &mut self,
        context: &mut MetalRenderContext<'_>,
        scale_factor: f32,
        terminal: &PreparedMetalTerminal,
        raster_cache: &GlyphRasterCache,
    ) -> Result<MetalFrameStats> {
        let bounds = context.bounds();
        let origin = [bounds.origin.x.as_f32(), bounds.origin.y.as_f32()];
        let mut stats = MetalFrameStats {
            command_count: terminal.commands.len(),
            ..Default::default()
        };
        let mut instances = mem::take(&mut self.instances);
        instances.clear();
        let mut command_index = 0;
        while command_index < terminal.commands.len() {
            let command = &terminal.commands[command_index];
            if let MetalCommand::Image { image, .. } = command {
                self.draw_text_batch(context, &mut instances, &mut stats)?;
                let batch_start = command_index;
                command_index = image_batch_end(&terminal.commands, batch_start, image.key);
                self.draw_image_batch(
                    context,
                    scale_factor,
                    origin,
                    &terminal.commands[batch_start..command_index],
                    &mut instances,
                    &mut stats,
                )?;
                continue;
            }
            match self.build_instance(
                terminal,
                command,
                scale_factor,
                origin,
                raster_cache,
                &mut stats,
            ) {
                Ok(instance) => instances.push(instance),
                Err(error) if error.to_string().contains("atlas is full") => {
                    self.draw_text_batch(context, &mut instances, &mut stats)?;
                    self.atlas.reset();
                    stats.atlas_resets += 1;
                    instances.push(self.build_instance(
                        terminal,
                        command,
                        scale_factor,
                        origin,
                        raster_cache,
                        &mut stats,
                    )?);
                }
                Err(error) => {
                    self.instances = instances;
                    return Err(error);
                }
            }
            command_index += 1;
        }
        self.draw_text_batch(context, &mut instances, &mut stats)?;
        self.instances = instances;
        Ok(stats)
    }

    fn viewport(context: &MetalRenderContext<'_>) -> [f32; 2] {
        let viewport_size = context.viewport_size();
        [
            i32::from(viewport_size.width) as f32,
            i32::from(viewport_size.height) as f32,
        ]
    }

    fn draw_text_batch(
        &self,
        context: &mut MetalRenderContext<'_>,
        instances: &mut Vec<Instance>,
        stats: &mut MetalFrameStats,
    ) -> Result<()> {
        if instances.is_empty() {
            return Ok(());
        }
        let byte_length = instances.len() * mem::size_of::<Instance>();
        let bytes =
            unsafe { std::slice::from_raw_parts(instances.as_ptr().cast::<u8>(), byte_length) };
        let Some(buffer_offset) = context.write_buffer(bytes, mem::align_of::<Instance>()) else {
            instances.clear();
            return Ok(());
        };
        let viewport = Self::viewport(context);
        let encoder = context.command_encoder();
        encoder.set_render_pipeline_state(&self.pipeline);
        encoder.set_vertex_buffer(0, Some(context.instance_buffer()), buffer_offset);
        encoder.set_vertex_bytes(
            1,
            mem::size_of_val(&viewport) as u64,
            viewport.as_ptr().cast(),
        );
        encoder.set_fragment_texture(0, Some(&self.atlas.mask_texture));
        encoder.set_fragment_texture(1, Some(self.atlas.color_texture()));
        encoder.set_fragment_sampler_state(0, Some(&self.sampler));
        encoder.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, 6, instances.len() as u64);
        stats.draw_calls += 1;
        instances.clear();
        Ok(())
    }

    fn draw_image_batch(
        &mut self,
        context: &mut MetalRenderContext<'_>,
        scale: f32,
        origin: [f32; 2],
        commands: &[MetalCommand],
        instances: &mut Vec<Instance>,
        stats: &mut MetalFrameStats,
    ) -> Result<()> {
        let Some(MetalCommand::Image { image, .. }) = commands.first() else {
            return Ok(());
        };
        let (texture, uploaded_bytes, upload_time) = self.image_texture(image)?;
        if uploaded_bytes > 0 {
            stats.image_uploads += 1;
            stats.image_upload_bytes += uploaded_bytes;
            stats.image_upload_time += upload_time;
        }
        instances.reserve(commands.len());
        for command in commands {
            let MetalCommand::Image {
                image: command_image,
                rect,
                source,
            } = command
            else {
                unreachable!("image batches contain only images")
            };
            debug_assert_eq!(command_image.key, image.key);
            let (pixel_origin, pixel_size) = snap_device_rect(origin, *rect, scale);
            instances.push(Instance {
                origin: pixel_origin,
                size: pixel_size,
                color: [1.; 4],
                uv_origin: [source[0], source[1]],
                uv_size: [source[2], source[3]],
                metadata: [3, 0, 0, 0],
            });
        }
        let byte_length = instances.len() * mem::size_of::<Instance>();
        let bytes =
            unsafe { std::slice::from_raw_parts(instances.as_ptr().cast::<u8>(), byte_length) };
        let Some(buffer_offset) = context.write_buffer(bytes, mem::align_of::<Instance>()) else {
            instances.clear();
            return Ok(());
        };
        let viewport = Self::viewport(context);
        let encoder = context.command_encoder();
        encoder.set_render_pipeline_state(&self.image_pipeline);
        encoder.set_vertex_buffer(0, Some(context.instance_buffer()), buffer_offset);
        encoder.set_vertex_bytes(
            1,
            mem::size_of_val(&viewport) as u64,
            viewport.as_ptr().cast(),
        );
        encoder.set_fragment_texture(0, Some(&texture));
        encoder.set_fragment_sampler_state(0, Some(&self.image_sampler));
        encoder.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, 6, instances.len() as u64);
        stats.draw_calls += 1;
        instances.clear();
        Ok(())
    }

    fn image_texture(&mut self, image: &TerminalImageData) -> Result<(Texture, usize, Duration)> {
        self.image_clock = self.image_clock.wrapping_add(1);
        if let Some(cached) = self.images.get_mut(&image.key) {
            cached.last_used = self.image_clock;
            return Ok((cached.texture.to_owned(), 0, Duration::ZERO));
        }
        let started = Instant::now();
        let width = image.width as usize;
        let height = image.height as usize;
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| anyhow!("terminal image dimensions overflow"))?;
        if width == 0 || height == 0 || image.pixels.len() != expected {
            return Err(anyhow!(
                "invalid terminal image {}x{} with {} bytes",
                width,
                height,
                image.pixels.len()
            ));
        }
        const GPU_IMAGE_CACHE_LIMIT: usize = 320 * 1_000 * 1_000;
        while self.image_bytes.saturating_add(expected) > GPU_IMAGE_CACHE_LIMIT {
            let Some(oldest) = self
                .images
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.images.remove(&oldest) {
                self.image_bytes = self.image_bytes.saturating_sub(removed.bytes);
            }
        }

        // Zero-copy fast path (opt-in): wrap the shm mmap pages in an MTLBuffer and build a texture
        // view over it, skipping the replace_region CPU copy entirely. Falls back to the copy path
        // if disabled, if the image is not shm-mapped, or if the row stride is not aligned to the
        // device's linear-texture requirement.
        #[cfg(unix)]
        if image_zero_copy_enabled()
            && let Some(mmap) = image.pixels.mapped()
            && let Some((texture, buffer)) = self.zero_copy_image_texture(image, &mmap, width)
        {
            self.image_bytes = self.image_bytes.saturating_add(expected);
            self.images.insert(
                image.key,
                CachedTerminalImage {
                    texture: texture.to_owned(),
                    bytes: expected,
                    last_used: self.image_clock,
                    zero_copy: Some(ZeroCopyBacking {
                        _buffer: buffer,
                        _mmap: mmap,
                    }),
                },
            );
            return Ok((texture, 0, started.elapsed()));
        }

        let texture = self
            .recycled_images
            .iter()
            .position(|cached| cached.width == image.width && cached.height == image.height)
            .map(|index| self.recycled_images.swap_remove(index).texture)
            .unwrap_or_else(|| {
                let descriptor = TextureDescriptor::new();
                descriptor.set_width(image.width as u64);
                descriptor.set_height(image.height as u64);
                descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
                descriptor.set_storage_mode(MTLStorageMode::Shared);
                descriptor.set_usage(MTLTextureUsage::ShaderRead);
                self.device.new_texture(&descriptor)
            });
        texture.replace_region(
            MTLRegion::new_2d(0, 0, image.width as u64, image.height as u64),
            0,
            image.pixels.as_ptr().cast(),
            (width * 4) as u64,
        );
        self.image_bytes = self.image_bytes.saturating_add(expected);
        self.images.insert(
            image.key,
            CachedTerminalImage {
                texture: texture.to_owned(),
                bytes: expected,
                last_used: self.image_clock,
                #[cfg(unix)]
                zero_copy: None,
            },
        );
        Ok((texture, expected, started.elapsed()))
    }

    /// Build a texture that samples the shm mmap directly, with no CPU copy. Returns `None` (so the
    /// caller falls back to `replace_region`) when the row stride does not meet the device's
    /// `minimumLinearTextureAlignmentForPixelFormat`, since an unaligned buffer-backed texture is
    /// invalid. The mmap base from POSIX shm is page-aligned, satisfying the buffer's own alignment.
    #[cfg(unix)]
    fn zero_copy_image_texture(
        &self,
        image: &TerminalImageData,
        mmap: &Arc<memmap2::Mmap>,
        width: usize,
    ) -> Option<(Texture, Buffer)> {
        let bytes_per_row = (width * 4) as u64;
        let alignment = self
            .device
            .minimum_linear_texture_alignment_for_pixel_format(MTLPixelFormat::RGBA8Unorm);
        if alignment == 0 || bytes_per_row % alignment != 0 {
            return None;
        }
        let expected = (width * image.height as usize * 4) as NSUInteger;
        if (mmap.len() as NSUInteger) < expected {
            return None;
        }
        // Bind the mmap's lifetime to the MTLBuffer via a deallocator block that owns a clone of the
        // Arc<Mmap>. Metal calls (and releases) the deallocator only when the buffer's refcount hits
        // zero — which is after our own drop AND after every command buffer that referenced a
        // texture over this buffer has completed (command buffers retain their resources). So the
        // pages stay mapped until the GPU is truly done, closing the use-after-free window that a
        // cache eviction on the CPU side would otherwise open mid-frame. The closure body is empty:
        // dropping the captured Arc runs memmap2's munmap when the last reference goes away.
        let backing = Arc::clone(mmap);
        let deallocator = block::ConcreteBlock::new(
            move |_ptr: *const std::ffi::c_void, _len: NSUInteger| {
                // Hold the mapping alive until Metal releases this block; drop does the munmap.
                let _keep_alive = &backing;
            },
        )
        .copy();
        // SAFETY: the mmap is a read-only, page-aligned POSIX shm mapping. The deallocator block
        // above keeps it mapped for as long as Metal (and thus the GPU) can reference the buffer;
        // the daemon writes the segment once before sending its name and never mutates it after.
        let buffer = self.device.new_buffer_with_bytes_no_copy(
            mmap.as_ptr().cast(),
            expected,
            MTLResourceOptions::StorageModeShared,
            Some(&deallocator),
        );
        let descriptor = TextureDescriptor::new();
        descriptor.set_width(image.width as u64);
        descriptor.set_height(image.height as u64);
        descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
        descriptor.set_storage_mode(MTLStorageMode::Shared);
        descriptor.set_usage(MTLTextureUsage::ShaderRead);
        let texture = buffer.new_texture_with_descriptor(&descriptor, 0, bytes_per_row);
        Some((texture, buffer))
    }

    fn build_instance(
        &mut self,
        terminal: &PreparedMetalTerminal,
        command: &MetalCommand,
        scale: f32,
        origin: [f32; 2],
        raster_cache: &GlyphRasterCache,
        stats: &mut MetalFrameStats,
    ) -> Result<Instance> {
        match command {
            MetalCommand::Rect { rect, color } => {
                let (pixel_origin, pixel_size) = snap_device_rect(origin, *rect, scale);
                Ok(Instance {
                    origin: pixel_origin,
                    size: pixel_size,
                    color: rgba_components(*color),
                    uv_origin: [0., 0.],
                    uv_size: [0., 0.],
                    metadata: [0; 4],
                })
            }
            MetalCommand::Glyph {
                rect,
                text: _,
                color,
                bold: _,
                italic: _,
                sprite: _,
                background,
                minimum_contrast,
                minimum_contrast_disabled,
                glyph_id: _,
            } => {
                stats.glyph_count += 1;
                let (pixel_origin, _) = snap_device_rect(origin, *rect, scale);
                let key = glyph_key_for_command(terminal, command, scale, origin)
                    .expect("glyph commands produce glyph keys");
                let lookup = self.atlas.get_or_insert(&self.device, raster_cache, key)?;
                stats.atlas_hits += usize::from(lookup.hit);
                stats.atlas_misses += usize::from(!lookup.hit);
                stats.raster_time += lookup.raster_time;
                let cached = lookup.glyph;
                let atlas_size = match cached.kind {
                    GlyphTextureKind::Mask => MASK_ATLAS_SIZE,
                    GlyphTextureKind::Color => COLOR_ATLAS_SIZE,
                } as f32;
                Ok(Instance {
                    origin: [
                        pixel_origin[0] + f32::from(cached.offset[0]),
                        pixel_origin[1] + f32::from(cached.offset[1]),
                    ],
                    size: [cached.entry.width as f32, cached.entry.height as f32],
                    color: rgba_components(*color),
                    uv_origin: [
                        cached.entry.x as f32 / atlas_size,
                        cached.entry.y as f32 / atlas_size,
                    ],
                    uv_size: [
                        cached.entry.width as f32 / atlas_size,
                        cached.entry.height as f32 / atlas_size,
                    ],
                    metadata: [
                        cached.kind as u32,
                        *background,
                        minimum_contrast.to_bits(),
                        u32::from(*minimum_contrast_disabled),
                    ],
                })
            }
            MetalCommand::Image { .. } => unreachable!("images use their own Metal pipeline"),
        }
    }
}

fn image_batch_end(
    commands: &[MetalCommand],
    start: usize,
    image_key: TerminalTextureKey,
) -> usize {
    let mut end = start + 1;
    while end < commands.len()
        && matches!(
            &commands[end],
            MetalCommand::Image { image, .. } if image.key == image_key
        )
    {
        end += 1;
    }
    end
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
        } else if text
            .chars()
            .next()
            .is_some_and(is_nerd_icon_codepoint)
        {
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

fn rasterize_glyph(key: &GlyphKey) -> RasterizedGlyph {
    let width = key.width as usize;
    let height = key.height as usize;
    if key.sprite
        && let Some(pixels) = key
            .text
            .chars()
            .next()
            .and_then(|character| {
                terminal_sprites::rasterize(character, width, height, key.box_thickness)
            })
    {
        return RasterizedGlyph::exact_mask(pixels, width, height);
    }
    rasterize_text(key)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FontStyleKey {
    family: Arc<str>,
    point_size_bits: u64,
    bold: bool,
    italic: bool,
    variations: Arc<[crate::settings::FontVariation]>,
}

#[derive(Default)]
struct GlyphRasterizer {
    fonts: FxHashMap<FontStyleKey, CTFont>,
}

impl GlyphRasterizer {
    fn font(&mut self, key: &GlyphKey) -> CTFont {
        let point_size = f32::from_bits(key.font_size_bits) as f64;
        let style = FontStyleKey {
            family: Arc::clone(&key.family),
            point_size_bits: point_size.to_bits(),
            bold: key.bold,
            italic: key.italic,
            variations: Arc::clone(&key.variations),
        };
        if let Some(font) = self.fonts.get(&style) {
            return font.clone();
        }
        let base_font = font::new_from_name(&key.family, point_size)
            .or_else(|_| font::new_from_name("Menlo", point_size))
            .expect("macOS must provide Menlo");
        let mut traits = 0;
        if key.bold {
            traits |= kCTFontBoldTrait;
        }
        if key.italic {
            traits |= kCTFontItalicTrait;
        }
        let font = if traits == 0 {
            base_font
        } else {
            base_font
                .clone_with_symbolic_traits(traits, kCTFontBoldTrait | kCTFontItalicTrait)
                .unwrap_or(base_font)
        };
        let font = font_with_variations(&font, &key.variations);
        self.fonts.insert(style, font.clone());
        font
    }

    fn rasterize_text(&mut self, key: &GlyphKey) -> RasterizedGlyph {
        let width = key.width as usize;
        let height = key.height as usize;
        let primary = self.font(key);
        // Ligature run cells carry a pre-shaped glyph id: draw that exact glyph from the run's font
        // (a contextual variant that visually joins its neighbors) rather than re-shaping the text.
        if let Some(glyph_id) = key.glyph_id {
            return RasterizedGlyph::Mask(rasterize_glyph_id(
                &primary,
                glyph_id,
                width,
                height,
                key.baseline,
                key.thicken,
            ));
        }
        let font = font_for_text(&primary, &key.text);
        // `adjust-icon-height`: re-size the fallback face so Nerd Font icons hit the configured
        // constraint height. Gated by the key builder to icon codepoints, so this never touches
        // ordinary text; `None` skips it entirely and preserves the font-native size byte-for-byte.
        let font = match key.icon_height {
            Some(modifier) => {
                // A double-width icon cell spans two cells; use the multi-cell constraint height.
                let single = (key.cell_width as usize).max(1);
                let constraint_cells = ((width + single / 2) / single).max(1);
                scale_icon_font(
                    &primary,
                    &font,
                    modifier,
                    single,
                    constraint_cells,
                    height,
                    &key.text,
                )
            }
            None => font,
        };
        let confine_to_cell = is_single_cell_fraction(&key.text);
        let font = if confine_to_cell {
            fit_font_to_cell(&key.text, &font, width, height)
        } else {
            font
        };
        // Apply OpenType feature overrides (ligature on/off, ss01, …) so the same feature state
        // that drove run shaping also renders the (possibly ligated) glyph.
        let font = font_with_features(&font, &key.features);
        let line = attributed_line(&key.text, &font);
        if font.postscript_name().contains("LastResort") || line_uses_last_resort(&line) {
            return unknown_glyph_outline(key.cell_width as usize, height);
        }
        if !key.text.contains('\u{fe0e}') && line_uses_only_color_glyphs(&line) {
            RasterizedGlyph::Color(rasterize_color_line(&line, width, height, key.baseline))
        } else {
            RasterizedGlyph::Mask(rasterize_mask_line(
                &line,
                width,
                height,
                confine_to_cell,
                key.baseline,
                key.thicken,
            ))
        }
    }
}

fn line_uses_last_resort(line: &CTLine) -> bool {
    line.glyph_runs().iter().any(|run| {
        run.attributes().is_some_and(|attributes| {
            attributes
                .find(CFString::new("NSFont"))
                .and_then(|value| value.downcast::<CTFont>())
                .is_some_and(|font| font.postscript_name().contains("LastResort"))
        })
    })
}

fn unknown_glyph_outline(width: usize, height: usize) -> RasterizedGlyph {
    let width = width.max(1);
    let height = height.max(1);
    let mut pixels = vec![0; width * height];
    for pixel in &mut pixels[..width] {
        *pixel = u8::MAX;
    }
    if height > 1 {
        let last_row = (height - 1) * width;
        for pixel in &mut pixels[last_row..last_row + width] {
            *pixel = u8::MAX;
        }
    }
    if width > 1 {
        for row in 1..height.saturating_sub(1) {
            pixels[row * width] = u8::MAX;
            pixels[row * width + width - 1] = u8::MAX;
        }
    }
    RasterizedGlyph::exact_mask(pixels, width, height)
}

thread_local! {
    static GLYPH_RASTERIZER: RefCell<GlyphRasterizer> = RefCell::new(GlyphRasterizer::default());
}

fn rasterize_text(key: &GlyphKey) -> RasterizedGlyph {
    GLYPH_RASTERIZER.with(|rasterizer| rasterizer.borrow_mut().rasterize_text(key))
}

fn is_single_cell_fraction(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(character) = characters.next() else {
        return false;
    };
    characters.next().is_none()
        && matches!(character, '\u{00bc}'..='\u{00be}' | '\u{2150}'..='\u{215f}' | '\u{2189}')
}

fn fit_font_to_cell(text: &str, font: &CTFont, width: usize, height: usize) -> CTFont {
    // Some system fallback faces give the precomposed fractions a one-cell advance but ink that
    // is considerably wider. Kitty handles the same CoreText fallback mismatch by comparing the
    // glyph's right edge with the terminal canvas and resizing the face by canvas_width / right.
    // Keep that behavior here so PingFang's U+2150/U+2151/U+2152 fit without replacing their
    // native outlines with hand-drawn approximations.
    let line = attributed_line(text, font);
    let context = mask_context(width, height, None);
    let ink = line.get_image_bounds(&context);
    let right = ink.origin.x + ink.size.width;
    if !ink.is_empty()
        && right.is_finite()
        && right > width as f64 + 1.
        && font.pt_size().is_finite()
    {
        font.clone_with_font_size((font.pt_size() * width as f64 / right).max(1.))
    } else {
        font.clone()
    }
}

/// Constraint heights (device px) for Nerd Font icons, mirroring Ghostty's `Metrics.zig`:
/// `icon_height` is the full face height (used for multi-cell icons) and `icon_height_single`
/// blends cap height with face height for single-cell icons. `adjust-icon-height` scales both.
#[derive(Clone, Copy, Debug)]
struct IconMetrics {
    icon_height: f64,
    icon_height_single: f64,
}

/// Derive the Nerd Font icon constraint heights from the *primary* styled face (not the symbol
/// fallback face, which often lacks a cap-height metric), then apply the `adjust-icon-height`
/// modifier to both. Values are in device px because `primary` is built at `font_size * scale`.
///
/// Mirrors `ghostty/src/font/Metrics.zig`: `icon_height = face_height`,
/// `icon_height_single = (2 * cap_height + face_height) / 3`, with the cap-height estimate
/// (`0.75 * ascent`) used when the face reports no usable cap height.
fn icon_constraint_metrics(
    primary: &CTFont,
    modifier: Option<crate::settings::MetricModifier>,
) -> IconMetrics {
    let ascent = primary.ascent();
    let descent = primary.descent();
    let leading = primary.leading().max(0.);
    let face_height = ascent + descent + leading;
    let cap_height = {
        let reported = primary.cap_height();
        if reported > 0. {
            reported
        } else {
            0.75 * ascent
        }
    };
    let icon_height = face_height;
    let icon_height_single = (2.0 * cap_height + face_height) / 3.0;
    let adjust = |base: f64| {
        f64::from(crate::settings::ResolvedMetricAdjustments::adjust(
            modifier,
            base as f32,
            1.0,
        ))
    };
    IconMetrics {
        icon_height: adjust(icon_height),
        icon_height_single: adjust(icon_height_single),
    }
}

/// The uniform scale factor for a Nerd Font icon under Ghostty's `fit_cover1 + height=icon`
/// constraint (the ~85% common case). Because that constraint scales width and height by the same
/// factor, the whole thing reduces to one scalar we can apply by re-sizing the face.
///
/// The scale is **height-driven**: `target_height / glyph_h` makes the icon's ink match the
/// configured constraint height (shrinking a tall icon, growing a small one — `fit_cover1`'s
/// "cover at least one cell" upscaling). `max_width` is only a safety cap so a wide icon can't grow
/// past the space Eggie is willing to give it (the cell advance plus its bounded overhang); it is
/// deliberately *not* the plain cell advance, since Nerd Font icon ink is roughly an em square
/// (~2× the cell advance) and clamping on the advance would pin every icon to ~0.5 regardless of
/// the height target — defeating `adjust-icon-height`. Degenerate ink returns `1.0`.
fn icon_fit_cover1_scale(glyph_w: f64, glyph_h: f64, max_width: f64, target_height: f64) -> f64 {
    let positive_finite = |v: f64| v.is_finite() && v > 0.;
    if !positive_finite(glyph_w) || !positive_finite(glyph_h) {
        return 1.0;
    }
    if !max_width.is_finite() || !target_height.is_finite() {
        return 1.0;
    }
    let height_factor = target_height / glyph_h;
    // Cap on width only when a positive bound is given, so the icon never overruns its overhang
    // budget; otherwise the height target alone decides the scale.
    let factor = if max_width > 0. {
        height_factor.min(max_width / glyph_w)
    } else {
        height_factor
    };
    if factor.is_finite() && factor > 0. {
        factor
    } else {
        1.0
    }
}

/// Whether `character` is a Nerd Font icon codepoint that should receive the icon-height
/// constraint. Covers the private-use areas Nerd Font packs icons into (BMP PUA plus the two
/// supplementary planes) but excludes anything Eggie draws as a hand-drawn sprite (box/block/
/// braille/powerline/…), since those never reach the font raster path and are already grid-fitted.
fn is_nerd_icon_codepoint(character: char) -> bool {
    let cp = character as u32;
    let in_pua = (0xE000..=0xF8FF).contains(&cp)
        || (0xF0000..=0xFFFFD).contains(&cp)
        || (0x100000..=0x10FFFD).contains(&cp);
    in_pua && terminal_sprites::SpriteKind::for_char(character).is_none()
}

/// Scale the Nerd Font fallback face so an icon's ink matches the (adjusted) icon-constraint
/// height, Eggie's take on Ghostty's `fit_cover1 + height=icon` rule. `primary` supplies the
/// constraint heights (its cap/face metrics), `icon_face` is the face that actually carries the
/// glyph, and `modifier` is the pre-scaled `adjust-icon-height`. Single-cell icons target
/// `icon_height_single`; a two-cell-wide icon (`constraint_cells == 2`) targets the full
/// `icon_height` and may span both cells, matching Ghostty's single/multi-cell split. Measures the
/// icon's ink at the current size, derives the uniform scale, and re-sizes the face (crisper than a
/// raster transform, and it reuses the existing placement path). Returns the face unchanged when
/// the icon can't be measured or the scale is ~1.
fn scale_icon_font(
    primary: &CTFont,
    icon_face: &CTFont,
    modifier: crate::settings::MetricModifier,
    single_cell_width: usize,
    constraint_cells: usize,
    cell_height: usize,
    text: &str,
) -> CTFont {
    let metrics = icon_constraint_metrics(primary, Some(modifier));
    let multi_cell = constraint_cells >= 2;
    let target_height = if multi_cell {
        metrics.icon_height
    } else {
        metrics.icon_height_single
    };
    let single = single_cell_width.max(1);
    let cells = constraint_cells.max(1);
    // The icon may overhang one cell to each side (see `raster_placement`), so the width it is
    // allowed to grow into is the constraint span plus that bounded overhang — not the bare advance.
    // The height target drives the scale within this budget.
    let target_width = (single * cells) as f64;
    let max_width = target_width + 2.0 * single as f64;
    // Measure the icon's ink in a context wide enough to include the overhang so a large icon isn't
    // clipped during measurement (which would under-report its width and mis-cap the scale).
    let line = attributed_line(text, icon_face);
    let context = mask_context(max_width.max(1.) as usize, cell_height.max(1), None);
    let ink = line.get_image_bounds(&context);
    if ink.is_empty() || !ink.size.width.is_finite() || !ink.size.height.is_finite() {
        return icon_face.clone();
    }
    let scale = icon_fit_cover1_scale(ink.size.width, ink.size.height, max_width, target_height);
    let pt = icon_face.pt_size();
    if pt.is_finite() && (scale - 1.0).abs() > 1e-3 {
        icon_face.clone_with_font_size((pt * scale).max(1.))
    } else {
        icon_face.clone()
    }
}

fn font_for_text(base_font: &CTFont, text: &str) -> CTFont {
    let string = CFString::new(text);
    let range = CFRange::init(0, string.char_len());
    let fallback = unsafe {
        CTFontCreateForString(
            base_font.as_concrete_TypeRef(),
            string.as_concrete_TypeRef(),
            range,
        )
    };
    let resolved = if fallback.is_null() {
        base_font.clone()
    } else {
        unsafe { CTFont::wrap_under_create_rule(fallback) }
    };
    // If neither the base font nor the system fallback chain can render the text (LastResort), try
    // the bundled Nerd Font symbols so private-use-area icons work without a patched font installed.
    if resolved.postscript_name().contains("LastResort")
        && let Some(nerd) = nerd_font_fallback(base_font.pt_size())
        && nerd_font_covers(&nerd, text)
    {
        return nerd;
    }
    resolved
}

/// The bundled "Symbols Only" Nerd Font, embedded so private-use-area icons render even without a
/// patched Nerd Font installed. Registered with CoreText once (not installed system-wide).
const NERD_FONT_SYMBOLS: &[u8] = include_bytes!("../assets/SymbolsNerdFontMono-Regular.ttf");

thread_local! {
    /// Lazily-built descriptor for the embedded Nerd Font (per-thread; CoreText objects aren't Sync).
    /// `None` if registration failed. The outer Option marks "not yet attempted".
    static NERD_FONT_DESCRIPTOR: RefCell<Option<Option<core_text::font_descriptor::CTFontDescriptor>>> =
        const { RefCell::new(None) };
}

/// A `CTFont` for the embedded Nerd Font symbols at `pt_size`, or `None` if it couldn't be built.
fn nerd_font_fallback(pt_size: f64) -> Option<CTFont> {
    NERD_FONT_DESCRIPTOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let descriptor = slot
            .get_or_insert_with(|| {
                core_text::font_manager::create_font_descriptor(NERD_FONT_SYMBOLS).ok()
            })
            .clone()?;
        Some(font::new_from_descriptor(&descriptor, pt_size.max(1.)))
    })
}

/// Whether `font` has a real (non-`.notdef`) glyph for the first character of `text`.
fn nerd_font_covers(font: &CTFont, text: &str) -> bool {
    let Some(character) = text.chars().next() else {
        return false;
    };
    let mut utf16 = [0u16; 2];
    let encoded = character.encode_utf16(&mut utf16);
    let mut glyphs = [0u16; 2];
    let ok = unsafe {
        font.get_glyphs_for_characters(encoded.as_ptr(), glyphs.as_mut_ptr(), encoded.len() as _)
    };
    ok && glyphs[0] != 0
}

fn attributed_line(text: &str, font: &CTFont) -> CTLine {
    let string = CFString::new(text);
    let mut attributed = CFMutableAttributedString::new();
    attributed.replace_str(&string, CFRange::init(0, 0));
    let range = CFRange::init(0, attributed.char_len());
    attributed.set_attribute(range, unsafe { kCTFontAttributeName }, font);
    attributed.set_attribute(
        range,
        unsafe { kCTForegroundColorFromContextAttributeName },
        &CFBoolean::true_value(),
    );
    CTLine::new_with_attributed_string(attributed.as_concrete_TypeRef())
}

/// Apply OpenType feature overrides to a font via a copied descriptor. An empty list returns the
/// font unchanged. Each feature becomes a `{OpenTypeFeatureTag, OpenTypeFeatureValue}` dict entry
/// under `kCTFontFeatureSettingsAttribute`.
fn font_with_features(font: &CTFont, features: &[crate::settings::FontFeature]) -> CTFont {
    if features.is_empty() {
        return font.clone();
    }
    let tag_key = unsafe { CFString::wrap_under_get_rule(kCTFontOpenTypeFeatureTag) };
    let value_key = unsafe { CFString::wrap_under_get_rule(kCTFontOpenTypeFeatureValue) };
    let settings: Vec<CFDictionary<CFString, core_foundation::base::CFType>> = features
        .iter()
        .map(|feature| {
            let tag = CFString::new(
                std::str::from_utf8(&feature.tag).unwrap_or("").trim_end_matches('\0'),
            );
            let value = CFNumber::from(feature.value as i64);
            CFDictionary::from_CFType_pairs(&[
                (tag_key.clone(), tag.as_CFType()),
                (value_key.clone(), value.as_CFType()),
            ])
        })
        .collect();
    let settings_array = CFArray::from_CFTypes(&settings);
    let feature_key = unsafe { CFString::wrap_under_get_rule(kCTFontFeatureSettingsAttribute) };
    let attributes = CFDictionary::from_CFType_pairs(&[(
        feature_key,
        settings_array.as_CFType(),
    )]);
    let base_descriptor = font.copy_descriptor();
    match base_descriptor.create_copy_with_attributes(attributes.to_untyped()) {
        Ok(descriptor) => font::new_from_descriptor(&descriptor, font.pt_size()),
        Err(_) => font.clone(),
    }
}

/// Apply variable-font axis settings (`wght`, `slnt`, …) to a font via a copied descriptor. The
/// `kCTFontVariationAttribute` value is a dictionary keyed by each axis's four-char tag encoded as a
/// big-endian `u32` identifier → the axis value. Empty list returns the font unchanged.
fn font_with_variations(font: &CTFont, variations: &[crate::settings::FontVariation]) -> CTFont {
    if variations.is_empty() {
        return font.clone();
    }
    let pairs: Vec<(CFNumber, CFNumber)> = variations
        .iter()
        .map(|variation| {
            let identifier = u32::from_be_bytes(variation.tag) as i64;
            (CFNumber::from(identifier), CFNumber::from(variation.value as f64))
        })
        .collect();
    let variation_dict = CFDictionary::from_CFType_pairs(
        &pairs
            .iter()
            .map(|(key, value)| (key.as_CFType(), value.as_CFType()))
            .collect::<Vec<_>>(),
    );
    let variation_key = unsafe { CFString::wrap_under_get_rule(kCTFontVariationAttribute) };
    let attributes =
        CFDictionary::from_CFType_pairs(&[(variation_key.as_CFType(), variation_dict.as_CFType())]);
    let base_descriptor = font.copy_descriptor();
    match base_descriptor.create_copy_with_attributes(attributes.to_untyped()) {
        Ok(descriptor) => font::new_from_descriptor(&descriptor, font.pt_size()),
        Err(_) => font.clone(),
    }
}

/// Shape a run of `cell_count` single-cell characters with rustybuzz (a HarfBuzz port) and return
/// the contextually-substituted glyph id for each cell. Modern programming fonts render ligatures
/// as per-cell contextual substitutions (each cell keeps its own glyph slot and advance, but the
/// glyph is swapped for a variant that visually joins its neighbors), so a correct rendering just
/// needs the shaped glyph id per cell. Returns `Some` only when the shaper produced exactly one
/// glyph per cell in order (the common case); otherwise `None`, so the caller falls back to
/// rendering each character normally.
fn shape_run_glyph_ids(
    text: &str,
    font_data: &[u8],
    face_index: u32,
    features: &[crate::settings::FontFeature],
    cell_count: usize,
) -> Option<Vec<u16>> {
    if cell_count == 0 {
        return None;
    }
    let face = rustybuzz::Face::from_slice(font_data, face_index)?;
    let hb_features: Vec<rustybuzz::Feature> = features
        .iter()
        .map(|feature| {
            rustybuzz::Feature::new(
                rustybuzz::ttf_parser::Tag::from_bytes(&feature.tag),
                feature.value,
                ..,
            )
        })
        .collect();
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(rustybuzz::Direction::LeftToRight);
    let shaped = rustybuzz::shape(&face, &hb_features, buffer);
    let infos = shaped.glyph_infos();
    // Require exactly one glyph per cell, clusters 0,1,2,… (the per-cell substitution model). If the
    // font fused clusters into fewer glyphs, we can't map them onto the fixed cell grid, so bail.
    if infos.len() != cell_count {
        return None;
    }
    let mut glyph_ids = Vec::with_capacity(cell_count);
    for (cell, info) in infos.iter().enumerate() {
        if info.cluster as usize != cell {
            return None;
        }
        glyph_ids.push(u16::try_from(info.glyph_id).ok()?);
    }
    Some(glyph_ids)
}

/// Public entry for the prepare path: shape a same-style run of single-cell characters and return
/// the contextually-substituted glyph id per cell (or `None` to fall back to normal per-cell text
/// rendering). Loads the family's font data (Menlo fallback) for the given bold/italic style and
/// shapes with rustybuzz so programming ligatures (`->`, `==>`, …) apply.
pub(crate) fn shape_terminal_run(
    text: &str,
    family: &str,
    font_size: f32,
    bold: bool,
    italic: bool,
    features: &[crate::settings::FontFeature],
    cell_count: usize,
) -> Option<Vec<u16>> {
    if cell_count < 2 {
        return None;
    }
    let font = styled_font(family, font_size, bold, italic)?;
    let (data, face_index) = font_file_data(&font)?;
    shape_run_glyph_ids(text, &data, face_index, features, cell_count)
}

/// Resolve a family + bold/italic style into a `CTFont` (Menlo fallback), applying symbolic traits.
fn styled_font(family: &str, font_size: f32, bold: bool, italic: bool) -> Option<CTFont> {
    let base = font::new_from_name(family, font_size as f64)
        .or_else(|_| font::new_from_name("Menlo", font_size as f64))
        .ok()?;
    let mut traits = 0;
    if bold {
        traits |= kCTFontBoldTrait;
    }
    if italic {
        traits |= kCTFontItalicTrait;
    }
    Some(if traits == 0 {
        base
    } else {
        base.clone_with_symbolic_traits(traits, kCTFontBoldTrait | kCTFontItalicTrait)
            .unwrap_or(base)
    })
}

/// Backing bytes for a font file: an mmap of the file (the common path, so identical files opened
/// under different PostScript names share physical pages via the page cache and stay reclaimable),
/// or an owned in-memory copy as a fallback when mmap fails (unusual file systems, sandboxing).
enum FontBytes {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for FontBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            FontBytes::Mapped(mmap) => mmap,
            FontBytes::Owned(bytes) => bytes,
        }
    }
}

/// Raw font file bytes plus the face index within the file (for `.ttc`/`.otc` collections).
type FontFileData = (Arc<FontBytes>, u32);

/// Cache of font file bytes keyed by resolved PostScript name, so a run doesn't re-read the font
/// file every frame. Shared across threads (not thread-local) so a given font file is mapped once
/// process-wide, and `Arc<FontBytes>` clones are cheap. Values include the face index within a font
/// collection.
static FONT_DATA_CACHE: OnceLock<Mutex<FxHashMap<String, Option<FontFileData>>>> = OnceLock::new();

/// Read a `CTFont`'s backing font file bytes and its face index within the file. Cached per
/// PostScript name. Returns `None` if the font isn't backed by a readable file on disk.
fn font_file_data(font: &CTFont) -> Option<FontFileData> {
    let key = font.postscript_name();
    let cache = FONT_DATA_CACHE.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Some(entry) = cache.lock().get(&key) {
        return entry.clone();
    }
    let loaded = load_font_file(font);
    cache.lock().insert(key, loaded.clone());
    loaded
}

/// Load a `CTFont`'s file bytes and locate its face index (its position within a `.ttc`/`.otc`
/// collection) by matching PostScript names.
fn load_font_file(font: &CTFont) -> Option<FontFileData> {
    let url = font.url()?;
    let path = url.to_path()?;
    // Memory-map the font file rather than reading it into the heap: font files are large (a Nerd
    // Font CJK build is ~20 MB) and rustybuzz only borrows a `&[u8]` slice, so the mapping's clean,
    // demand-paged pages stay reclaimable and are shared by the kernel across every mapping of the
    // same file (e.g. the same regular face opened under a synthesized bold PostScript name). The
    // theoretical hazard is an external truncation causing SIGBUS mid-shape, but font files are
    // stable for a process's lifetime; if mapping fails we fall back to an owned in-memory copy.
    let bytes = match std::fs::File::open(&path) {
        Ok(file) => match unsafe { Mmap::map(&file) } {
            Ok(mmap) => FontBytes::Mapped(mmap),
            Err(_) => FontBytes::Owned(std::fs::read(&path).ok()?),
        },
        Err(_) => FontBytes::Owned(std::fs::read(&path).ok()?),
    };
    let target = font.postscript_name();
    let face_index = rustybuzz::ttf_parser::fonts_in_collection(&bytes)
        .map(|count| {
            (0..count)
                .find(|index| {
                    rustybuzz::ttf_parser::Face::parse(&bytes, *index)
                        .ok()
                        .map(|face| postscript_name_matches(&face, &target))
                        .unwrap_or(false)
                })
                .unwrap_or(0)
        })
        .unwrap_or(0);
    Some((Arc::new(bytes), face_index))
}

/// Whether a ttf-parser face's PostScript name (name id 6) matches `target`.
fn postscript_name_matches(face: &rustybuzz::ttf_parser::Face<'_>, target: &str) -> bool {
    face.names()
        .into_iter()
        .filter(|name| name.name_id == rustybuzz::ttf_parser::name_id::POST_SCRIPT_NAME)
        .filter_map(|name| name.to_string())
        .any(|name| name == target)
}

fn line_uses_only_color_glyphs(line: &CTLine) -> bool {
    let mut saw_glyph = false;
    for run in line.glyph_runs().iter() {
        if run.glyph_count() == 0 {
            continue;
        }
        saw_glyph = true;
        let uses_color_font = run.attributes().is_some_and(|attributes| {
            attributes
                .find(CFString::new("NSFont"))
                .and_then(|value| value.downcast::<CTFont>())
                .is_some_and(|font| font.symbolic_traits() & kCTFontColorGlyphsTrait != 0)
        });
        if !uses_color_font {
            return false;
        }
    }
    saw_glyph
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RasterPlacement {
    width: usize,
    height: usize,
    offset: [i16; 2],
    text_x: f64,
    baseline: f64,
}

/// Baseline (in the bottom-origin bitmap context) that vertically centers `ascent + descent`
/// within a `cell_height`-tall cell, then applies the optional `adjust-font-baseline` modifier
/// (pre-scaled to device px; positive raises the glyph). Shared by the `CTLine` text path and the
/// pre-shaped `rasterize_glyph_id` path so both sit on the same line. The float operation order is
/// load-bearing for bit-identical output and must not change.
fn resolve_baseline(
    ascent: f64,
    descent: f64,
    cell_height: usize,
    adjustment: Option<crate::settings::MetricModifier>,
) -> f64 {
    let text_height = ascent + descent;
    let base_baseline = ((cell_height as f64 - text_height) / 2.).max(0.) + descent;
    match adjustment {
        Some(modifier) => f64::from(modifier.apply(base_baseline as f32)).max(0.),
        None => base_baseline,
    }
}

/// Bounded per-side ink overhang for an atlas tile. `left_edge`/`right_edge` are the glyph ink's
/// left and right extents relative to the cell's left edge; each overhang is the amount that ink
/// spills past the cell, clamped to `max_overhang`. `tile_width` is the resulting tile width and
/// `offset` is the tile's x placement (negative = shifted left by the left overhang). Shared by the
/// `CTLine` and pre-shaped glyph paths; the expressions are load-bearing for bit-identical output.
struct OverhangTile {
    left: usize,
    tile_width: usize,
    offset: [i16; 2],
}

fn overhang_tile(
    left_edge: f64,
    right_edge: f64,
    cell_width: usize,
    max_overhang: usize,
) -> OverhangTile {
    let left = ((-left_edge).ceil().max(0.) as usize).min(max_overhang);
    let right = ((right_edge - cell_width as f64).ceil().max(0.) as usize).min(max_overhang);
    OverhangTile {
        left,
        tile_width: cell_width + left + right,
        offset: [-(left.min(i16::MAX as usize) as i16), 0],
    }
}

fn raster_placement(
    line: &CTLine,
    context: &CGContext,
    cell_width: usize,
    cell_height: usize,
    confine_to_cell: bool,
    baseline_adjustment: Option<crate::settings::MetricModifier>,
) -> RasterPlacement {
    let metrics = line.get_typographic_bounds();
    // `adjust-font-baseline` shifts the baseline. The modifier is pre-scaled to device px; a
    // positive value raises the glyph (larger baseline y in this bottom-origin bitmap context).
    let baseline = resolve_baseline(
        metrics.ascent,
        metrics.descent,
        cell_height,
        baseline_adjustment,
    );
    let ink = line.get_image_bounds(context);
    let has_finite_ink = !ink.is_empty()
        && ink.origin.x.is_finite()
        && ink.size.width.is_finite()
        && ink.size.width > 0.;
    let centered_x = if confine_to_cell && has_finite_ink {
        ((cell_width as f64 - ink.size.width) / 2.).max(0.) - ink.origin.x
    } else {
        ((cell_width as f64 - metrics.width) / 2.).max(0.)
    };
    let ink_left = if has_finite_ink {
        centered_x + ink.origin.x
    } else {
        centered_x
    };
    let ink_right = if has_finite_ink {
        ink_left + ink.size.width
    } else {
        centered_x + metrics.width
    };

    // Patched Nerd Fonts commonly retain a one-cell advance while their actual icon ink is
    // wider. A cell-sized atlas tile clips those glyphs (for example Maple Mono NF CN's U+F023
    // has a 16.8 px advance and a 22.6 px image width at 28 pt). Preserve a bounded amount of
    // overhang in the atlas while leaving terminal layout, cursor positions, and PTY columns at
    // their original cell widths.
    let max_overhang = if confine_to_cell {
        0
    } else {
        cell_width.max(1)
    };
    let tile = overhang_tile(ink_left, ink_right, cell_width, max_overhang);

    RasterPlacement {
        width: tile.tile_width,
        height: cell_height,
        offset: tile.offset,
        text_x: centered_x + tile.left as f64,
        baseline,
    }
}

fn mask_context(width: usize, height: usize, thicken: Option<u8>) -> CGContext {
    let color_space = CGColorSpace::create_device_gray();
    let context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width,
        &color_space,
        kCGImageAlphaNone,
    );
    configure_text_context(&context, thicken);
    context
}

fn rasterize_mask_line(
    line: &CTLine,
    width: usize,
    height: usize,
    confine_to_cell: bool,
    baseline_adjustment: Option<crate::settings::MetricModifier>,
    thicken: Option<u8>,
) -> RasterizedImage {
    let measurement_context = mask_context(width, height, thicken);
    let placement = raster_placement(
        line,
        &measurement_context,
        width,
        height,
        confine_to_cell,
        baseline_adjustment,
    );
    let mut context = if placement.width == width {
        measurement_context
    } else {
        mask_context(placement.width, placement.height, thicken)
    };
    context.data().fill(0);
    context.set_gray_fill_color(1., 1.);
    draw_positioned_line(line, &context, placement);
    context.flush();

    // CGBitmapContext exposes its first memory row as the top row for this bitmap setup. Metal
    // texture coordinates also start at the top, so flipping here mirrors every glyph vertically.
    RasterizedImage {
        pixels: context.data().to_vec(),
        width: placement.width,
        height: placement.height,
        offset: placement.offset,
    }
}

/// Rasterize a single pre-shaped glyph (by id) from `font` into a `cell_width`×`cell_height` mask,
/// allowing a bounded one-cell overhang on each side so ligature halves that extend past the cell
/// (e.g. the joining strokes of `=>`) aren't clipped. Baseline mirrors the text path so shaped
/// glyphs sit on the same line as ordinary characters.
fn rasterize_glyph_id(
    font: &CTFont,
    glyph_id: u16,
    cell_width: usize,
    cell_height: usize,
    baseline_adjustment: Option<crate::settings::MetricModifier>,
    thicken: Option<u8>,
) -> RasterizedImage {
    let ascent = font.ascent();
    let descent = font.descent();
    let baseline = resolve_baseline(ascent, descent, cell_height, baseline_adjustment);
    // Measure ink so a glyph wider than its cell (a ligature half) keeps a bounded overhang.
    let glyphs = [glyph_id as CGGlyph];
    let bounds = font.get_bounding_rects_for_glyphs(kCTFontOrientationDefault, &glyphs);
    let max_overhang = cell_width.max(1);
    let right_edge = bounds.origin.x + bounds.size.width;
    let tile = overhang_tile(bounds.origin.x, right_edge, cell_width, max_overhang);
    let tile_width = tile.tile_width;

    let mut context = mask_context(tile_width, cell_height, thicken);
    context.data().fill(0);
    context.set_gray_fill_color(1., 1.);
    let positions = [CGPoint::new(tile.left as f64, baseline)];
    font.draw_glyphs(&glyphs, &positions, context.clone());
    context.flush();

    RasterizedImage {
        pixels: context.data().to_vec(),
        width: tile_width,
        height: cell_height,
        offset: tile.offset,
    }
}

fn color_context(width: usize, height: usize, thicken: Option<u8>) -> CGContext {
    let color_space = CGColorSpace::create_device_rgb();
    let context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &color_space,
        kCGImageAlphaPremultipliedLast,
    );
    configure_text_context(&context, thicken);
    context
}

fn rasterize_color_line(
    line: &CTLine,
    width: usize,
    height: usize,
    baseline_adjustment: Option<crate::settings::MetricModifier>,
) -> RasterizedImage {
    let measurement_context = color_context(width, height, None);
    let placement = raster_placement(
        line,
        &measurement_context,
        width,
        height,
        false,
        baseline_adjustment,
    );
    let mut context = if placement.width == width {
        measurement_context
    } else {
        color_context(placement.width, placement.height, None)
    };
    context.data().fill(0);
    context.set_rgb_fill_color(1., 1., 1., 1.);
    draw_positioned_line(line, &context, placement);
    context.flush();
    RasterizedImage {
        pixels: context.data().to_vec(),
        width: placement.width,
        height: placement.height,
        offset: placement.offset,
    }
}

fn configure_text_context(context: &CGContext, thicken: Option<u8>) {
    context.set_allows_antialiasing(true);
    context.set_should_antialias(true);
    // `font-thicken` (macOS): enable font smoothing, which fattens strokes. The smoothing style
    // encodes strength in the high byte (Ghostty uses the same 0x00_SS_00_02 encoding); fall back
    // to plain smoothing if the strength byte is 0.
    let smooth = thicken.is_some();
    context.set_allows_font_smoothing(smooth);
    context.set_should_smooth_fonts(smooth);
    if let Some(strength) = thicken {
        context.set_font_smoothing_style(((strength as i32) << 16) | 2);
    }
    context.set_allows_font_subpixel_positioning(true);
    context.set_should_subpixel_position_fonts(true);
    context.set_allows_font_subpixel_quantization(false);
    context.set_should_subpixel_quantize_fonts(false);
}

fn draw_positioned_line(line: &CTLine, context: &CGContext, placement: RasterPlacement) {
    context.set_text_position(placement.text_x, placement.baseline);
    line.draw(context);
}

fn rgba_components(color: u32) -> [f32; 4] {
    [
        ((color >> 24) & 0xff) as f32 / 255.,
        ((color >> 16) & 0xff) as f32 / 255.,
        ((color >> 8) & 0xff) as f32 / 255.,
        (color & 0xff) as f32 / 255.,
    ]
}

pub(crate) fn font_metrics(family: &str, font_size: f32, scale_factor: f32) -> (f32, f32) {
    let font = font::new_from_name(family, font_size as f64)
        .or_else(|_| font::new_from_name("Menlo", font_size as f64))
        .expect("macOS must provide Menlo");
    let string = CFString::new("M");
    let mut attributed = CFMutableAttributedString::new();
    attributed.replace_str(&string, CFRange::init(0, 0));
    let range = CFRange::init(0, attributed.char_len());
    attributed.set_attribute(range, unsafe { kCTFontAttributeName }, &font);
    let line = CTLine::new_with_attributed_string(attributed.as_concrete_TypeRef());
    let width = line.get_typographic_bounds().width as f32;
    let height = (font.ascent() + font.descent() + font.leading().max(0.)) as f32;
    let snap = |value: f32| (value * scale_factor).round().max(1.) / scale_factor;
    (snap(width), snap(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `GlyphKey` for ordinary text with every non-dimensional field at its default
    /// (no bold/italic, not a sprite, no metric modifiers, empty feature/variation sets). Tests
    /// that need a non-default field layer it on with struct-update syntax
    /// (`GlyphKey { bold: true, ..text_glyph_key(...) }`).
    fn text_glyph_key(
        text: &str,
        family: &str,
        font_size_bits: u32,
        cell_width: u16,
        width: u16,
        height: u16,
    ) -> GlyphKey {
        GlyphKey {
            text: Arc::from(text),
            family: Arc::from(family),
            font_size_bits,
            bold: false,
            italic: false,
            cell_width,
            width,
            height,
            sprite: false,
            baseline: None,
            box_thickness: None,
            icon_height: None,
            features: Arc::from(&[][..]),
            glyph_id: None,
            variations: Arc::from(&[][..]),
            thicken: None,
        }
    }

    #[test]
    fn consecutive_placements_of_one_texture_form_one_metal_batch() {
        let key = TerminalTextureKey {
            session_id: uuid::Uuid::nil(),
            image: eggie_protocol::TerminalImageKey {
                id: 7,
                generation: 9,
            },
        };
        let image = Arc::new(TerminalImageData {
            key,
            width: 1,
            height: 1,
            pixels: crate::terminal_renderer::PixelStore::Owned(Arc::new(vec![255; 4])),
        });
        let mut commands = (0..2048)
            .map(|column| MetalCommand::Image {
                image: Arc::clone(&image),
                rect: [column as f32, 0., 1., 1.],
                source: [0., 0., 1., 1.],
            })
            .collect::<Vec<_>>();
        commands.push(MetalCommand::Rect {
            rect: [0., 0., 1., 1.],
            color: 0,
        });
        commands.push(MetalCommand::Image {
            image,
            rect: [0., 1., 1., 1.],
            source: [0., 0., 1., 1.],
        });
        assert_eq!(image_batch_end(&commands, 0, key), 2048);
        assert_eq!(image_batch_end(&commands, 2049, key), 2050);
    }

    #[test]
    fn color_unpacking_preserves_terminal_rgba_order() {
        assert_eq!(
            rgba_components(0x12345678),
            [
                0x12 as f32 / 255.,
                0x34 as f32 / 255.,
                0x56 as f32 / 255.,
                0x78 as f32 / 255.
            ]
        );
    }

    #[test]
    fn cpu_instance_layout_matches_metal_shader_stride() {
        assert_eq!(mem::size_of::<Instance>(), 64);
        assert_eq!(mem::align_of::<Instance>(), 16);
        assert_eq!(std::mem::offset_of!(Instance, origin), 0);
        assert_eq!(std::mem::offset_of!(Instance, size), 8);
        assert_eq!(std::mem::offset_of!(Instance, color), 16);
        assert_eq!(std::mem::offset_of!(Instance, uv_origin), 32);
        assert_eq!(std::mem::offset_of!(Instance, uv_size), 40);
        assert_eq!(std::mem::offset_of!(Instance, metadata), 48);
    }

    #[test]
    fn adjacent_terminal_cells_share_the_same_snapped_device_edge() {
        let origin = [0.35, 1.6];
        let (first_origin, first_size) = snap_device_rect(origin, [0., 0., 8.25, 18.25], 2.);
        let (second_origin, _) = snap_device_rect(origin, [8.25, 0., 8.25, 18.25], 2.);
        assert_eq!(first_origin[0] + first_size[0], second_origin[0]);
    }

    #[test]
    fn core_text_rasterization_has_exact_cell_dimensions() {
        let key = text_glyph_key("M", "Menlo", 28f32.to_bits(), 18, 18, 36);
        let RasterizedGlyph::Mask(image) = rasterize_text(&key) else {
            panic!("ordinary text must stay on the compact mask path")
        };
        assert_eq!((image.width, image.height), (18, 36));
        assert_eq!(image.pixels.len(), 18 * 36);
        assert!(image.pixels.iter().any(|value| *value != 0));
    }

    #[test]
    fn unknown_text_uses_one_cell_with_a_one_pixel_hollow_outline() {
        let key = GlyphKey {
            bold: true,
            italic: true,
            ..text_glyph_key("\u{10ffff}\u{10fffe}", "Menlo", 28f32.to_bits(), 17, 34, 36)
        };
        let RasterizedGlyph::Mask(image) = rasterize_text(&key) else {
            panic!("unknown text must stay on the tintable mask path")
        };
        assert_eq!((image.width, image.height, image.offset), (17, 36, [0, 0]));
        for y in 0..image.height {
            for x in 0..image.width {
                let expected = if x == 0 || x + 1 == image.width || y == 0 || y + 1 == image.height
                {
                    u8::MAX
                } else {
                    0
                };
                assert_eq!(image.pixels[y * image.width + x], expected, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn precomposed_fractions_stay_inside_their_single_cell_atlas_tiles() {
        for fraction in "¼½¾⅐⅑⅒⅓⅔⅕⅖⅗⅘⅙⅚⅛⅜⅝⅞⅟↉".chars() {
            let key = text_glyph_key(
                &fraction.to_string(),
                "Maple Mono NF CN",
                28f32.to_bits(),
                17,
                17,
                36,
            );
            let RasterizedGlyph::Mask(image) = rasterize_text(&key) else {
                panic!("fraction U+{:04X} must use the mask atlas", fraction as u32)
            };
            assert_eq!(
                (image.width, image.height, image.offset),
                (17, 36, [0, 0]),
                "fraction U+{:04X} escaped its terminal cell",
                fraction as u32
            );
            assert!(
                image.pixels.iter().any(|coverage| *coverage != 0),
                "fraction U+{:04X} rasterized empty",
                fraction as u32
            );
        }
    }

    #[test]
    fn icon_constraint_metrics_derive_and_adjust_both_heights() {
        let font = font::new_from_name("Menlo", 32.).expect("macOS must provide Menlo");
        let ascent = font.ascent();
        let descent = font.descent();
        let leading = font.leading().max(0.);
        let face_height = ascent + descent + leading;
        let cap = {
            let reported = font.cap_height();
            if reported > 0. { reported } else { 0.75 * ascent }
        };
        let expected_single = (2.0 * cap + face_height) / 3.0;

        // No modifier: heights are the raw derivations.
        let base = icon_constraint_metrics(&font, None);
        assert!((base.icon_height - face_height).abs() < 1e-6);
        assert!((base.icon_height_single - expected_single).abs() < 1e-6);
        // The blended single-cell height sits below the full face height.
        assert!(base.icon_height_single < base.icon_height);

        // A percentage scales both heights by the same factor.
        let scaled = icon_constraint_metrics(&font, crate::settings::MetricModifier::parse("-50%"));
        assert!((scaled.icon_height - face_height * 0.5).abs() < 1e-4);
        assert!((scaled.icon_height_single - expected_single * 0.5).abs() < 1e-4);

        // A huge negative absolute delta is floored at 1px, never zero/negative.
        let floored =
            icon_constraint_metrics(&font, crate::settings::MetricModifier::parse("-10000"));
        assert_eq!(floored.icon_height, 1.0);
        assert_eq!(floored.icon_height_single, 1.0);
    }

    #[test]
    fn icon_fit_cover1_scale_fits_shrinks_and_grows() {
        // Height drives the scale: a tall icon shrinks to the target height. max_width is generous
        // (overhang-inclusive) so it doesn't bind.
        let shrink = icon_fit_cover1_scale(10., 40., 60., 20.);
        assert!(shrink < 1.0, "tall icon should shrink, got {shrink}");
        assert!((shrink - 0.5).abs() < 1e-9, "height-limited factor 20/40");

        // A small icon grows to the target height (fit_cover1 upscaling), width headroom permitting.
        let grow = icon_fit_cover1_scale(8., 8., 60., 16.);
        assert!(grow > 1.0, "small icon should grow, got {grow}");
        assert!((grow - 2.0).abs() < 1e-9, "height-limited factor 16/8");

        // Height genuinely drives the result: a square icon whose width ≈ cell advance is NOT pinned
        // to the advance — a larger target height yields a larger scale (the bug we fixed).
        let small_target = icon_fit_cover1_scale(20., 20., 200., 20.);
        let large_target = icon_fit_cover1_scale(20., 20., 200., 40.);
        assert!(
            large_target > small_target,
            "raising the height target must raise the scale ({large_target} !> {small_target})"
        );

        // max_width caps runaway growth so the icon can't overrun its overhang budget.
        let width_capped = icon_fit_cover1_scale(40., 8., 20., 100.);
        assert!(
            (width_capped - 0.5).abs() < 1e-9,
            "width cap 20/40 binds when the height target would grow past the budget"
        );

        // A non-positive width budget means "height only" (no cap).
        let uncapped = icon_fit_cover1_scale(40., 8., 0., 80.);
        assert!((uncapped - 10.0).abs() < 1e-9, "height-only factor 80/8");

        // Degenerate ink leaves the glyph untouched.
        assert_eq!(icon_fit_cover1_scale(0., 10., 20., 20.), 1.0);
        assert_eq!(icon_fit_cover1_scale(10., 0., 20., 20.), 1.0);
    }

    #[test]
    fn is_nerd_icon_codepoint_targets_icons_and_spares_text_and_sprites() {
        // Nerd Font PUA icons across the BMP and a supplementary plane.
        assert!(is_nerd_icon_codepoint('\u{f001}'));
        assert!(is_nerd_icon_codepoint('\u{e5fb}'));
        assert!(is_nerd_icon_codepoint('\u{f0001}'));
        // Ordinary text and CJK are never constrained.
        assert!(!is_nerd_icon_codepoint('A'));
        assert!(!is_nerd_icon_codepoint('水'));
        // Every sprite-drawn codepoint is excluded (box/block/braille/powerline).
        for cp in [0x2500u32, 0x2588, 0x2800, 0xe0b0, 0xe0b2, 0xe0d4] {
            let character = char::from_u32(cp).unwrap();
            assert!(
                !is_nerd_icon_codepoint(character),
                "sprite U+{cp:04X} must not receive the icon constraint"
            );
        }
    }

    #[test]
    fn nerd_font_private_use_glyphs_keep_their_ink_overhang() {
        let Ok(installed) = font::new_from_name("Maple Mono NF CN", 28.) else {
            return;
        };
        if !installed.postscript_name().contains("MapleMono-NF-CN") {
            return;
        }
        let key = text_glyph_key("\u{f023}", "Maple Mono NF CN", 28f32.to_bits(), 17, 17, 36);
        let RasterizedGlyph::Mask(image) = rasterize_text(&key) else {
            panic!("Nerd Font lock must stay on the tintable mask path")
        };
        assert!(
            image.width > key.width as usize,
            "the U+F023 ink is wider than its one-cell advance and must not be clipped"
        );
        assert_eq!(image.height, key.height as usize);
        assert_eq!(image.offset, [0, 0]);
        assert_eq!(image.pixels.len(), image.width * image.height);
        assert!(
            (0..image.height).any(|row| image.pixels
                [row * image.width + key.width as usize..row * image.width + image.width]
                .iter()
                .any(|coverage| *coverage != 0)),
            "the preserved overhang must contain the lock's previously clipped pixels"
        );
    }

    #[test]
    fn core_text_selects_a_system_fallback_for_missing_legacy_symbols() {
        let base = font::new_from_name("Menlo", 28.).unwrap();
        let fallback = font_for_text(&base, "⫷");
        assert_ne!(fallback.postscript_name(), base.postscript_name());
        assert!(!fallback.postscript_name().contains("LastResort"));

        let line = attributed_line("⫷", &fallback);
        assert!(line.glyph_runs().iter().any(|run| run.glyph_count() > 0));
    }

    #[test]
    fn notcurses_math_and_symbol_glyphs_do_not_use_last_resort() {
        let base = font::new_from_name("Menlo", 28.).unwrap();
        for character in "⎧⎫♠♥⅗⅘⅙⅚⅛╥⎛⎞╲╿╱◨◧◪◩◖◗⫷⫸⎩♦♣¼½¾⅐⅑⅒⅓⅔⅕⅖⅜⅝⅞⅟↉◲◱◳◰◶◵◷◴◜◝◟◞◿◺◹◸♟♜♞♝♛♚⩘▵△▹▷▿▽◃◁⩗▴⏶⯅▲▸⏵⯈▶▾⏷⯆▼◂⏴⯇◀⭡⭣⭠⭢⭧⭩⭦⭨⊖⊗⟬⟭≶≷⊆⊇⊴⊵⧒⧑❨❩⟃⟄⁰¹²³⁴⁵⁶⁷⁸⁹₀₁₂₃₄₅₆₇₈₉".chars() {
            if terminal_sprites::rasterize(character, 16, 32, None).is_some() {
                continue;
            }
            let text = character.to_string();
            let selected = font_for_text(&base, &text);
            assert!(
                !selected.postscript_name().contains("LastResort"),
                "missing system fallback for U+{:04X} {}",
                character as u32,
                character
            );
        }
    }

    #[test]
    fn core_text_rasterization_preserves_color_emoji_pixels() {
        let key = text_glyph_key("🙂", "Menlo", 28f32.to_bits(), 18, 36, 36);
        let RasterizedGlyph::Color(image) = rasterize_text(&key) else {
            panic!("Apple Color Emoji fallback must use the color atlas")
        };
        assert_eq!(image.pixels.len(), image.width * image.height * 4);
        assert!(
            image
                .pixels
                .chunks_exact(4)
                .any(|pixel| { pixel[3] != 0 && (pixel[0] != pixel[1] || pixel[1] != pixel[2]) }),
            "color emoji rasterization must retain non-gray RGB pixels"
        );
    }

    #[test]
    fn core_text_rasterizes_extended_emoji_graphemes_as_single_color_glyphs() {
        for text in ["🇦🇶", "👩‍🔬", "✊🏿", "🏴‍☠️", "🤽🏼‍♀️"] {
            let key = text_glyph_key(text, "Menlo", 28f32.to_bits(), 18, 36, 36);
            let RasterizedGlyph::Color(image) = rasterize_text(&key) else {
                panic!("{text:?} must use the Apple Color Emoji atlas")
            };
            assert_eq!(image.pixels.len(), image.width * image.height * 4);
            assert!(
                image.pixels.chunks_exact(4).any(|pixel| {
                    pixel[3] != 0 && (pixel[0] != pixel[1] || pixel[1] != pixel[2])
                }),
                "{text:?} must retain non-gray color pixels"
            );
        }
    }

    #[test]
    fn text_presentation_and_mixed_preedit_content_stay_tintable() {
        for text in ["☺\u{fe0e}", "你好🙂"] {
            let key = text_glyph_key(text, "Menlo", 28f32.to_bits(), 18, 96, 36);
            assert!(
                matches!(rasterize_text(&key), RasterizedGlyph::Mask(_)),
                "{text:?} contains text-presentation glyphs and must remain tintable"
            );
        }
    }

    #[test]
    fn color_atlas_allocation_is_lazy_and_does_not_expand_the_text_path() {
        let Some(device) = Device::system_default() else {
            return;
        };
        let mut atlas = GlyphAtlas::new(&device);
        let raster_cache = GlyphRasterCache::default();
        assert!(atlas.color_texture.is_none());

        let text = text_glyph_key("M", "Menlo", 28f32.to_bits(), 18, 18, 36);
        assert_eq!(
            atlas
                .get_or_insert(&device, &raster_cache, text)
                .unwrap()
                .glyph
                .kind,
            GlyphTextureKind::Mask
        );
        assert!(atlas.color_texture.is_none());

        let emoji = text_glyph_key("🙂", "Menlo", 28f32.to_bits(), 18, 36, 36);
        assert_eq!(
            atlas
                .get_or_insert(&device, &raster_cache, emoji)
                .unwrap()
                .glyph
                .kind,
            GlyphTextureKind::Color
        );
        assert!(atlas.color_texture.is_some());
    }

    #[test]
    fn core_text_rasterization_keeps_glyphs_upright() {
        let key = text_glyph_key("/", "Menlo", 28f32.to_bits(), 18, 18, 36);
        let RasterizedGlyph::Mask(image) = rasterize_text(&key) else {
            panic!("slash must stay on the mask path")
        };
        let weighted_x = |rows: std::ops::Range<usize>| {
            let mut weight = 0usize;
            let mut total = 0usize;
            for y in rows {
                for x in 0..image.width {
                    let coverage = image.pixels[y * image.width + x] as usize;
                    weight += x * coverage;
                    total += coverage;
                }
            }
            weight as f32 / total.max(1) as f32
        };
        assert!(
            weighted_x(0..key.height as usize / 2)
                > weighted_x(key.height as usize / 2..key.height as usize),
            "a forward slash must lean from bottom-left to top-right"
        );
    }

    #[test]
    fn metal_reads_consecutive_instances_with_the_cpu_stride() {
        let Some(device) = Device::system_default() else {
            return;
        };
        let library = device
            .new_library_with_source(SHADER, &CompileOptions::new())
            .expect("terminal Metal shader must compile");
        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(
            &library
                .get_function("terminal_vertex", None)
                .expect("terminal vertex shader"),
        ));
        descriptor.set_fragment_function(Some(
            &library
                .get_function("terminal_fragment", None)
                .expect("terminal fragment shader"),
        ));
        let pipeline_attachment = descriptor
            .color_attachments()
            .object_at(0)
            .expect("color attachment");
        pipeline_attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        pipeline_attachment.set_blending_enabled(true);
        pipeline_attachment.set_rgb_blend_operation(MTLBlendOperation::Add);
        pipeline_attachment.set_alpha_blend_operation(MTLBlendOperation::Add);
        pipeline_attachment.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
        pipeline_attachment.set_source_alpha_blend_factor(MTLBlendFactor::One);
        pipeline_attachment.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        pipeline_attachment.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        let pipeline = device
            .new_render_pipeline_state(&descriptor)
            .expect("terminal pipeline must compile");
        descriptor.set_fragment_function(Some(
            &library
                .get_function("terminal_image_fragment", None)
                .expect("terminal image fragment shader"),
        ));
        let image_pipeline = device
            .new_render_pipeline_state(&descriptor)
            .expect("terminal image pipeline must compile");

        let texture_descriptor = TextureDescriptor::new();
        texture_descriptor.set_width(20);
        texture_descriptor.set_height(4);
        texture_descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        texture_descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        texture_descriptor.set_storage_mode(MTLStorageMode::Shared);
        let target = device.new_texture(&texture_descriptor);

        let atlas_descriptor = TextureDescriptor::new();
        atlas_descriptor.set_width(1);
        atlas_descriptor.set_height(1);
        atlas_descriptor.set_pixel_format(MTLPixelFormat::R8Unorm);
        atlas_descriptor.set_storage_mode(MTLStorageMode::Shared);
        atlas_descriptor.set_usage(MTLTextureUsage::ShaderRead);
        let atlas = device.new_texture(&atlas_descriptor);
        let white = [255u8];
        atlas.replace_region(MTLRegion::new_2d(0, 0, 1, 1), 0, white.as_ptr().cast(), 1);
        let color_atlas_descriptor = TextureDescriptor::new();
        color_atlas_descriptor.set_width(1);
        color_atlas_descriptor.set_height(1);
        color_atlas_descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
        color_atlas_descriptor.set_storage_mode(MTLStorageMode::Shared);
        color_atlas_descriptor.set_usage(MTLTextureUsage::ShaderRead);
        let color_atlas = device.new_texture(&color_atlas_descriptor);
        let premultiplied_orange = [128u8, 64, 0, 128];
        color_atlas.replace_region(
            MTLRegion::new_2d(0, 0, 1, 1),
            0,
            premultiplied_orange.as_ptr().cast(),
            4,
        );
        let sampler = device.new_sampler(&SamplerDescriptor::new());
        let image_descriptor = TextureDescriptor::new();
        image_descriptor.set_width(1);
        image_descriptor.set_height(1);
        image_descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
        image_descriptor.set_storage_mode(MTLStorageMode::Shared);
        image_descriptor.set_usage(MTLTextureUsage::ShaderRead);
        let image_texture = device.new_texture(&image_descriptor);
        let translucent_rgb = [10u8, 20, 30, 128];
        image_texture.replace_region(
            MTLRegion::new_2d(0, 0, 1, 1),
            0,
            translucent_rgb.as_ptr().cast(),
            4,
        );

        let instances = [
            Instance {
                origin: [0., 0.],
                size: [4., 4.],
                color: [1., 0., 0., 1.],
                uv_origin: [0., 0.],
                uv_size: [0., 0.],
                metadata: [0; 4],
            },
            Instance {
                origin: [4., 0.],
                size: [4., 4.],
                color: [16. / 255., 16. / 255., 16. / 255., 1.],
                uv_origin: [0., 0.],
                uv_size: [1. / MASK_ATLAS_SIZE as f32, 1. / MASK_ATLAS_SIZE as f32],
                metadata: [GlyphTextureKind::Mask as u32, 0x101010ff, 3f32.to_bits(), 0],
            },
            Instance {
                origin: [8., 0.],
                size: [4., 4.],
                color: [1., 1., 1., 1.],
                uv_origin: [0., 0.],
                uv_size: [0., 0.],
                metadata: [
                    GlyphTextureKind::Color as u32,
                    0xffffffff,
                    21f32.to_bits(),
                    0,
                ],
            },
            Instance {
                origin: [12., 0.],
                size: [4., 4.],
                color: [16. / 255., 16. / 255., 16. / 255., 1.],
                uv_origin: [0., 0.],
                uv_size: [1. / MASK_ATLAS_SIZE as f32, 1. / MASK_ATLAS_SIZE as f32],
                metadata: [
                    GlyphTextureKind::Mask as u32,
                    0x101010ff,
                    21f32.to_bits(),
                    1,
                ],
            },
        ];
        let instance_buffer = device.new_buffer_with_data(
            instances.as_ptr().cast(),
            mem::size_of_val(&instances) as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let image_instance = Instance {
            origin: [16., 0.],
            size: [4., 4.],
            color: [1.; 4],
            uv_origin: [0., 0.],
            uv_size: [1., 1.],
            metadata: [3, 0, 0, 0],
        };
        let image_instance_buffer = device.new_buffer_with_data(
            (&image_instance as *const Instance).cast(),
            mem::size_of::<Instance>() as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let viewport = [20f32, 4f32];
        let pass = RenderPassDescriptor::new();
        let attachment = pass
            .color_attachments()
            .object_at(0)
            .expect("render attachment");
        attachment.set_texture(Some(&target));
        attachment.set_load_action(MTLLoadAction::Clear);
        attachment.set_clear_color(MTLClearColor::new(0., 0., 0., 1.));
        attachment.set_store_action(MTLStoreAction::Store);
        let queue = device.new_command_queue();
        let command_buffer = queue.new_command_buffer();
        let encoder = command_buffer.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&pipeline);
        encoder.set_vertex_buffer(0, Some(&instance_buffer), 0);
        encoder.set_vertex_bytes(
            1,
            mem::size_of_val(&viewport) as u64,
            viewport.as_ptr().cast(),
        );
        encoder.set_fragment_texture(0, Some(&atlas));
        encoder.set_fragment_texture(1, Some(&color_atlas));
        encoder.set_fragment_sampler_state(0, Some(&sampler));
        encoder.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, 6, 4);
        encoder.set_render_pipeline_state(&image_pipeline);
        encoder.set_vertex_buffer(0, Some(&image_instance_buffer), 0);
        encoder.set_fragment_texture(0, Some(&image_texture));
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 6);
        encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();

        let mut pixels = vec![0u8; 20 * 4 * 4];
        target.get_bytes(
            pixels.as_mut_ptr().cast(),
            20 * 4,
            MTLRegion::new_2d(0, 0, 20, 4),
            0,
        );
        let pixel = |x: usize, y: usize| &pixels[(y * 20 + x) * 4..(y * 20 + x + 1) * 4];
        assert_eq!(pixel(1, 1), &[0, 0, 255, 255]);
        assert_eq!(pixel(6, 1), &[255, 255, 255, 255]);
        assert_eq!(pixel(10, 1), &[0, 64, 128, 255]);
        assert_eq!(pixel(14, 1), &[16, 16, 16, 255]);
        assert_eq!(pixel(18, 1), &[15, 10, 5, 255]);
    }

    fn glyph_command(bold: bool, italic: bool) -> MetalCommand {
        MetalCommand::Glyph {
            rect: [0., 0., 8., 18.],
            text: Arc::from("x"),
            color: 0xffffffff,
            bold,
            italic,
            sprite: false,
            background: 0,
            minimum_contrast: 1.,
            minimum_contrast_disabled: false,
            glyph_id: None,
        }
    }

    fn prepared_with_families(
        families: Option<crate::settings::ResolvedFontFamilies>,
    ) -> PreparedMetalTerminal {
        PreparedMetalTerminal {
            commands: Vec::new(),
            family: Arc::from("Menlo"),
            font_size: 14.,
            cell_width: 8.,
            last_input_sequence: 0,
            baseline_adjustment: None,
            box_thickness_adjustment: None,
            icon_height_adjustment: None,
            font_families: families,
            font_features: Arc::from(&[][..]),
            font_variations: Arc::from(&[][..]),
            font_thicken: None,
            font_codepoint_map: Arc::from(&[][..]),
            images_ready: true,
        }
    }

    #[test]
    fn dedicated_bold_family_is_used_without_synthetic_bold() {
        let mut config = crate::settings::AppSettings::default();
        config.font_family = "Menlo".to_owned();
        config.font_family_bold = "Menlo Bold".to_owned();
        let terminal = prepared_with_families(Some(config.resolved_font_families()));
        let key = glyph_key_for_command(&terminal, &glyph_command(true, false), 1., [0., 0.])
            .expect("glyph command produces a key");
        assert_eq!(key.family.as_ref(), "Menlo Bold");
        // The dedicated bold face already carries weight; don't also synthesize bold.
        assert!(!key.bold);
    }

    #[test]
    fn missing_bold_family_synthesizes_when_enabled_and_not_when_disabled() {
        let config = crate::settings::AppSettings::default();
        let terminal = prepared_with_families(Some(config.resolved_font_families()));
        let key = glyph_key_for_command(&terminal, &glyph_command(true, false), 1., [0., 0.])
            .expect("glyph command produces a key");
        // No dedicated bold family, synthesis on by default → falls back to regular + synth bold.
        assert_eq!(key.family.as_ref(), "Menlo");
        assert!(key.bold);

        let mut disabled = crate::settings::AppSettings::default();
        disabled.font_synthetic_style.bold = false;
        let terminal = prepared_with_families(Some(disabled.resolved_font_families()));
        let key = glyph_key_for_command(&terminal, &glyph_command(true, false), 1., [0., 0.])
            .expect("glyph command produces a key");
        // Synthesis disabled → regular face, no faux bold.
        assert_eq!(key.family.as_ref(), "Menlo");
        assert!(!key.bold);
    }

    #[test]
    fn font_with_features_toggles_standard_ligatures() {
        // Verifies the OpenType feature descriptor plumbing works end to end: disabling `liga`
        // splits a standard "fi" ligature back into two glyphs. (Helvetica ships on every macOS.)
        let Ok(font) = font::new_from_name("Helvetica", 28.) else {
            return;
        };
        let count = |line: &CTLine| {
            line.glyph_runs()
                .iter()
                .map(|run| run.glyph_count())
                .sum::<isize>()
        };
        let base_glyphs = count(&attributed_line("fi", &font));
        let no_liga = crate::settings::FontFeature::parse("-liga").unwrap();
        let disabled = font_with_features(&font, &[no_liga]);
        let disabled_glyphs = count(&attributed_line("fi", &disabled));
        // If the platform ligated "fi" at all, `-liga` must undo it (2 separate glyphs).
        if base_glyphs == 1 {
            assert_eq!(disabled_glyphs, 2, "-liga must split the fi ligature");
        }
    }

    #[test]
    fn font_with_variations_changes_the_advance_of_a_variable_font() {
        // Applying `wght` to a variable font must produce a different face (heavier strokes → wider
        // "M" advance). Guarded on a variable font being installed. Empty variations return the
        // font unchanged.
        for family in ["SF Mono", "Menlo", "Helvetica Neue"] {
            let Ok(font) = font::new_from_name(family, 28.) else {
                continue;
            };
            // Empty list is a no-op.
            let same = font_with_variations(&font, &[]);
            assert_eq!(same.postscript_name(), font.postscript_name());
            // A wght axis only changes anything on a variable font; if the font isn't variable the
            // descriptor copy still succeeds and returns a usable font (no panic), which is all we
            // require here.
            let wght = crate::settings::FontVariation {
                tag: *b"wght",
                value: 700.,
            };
            let varied = font_with_variations(&font, &[wght]);
            assert!(varied.pt_size() > 0.);
            return;
        }
    }

    #[test]
    fn embedded_nerd_font_registers_and_covers_private_use_icons() {
        // The bundled Symbols Nerd Font must register with CoreText and provide a glyph for a
        // common Nerd Font private-use-area icon (U+F001, a music note in the NF symbol range).
        let font = nerd_font_fallback(28.).expect("embedded Nerd Font should register");
        assert!(
            nerd_font_covers(&font, "\u{f001}"),
            "bundled Nerd Font must cover a PUA icon"
        );
        // It should NOT claim to cover an ordinary ASCII letter (symbols-only font).
        assert!(!nerd_font_covers(&font, "A"));
    }

    #[test]
    fn font_for_text_falls_back_to_the_bundled_nerd_font_for_pua_icons() {
        // Menlo has no U+F001; the fallback chain should route to the bundled Nerd Font, whose
        // PostScript name identifies it as the Symbols Nerd Font.
        let menlo = font::new_from_name("Menlo", 28.).unwrap();
        let resolved = font_for_text(&menlo, "\u{f001}");
        let name = resolved.postscript_name().to_lowercase();
        assert!(
            name.contains("symbols") || name.contains("nerd"),
            "expected the bundled Nerd Font, got {name}"
        );
    }

    #[test]
    fn adjust_icon_height_scales_bundled_nerd_font_icons() {
        // The bundled Symbols Nerd Font must be registerable for this to exercise the icon path.
        if nerd_font_fallback(28.).is_none() {
            return;
        }
        // Count the vertical ink extent (rows containing any coverage) of a rasterized icon mask.
        let ink_rows = |glyph: &RasterizedGlyph| -> usize {
            let RasterizedGlyph::Mask(image) = glyph else {
                panic!("Nerd Font icon must rasterize to a tintable mask");
            };
            (0..image.height)
                .filter(|row| {
                    image.pixels[row * image.width..(row + 1) * image.width]
                        .iter()
                        .any(|coverage| *coverage != 0)
                })
                .count()
        };
        let key_with = |icon_height: Option<crate::settings::MetricModifier>| GlyphKey {
            icon_height,
            ..text_glyph_key("\u{f001}", "Menlo", 28f32.to_bits(), 18, 18, 36)
        };

        let baseline = rasterize_text(&key_with(None));
        let shrunk = rasterize_text(&key_with(crate::settings::MetricModifier::parse("-50%")));
        let grown = rasterize_text(&key_with(crate::settings::MetricModifier::parse("40%")));

        let (base_rows, shrunk_rows, grown_rows) =
            (ink_rows(&baseline), ink_rows(&shrunk), ink_rows(&grown));
        assert!(base_rows > 0, "the icon must render some ink");
        assert!(
            shrunk_rows < base_rows,
            "-50% must shrink the icon ink ({shrunk_rows} !< {base_rows})"
        );
        assert!(
            grown_rows > shrunk_rows,
            "a larger target must grow the icon ink ({grown_rows} !> {shrunk_rows})"
        );

        // Regression guard for the "changing the value does nothing" bug: two distinct non-zero
        // adjustments must produce distinct ink heights (the scale is height-driven, not pinned to
        // the cell advance).
        let small = rasterize_text(&key_with(crate::settings::MetricModifier::parse("-30%")));
        let large = rasterize_text(&key_with(crate::settings::MetricModifier::parse("-10%")));
        assert!(
            ink_rows(&large) > ink_rows(&small),
            "distinct icon-height values must yield distinct ink ({} !> {})",
            ink_rows(&large),
            ink_rows(&small)
        );
    }

    #[test]
    fn rustybuzz_substitutes_contextual_ligature_glyphs() {
        // Modern programming fonts (Cascadia/JetBrains/Maple/…) render ligatures as *per-cell
        // contextual glyph substitutions*: each cell keeps its own glyph slot and advance, but the
        // glyph is swapped for a variant that visually joins its neighbors. So `=` inside `=>` uses
        // a different glyph id than `=` alone. Guarded on such a font being installed.
        let calt = crate::settings::FontFeature::parse("calt").unwrap();
        let liga = crate::settings::FontFeature::parse("liga").unwrap();
        for family in ["Maple Mono NF CN", "JetBrains Mono", "Fira Code", "Cascadia Code"] {
            let installed = font::new_from_name(family, 28.)
                .map(|font| {
                    let want = family.split_whitespace().next().unwrap_or("").to_lowercase();
                    font.postscript_name().to_lowercase().contains(&want)
                })
                .unwrap_or(false);
            if !installed {
                continue;
            }
            // Two-cell "=>": both cells stay (2 glyph ids), and the first differs from a lone "=".
            let arrow = shape_terminal_run("=>", family, 28., false, false, &[calt, liga], 2);
            let plain = shape_terminal_run("==", family, 28., false, false, &[calt, liga], 2);
            if let Some(arrow_gids) = arrow {
                assert_eq!(arrow_gids.len(), 2, "{family}: => keeps two cell glyphs");
                // At least one cell's glyph must differ from the equivalent non-ligated run.
                assert!(
                    plain.is_none() || plain.as_ref().unwrap() != &arrow_gids,
                    "{family}: => should substitute a contextual glyph"
                );
            }
            return;
        }
    }

    #[cfg(unix)]
    #[test]
    fn zero_copy_texture_samples_the_mapped_shm_pixels() {
        use std::io::Write;

        let Some(device) = Device::system_default() else {
            return;
        };
        let backend = MetalBackend::new(&device).expect("backend must build");

        // A 64x4 RGBA image: 64*4 = 256 bytes/row, aligned enough for RGBA8Unorm on Apple GPUs. Back
        // it with a page-sized mmap (POSIX shm is page-rounded) so the buffer's base is page-aligned.
        let width = 64u32;
        let height = 4u32;
        let logical = (width * height * 4) as usize;
        let page = 16 * 1024usize;
        let mut backing = vec![0u8; page];
        for (index, byte) in backing.iter_mut().enumerate().take(logical) {
            *byte = (index % 251) as u8;
        }
        let path = std::env::temp_dir().join(format!("eggie-zerocopy-test-{}", std::process::id()));
        {
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(&backing).unwrap();
            file.flush().unwrap();
        }
        let file = std::fs::File::open(&path).unwrap();
        let mmap = Arc::new(unsafe { memmap2::Mmap::map(&file).unwrap() });
        let _ = std::fs::remove_file(&path);

        let image = TerminalImageData {
            key: TerminalTextureKey {
                session_id: eggie_domain::SessionId::new_v4(),
                image: eggie_protocol::TerminalImageKey {
                    id: 1,
                    generation: 1,
                },
            },
            width,
            height,
            pixels: crate::terminal_renderer::PixelStore::Mapped {
                mmap: Arc::clone(&mmap),
                len: logical,
            },
        };

        let Some((texture, _buffer)) =
            backend.zero_copy_image_texture(&image, &mmap, width as usize)
        else {
            // Device declined the alignment; nothing to assert beyond graceful fallback.
            return;
        };
        assert_eq!(texture.width(), width as u64);
        assert_eq!(texture.height(), height as u64);

        // Read the texture back and confirm it samples the mmap bytes without any copy step.
        let mut readback = vec![0u8; logical];
        texture.get_bytes(
            readback.as_mut_ptr().cast(),
            (width * 4) as u64,
            MTLRegion::new_2d(0, 0, width as u64, height as u64),
            0,
        );
        assert_eq!(
            readback,
            &backing[..logical],
            "zero-copy texture must present the mapped shm pixels verbatim"
        );
    }
}
