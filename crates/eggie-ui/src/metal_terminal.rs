use crate::input_latency::MetalFrameStats;
use crate::terminal_renderer::{
    MetalCommand, PreparedMetalTerminal, TerminalImageData, TerminalTextureKey,
};
use crate::terminal_sprites;
use anyhow::{Result, anyhow};
use core_foundation::{
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    boolean::CFBoolean,
    string::{CFString, CFStringRef},
};
use core_graphics::{
    base::{kCGImageAlphaNone, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    context::CGContext,
};
use core_text::{
    font::{self, CTFont, CTFontRef},
    font_descriptor::{kCTFontBoldTrait, kCTFontColorGlyphsTrait, kCTFontItalicTrait},
    line::CTLine,
    string_attributes::{kCTFontAttributeName, kCTForegroundColorFromContextAttributeName},
};
use gpui::MetalRenderContext;
use metal::{
    CompileOptions, Device, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLStorageMode,
    MTLTextureUsage, RenderPipelineDescriptor, RenderPipelineState, SamplerDescriptor,
    SamplerState, Texture, TextureDescriptor,
};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    cell::RefCell,
    collections::HashSet,
    hash::Hash,
    mem,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use metal::{
    MTLClearColor, MTLLoadAction, MTLResourceOptions, MTLStoreAction, RenderPassDescriptor,
};

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
}

