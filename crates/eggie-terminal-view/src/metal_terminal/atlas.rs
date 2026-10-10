use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct GlyphKey {
    pub(super) text: Arc<str>,
    pub(super) family: Arc<str>,
    pub(super) font_size_bits: u32,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) cell_width: u16,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) sprite: bool,
    /// `adjust-font-baseline` modifier, pre-scaled to device px (text glyphs only).
    pub(super) baseline: Option<eggie_config::font::MetricModifier>,
    /// `adjust-box-thickness` modifier, pre-scaled to device px (sprite/box-drawing glyphs only).
    pub(super) box_thickness: Option<eggie_config::font::MetricModifier>,
    /// `adjust-icon-height` modifier, pre-scaled to device px. Only set on Nerd Font icon cells
    /// (`is_nerd_icon_codepoint`); `None` everywhere else keeps ordinary text out of a cache split.
    pub(super) icon_height: Option<eggie_config::font::MetricModifier>,
    /// OpenType feature overrides applied when shaping this glyph (text glyphs only). Empty = font
    /// defaults. Keeps deterministic ligature on/off in the cache key.
    pub(super) features: Arc<[eggie_config::font::FontFeature]>,
    /// A pre-shaped glyph id to draw directly (from a ligature run's per-cell substitution), instead
    /// of shaping `text`. `None` renders `text` normally.
    pub(super) glyph_id: Option<u16>,
    /// Variable-font axis settings applied to the face (empty = font defaults).
    pub(super) variations: Arc<[eggie_config::font::FontVariation]>,
    /// macOS font-smoothing thickening: `None` = off, `Some(strength)` = on at 0–255.
    pub(super) thicken: Option<u8>,
}

pub(super) enum RasterCacheEntry {
    Pending,
    Ready {
        glyph: Arc<RasterizedGlyph>,
        /// Prepared-frame index at which this glyph was last part of the visible working set (or
        /// first rasterized). Idle reclamation compares it against the current frame counter.
        last_used: u64,
    },
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
pub(super) struct GlyphRasterCounters {
    pub(super) background_jobs: AtomicU64,
    pub(super) background_nanos: AtomicU64,
    pub(super) synchronous_jobs: AtomicU64,
    pub(super) synchronous_nanos: AtomicU64,
    pub(super) pending: AtomicUsize,
}

/// CPU-side glyph cache populated by a small dedicated CoreText worker pool.
///
/// CoreText fallback and bitmap rasterization can take several hundred microseconds for a new
/// Unicode scalar. Doing that work from `MetalPrimitiveRenderer::paint` stalls GPUI's command
/// encoder and makes every other primitive miss the same frame. The CPU cache lets prepaint queue
/// the complete visible glyph set while Metal only performs cheap atlas uploads.
pub(crate) struct GlyphRasterCache {
    pub(super) entries: Arc<Mutex<FxHashMap<GlyphKey, RasterCacheEntry>>>,
    pub(super) sender: SyncSender<GlyphKey>,
    pub(super) counters: Arc<GlyphRasterCounters>,
    /// Monotonic count of `prepare_terminal` calls, used as the "now" for idle reclamation. Shared
    /// with the raster workers so freshly rasterized glyphs are stamped with the current frame.
    pub(super) frame: Arc<AtomicU64>,
}

impl Default for GlyphRasterCache {
    fn default() -> Self {
        const QUEUE_CAPACITY: usize = 32_768;
        let entries = Arc::new(Mutex::new(FxHashMap::default()));
        let counters = Arc::new(GlyphRasterCounters::default());
        let frame = Arc::new(AtomicU64::new(0));
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
            let frame = Arc::clone(&frame);
            thread::Builder::new()
                .name(format!("eggie-glyph-raster-{index}"))
                .spawn(move || glyph_raster_worker(receiver, entries, counters, frame))
                .expect("spawn terminal glyph raster worker");
        }
        Self {
            entries,
            sender,
            counters,
            frame,
        }
    }
}