struct RecycledTerminalImage {
    texture: Texture,
    width: u32,
    height: u32,
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
            },
        );
        Ok((texture, expected, started.elapsed()))
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
    Some(GlyphKey {
        text: Arc::clone(text),
        family: Arc::clone(&terminal.family),
        font_size_bits: (terminal.font_size * scale).to_bits(),
        bold: *bold,
        italic: *italic,
        cell_width: (cell_pixel_size[0].max(1.) as usize).min(u16::MAX as usize) as u16,
        width: (pixel_size[0].max(1.) as usize).min(u16::MAX as usize) as u16,
        height: (pixel_size[1].max(1.) as usize).min(u16::MAX as usize) as u16,
        sprite: *sprite,
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
            .and_then(|character| terminal_sprites::rasterize(character, width, height))
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
        self.fonts.insert(style, font.clone());
        font
    }

    fn rasterize_text(&mut self, key: &GlyphKey) -> RasterizedGlyph {
        let width = key.width as usize;
        let height = key.height as usize;
        let font = self.font(key);
        let font = font_for_text(&font, &key.text);
        let confine_to_cell = is_single_cell_fraction(&key.text);
        let font = if confine_to_cell {
            fit_font_to_cell(&key.text, &font, width, height)
        } else {
            font
        };
        let line = attributed_line(&key.text, &font);
        if font.postscript_name().contains("LastResort") || line_uses_last_resort(&line) {
            return unknown_glyph_outline(key.cell_width as usize, height);
        }
        if !key.text.contains('\u{fe0e}') && line_uses_only_color_glyphs(&line) {
            RasterizedGlyph::Color(rasterize_color_line(&line, width, height))
        } else {
            RasterizedGlyph::Mask(rasterize_mask_line(&line, width, height, confine_to_cell))
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
    let context = mask_context(width, height);
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
    if fallback.is_null() {
        base_font.clone()
    } else {
        unsafe { CTFont::wrap_under_create_rule(fallback) }
    }
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

fn raster_placement(
    line: &CTLine,
    context: &CGContext,
    cell_width: usize,
    cell_height: usize,
    confine_to_cell: bool,
) -> RasterPlacement {
    let metrics = line.get_typographic_bounds();
    let text_height = metrics.ascent + metrics.descent;
    let baseline = ((cell_height as f64 - text_height) / 2.).max(0.) + metrics.descent;
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
    let left_overhang = ((-ink_left).ceil().max(0.) as usize).min(max_overhang);
    let right_overhang =
        ((ink_right - cell_width as f64).ceil().max(0.) as usize).min(max_overhang);
    let width = cell_width + left_overhang + right_overhang;

    RasterPlacement {
        width,
        height: cell_height,
        offset: [-(left_overhang.min(i16::MAX as usize) as i16), 0],
        text_x: centered_x + left_overhang as f64,
        baseline,
    }
}

fn mask_context(width: usize, height: usize) -> CGContext {
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
    configure_text_context(&context);
    context
}

fn rasterize_mask_line(
    line: &CTLine,
    width: usize,
    height: usize,
    confine_to_cell: bool,
) -> RasterizedImage {
    let measurement_context = mask_context(width, height);
    let placement = raster_placement(line, &measurement_context, width, height, confine_to_cell);
    let mut context = if placement.width == width {
        measurement_context
    } else {
        mask_context(placement.width, placement.height)
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

fn color_context(width: usize, height: usize) -> CGContext {
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
    configure_text_context(&context);
    context
}

fn rasterize_color_line(line: &CTLine, width: usize, height: usize) -> RasterizedImage {
    let measurement_context = color_context(width, height);
    let placement = raster_placement(line, &measurement_context, width, height, false);
    let mut context = if placement.width == width {
        measurement_context
    } else {
        color_context(placement.width, placement.height)
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

fn configure_text_context(context: &CGContext) {
    context.set_allows_antialiasing(true);
    context.set_should_antialias(true);
    context.set_allows_font_smoothing(false);
    context.set_should_smooth_fonts(false);
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
            pixels: Arc::new(vec![255; 4]),
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
        let key = GlyphKey {
            text: Arc::from("M"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 18,
            width: 18,
            height: 36,
            sprite: false,
        };
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
            text: Arc::from("\u{10ffff}\u{10fffe}"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: true,
            italic: true,
            cell_width: 17,
            width: 34,
            height: 36,
            sprite: false,
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
            let key = GlyphKey {
                text: Arc::from(fraction.to_string()),
                family: Arc::from("Maple Mono NF CN"),
                font_size_bits: 28f32.to_bits(),
                bold: false,
                italic: false,
                cell_width: 17,
                width: 17,
                height: 36,
                sprite: false,
            };
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
    fn nerd_font_private_use_glyphs_keep_their_ink_overhang() {
        let Ok(installed) = font::new_from_name("Maple Mono NF CN", 28.) else {
            return;
        };
        if !installed.postscript_name().contains("MapleMono-NF-CN") {
            return;
        }
        let key = GlyphKey {
            text: Arc::from("\u{f023}"),
            family: Arc::from("Maple Mono NF CN"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 17,
            width: 17,
            height: 36,
            sprite: false,
        };
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
            if terminal_sprites::rasterize(character, 16, 32).is_some() {
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
        let key = GlyphKey {
            text: Arc::from("🙂"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 18,
            width: 36,
            height: 36,
            sprite: false,
        };
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
            let key = GlyphKey {
                text: Arc::from(text),
                family: Arc::from("Menlo"),
                font_size_bits: 28f32.to_bits(),
                bold: false,
                italic: false,
                cell_width: 18,
                width: 36,
                height: 36,
                sprite: false,
            };
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
            let key = GlyphKey {
                text: Arc::from(text),
                family: Arc::from("Menlo"),
                font_size_bits: 28f32.to_bits(),
                bold: false,
                italic: false,
                cell_width: 18,
                width: 96,
                height: 36,
                sprite: false,
            };
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

        let text = GlyphKey {
            text: Arc::from("M"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 18,
            width: 18,
            height: 36,
            sprite: false,
        };
        assert_eq!(
            atlas
                .get_or_insert(&device, &raster_cache, text)
                .unwrap()
                .glyph
                .kind,
            GlyphTextureKind::Mask
        );
        assert!(atlas.color_texture.is_none());

        let emoji = GlyphKey {
            text: Arc::from("🙂"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 18,
            width: 36,
            height: 36,
            sprite: false,
        };
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
        let key = GlyphKey {
            text: Arc::from("/"),
            family: Arc::from("Menlo"),
            font_size_bits: 28f32.to_bits(),
            bold: false,
            italic: false,
            cell_width: 18,
            width: 18,
            height: 36,
            sprite: false,
        };
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
}