impl GlyphRasterCache {
    /// Bound the cache in place before this frame's visible glyphs are (re)stamped.
    ///
    /// Two tiers, cheapest-common-case first:
    /// - Below [`CPU_GLYPH_CACHE_IDLE_WATERMARK`]: no scan at all, so ordinary sessions keep every
    ///   glyph warm.
    /// - Past the watermark: drop `Ready` glyphs neither visible this frame nor drawn within the
    ///   last [`CPU_GLYPH_CACHE_IDLE_FRAMES`] frames, so a finished graphics burst is reclaimed.
    /// - At the hard [`CPU_GLYPH_CACHE_LIMIT`]: keep only the visible set (plus in-flight jobs),
    ///   ignoring recency, so a single frame that references that many distinct glyphs still can't
    ///   grow the process without bound.
    ///
    /// `Pending` entries are always kept: a raster job for them is in flight and would otherwise be
    /// resurrected with a stale key.
    pub(super) fn evict_stale(
        entries: &mut FxHashMap<GlyphKey, RasterCacheEntry>,
        visible: &FxHashSet<GlyphKey>,
        frame: u64,
    ) {
        if entries.len() >= CPU_GLYPH_CACHE_LIMIT {
            entries.retain(|key, entry| {
                visible.contains(key) || matches!(entry, RasterCacheEntry::Pending)
            });
        } else if entries.len() > CPU_GLYPH_CACHE_IDLE_WATERMARK {
            entries.retain(|key, entry| match entry {
                RasterCacheEntry::Pending => true,
                RasterCacheEntry::Ready { last_used, .. } => {
                    visible.contains(key)
                        || frame.wrapping_sub(*last_used) <= CPU_GLYPH_CACHE_IDLE_FRAMES
                }
            });
        }
    }

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
        let frame = self.frame.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let mut queued = Vec::new();
        let mut pending: usize = 0;
        {
            let mut entries = self.entries.lock();
            Self::evict_stale(&mut entries, &keys, frame);
            for key in keys {
                match entries.get_mut(&key) {
                    Some(RasterCacheEntry::Ready { last_used, .. }) => *last_used = frame,
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

    pub(super) fn ready(&self, key: &GlyphKey) -> Option<Arc<RasterizedGlyph>> {
        let frame = self.frame.load(Ordering::Relaxed);
        match self.entries.lock().get_mut(key) {
            Some(RasterCacheEntry::Ready { glyph, last_used }) => {
                // A glyph actually drawn this frame is live; refresh recency so idle reclamation in
                // a later `prepare_terminal` does not drop something still on screen.
                *last_used = frame;
                Some(Arc::clone(glyph))
            }
            _ => None,
        }
    }

    pub(super) fn install_synchronous(
        &self,
        key: GlyphKey,
        rasterized: Arc<RasterizedGlyph>,
        elapsed: Duration,
    ) {
        let last_used = self.frame.load(Ordering::Relaxed);
        self.entries.lock().insert(
            key,
            RasterCacheEntry::Ready {
                glyph: rasterized,
                last_used,
            },
        );
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

pub(super) fn glyph_raster_worker(
    receiver: Arc<Mutex<Receiver<GlyphKey>>>,
    entries: Arc<Mutex<FxHashMap<GlyphKey, RasterCacheEntry>>>,
    counters: Arc<GlyphRasterCounters>,
    frame: Arc<AtomicU64>,
) {
    loop {
        let Ok(key) = receiver.lock().recv() else {
            return;
        };
        let started = Instant::now();
        let rasterized = Arc::new(rasterize_glyph(&key));
        let elapsed = started.elapsed();
        let last_used = frame.load(Ordering::Relaxed);
        entries.lock().insert(
            key,
            RasterCacheEntry::Ready {
                glyph: rasterized,
                last_used,
            },
        );
        counters.pending.fetch_sub(1, Ordering::Relaxed);
        counters.background_jobs.fetch_add(1, Ordering::Relaxed);
        counters
            .background_nanos
            .fetch_add(duration_nanos(elapsed), Ordering::Relaxed);
    }
}

pub(super) fn duration_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy)]
pub(super) struct AtlasEntry {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) width: usize,
    pub(super) height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum GlyphTextureKind {
    Mask = 1,
    Color = 2,
}

#[derive(Clone, Copy)]
pub(super) struct CachedGlyph {
    pub(super) entry: AtlasEntry,
    pub(super) kind: GlyphTextureKind,
    pub(super) offset: [i16; 2],
}

pub(super) struct GlyphLookup {
    pub(super) glyph: CachedGlyph,
    pub(super) hit: bool,
    pub(super) raster_time: Duration,
}

pub(super) struct RasterizedImage {
    pub(super) pixels: Vec<u8>,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) offset: [i16; 2],
}

pub(super) enum RasterizedGlyph {
    Mask(RasterizedImage),
    Color(RasterizedImage),
}

impl RasterizedGlyph {
    pub(super) fn exact_mask(pixels: Vec<u8>, width: usize, height: usize) -> Self {
        Self::Mask(RasterizedImage {
            pixels,
            width,
            height,
            offset: [0, 0],
        })
    }
}

#[derive(Default)]
pub(super) struct ShelfAllocator {
    pub(super) next_x: usize,
    pub(super) next_y: usize,
    pub(super) row_height: usize,
}

impl ShelfAllocator {
    pub(super) fn allocate(
        &mut self,
        width: usize,
        height: usize,
        atlas_size: usize,
    ) -> Option<AtlasEntry> {
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

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

pub(super) struct GlyphAtlas {
    pub(super) mask_texture: Texture,
    pub(super) color_texture: Option<Texture>,
    pub(super) empty_color_texture: Texture,
    pub(super) entries: FxHashMap<GlyphKey, CachedGlyph>,
    pub(super) mask_allocator: ShelfAllocator,
    pub(super) color_allocator: ShelfAllocator,
}

impl GlyphAtlas {
    pub(super) fn new(device: &Device) -> Self {
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

    pub(super) fn get_or_insert(
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

    pub(super) fn color_texture(&self) -> &Texture {
        self.color_texture
            .as_ref()
            .unwrap_or(&self.empty_color_texture)
    }

    pub(super) fn reset(&mut self) {
        self.entries.clear();
        self.mask_allocator.reset();
        self.color_allocator.reset();
    }
}

pub(super) fn atlas_texture(device: &Device, size: usize, format: MTLPixelFormat) -> Texture {
    let descriptor = TextureDescriptor::new();
    descriptor.set_width(size as u64);
    descriptor.set_height(size as u64);
    descriptor.set_pixel_format(format);
    descriptor.set_storage_mode(MTLStorageMode::Shared);
    descriptor.set_usage(MTLTextureUsage::ShaderRead);
    device.new_texture(&descriptor)
}

pub(super) fn atlas_region(entry: AtlasEntry) -> MTLRegion {
    MTLRegion::new_2d(
        entry.x as u64,
        entry.y as u64,
        entry.width as u64,
        entry.height as u64,
    )
}
