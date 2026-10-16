use super::*;

pub(crate) struct MetalBackend {
    pub(super) device: Device,
    pub(super) pipeline: RenderPipelineState,
    pub(super) image_pipeline: RenderPipelineState,
    pub(super) sampler: SamplerState,
    pub(super) image_sampler: SamplerState,
    pub(super) atlas: GlyphAtlas,
    pub(super) images: FxHashMap<TerminalTextureKey, CachedTerminalImage>,
    pub(super) recycled_images: Vec<RecycledTerminalImage>,
    pub(super) image_clock: u64,
    pub(super) image_bytes: usize,
    pub(super) instances: Vec<Instance>,
}

pub(super) struct CachedTerminalImage {
    pub(super) texture: Texture,
    pub(super) bytes: usize,
    pub(super) last_used: u64,
    /// When present, `texture` is a zero-copy view over shared memory: the `MTLBuffer` wraps the
    /// mmap pages directly (`new_buffer_with_bytes_no_copy`), so the mmap and buffer must outlive
    /// the texture. Such textures alias a specific shm segment and must never be recycled for a
    /// different image.
    #[cfg(unix)]
    pub(super) zero_copy: Option<ZeroCopyBacking>,
}

/// Keeps a zero-copy image texture's Rust-side handles together with the cache entry. GPU-safe
/// lifetime of the mmap is guaranteed by the buffer's deallocator block (which owns its own
/// `Arc<Mmap>` clone and runs `munmap` only when Metal finally releases the buffer, i.e. after all
/// in-flight command buffers complete). This struct just parks the `Buffer` handle and a mmap
/// reference alongside the texture so they are not dropped earlier than the cache entry.
#[cfg(unix)]
pub(super) struct ZeroCopyBacking {
    pub(super) _buffer: Buffer,
    pub(super) _mmap: Arc<memmap2::Mmap>,
}

pub(super) struct RecycledTerminalImage {
    pub(super) texture: Texture,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// Whether to attempt zero-copy image upload (`new_buffer_with_bytes_no_copy` over the shm mmap,
/// skipping `replace_region`). On by default; the path degrades safely (falls back to
/// `replace_region` when the row stride is unaligned or the image is not shm-mapped) and the mmap's
/// GPU lifetime is bound to the buffer's deallocator. Set `EGGIE_IMAGE_ZERO_COPY=0` to force the
/// copy path.
#[cfg(unix)]
pub(super) fn image_zero_copy_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var_os("EGGIE_IMAGE_ZERO_COPY").is_none_or(|value| value != "0"))
}

impl MetalBackend {
    pub(crate) fn new(device: &DeviceRef) -> Result<Self> {
        let device = device.to_owned();
        let library = device
            .new_library_with_data(SHADER_LIBRARY)
            .map_err(|error| anyhow!("load terminal Metal shader: {error}"))?;
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

    pub(super) fn viewport(context: &MetalRenderContext<'_>) -> [f32; 2] {
        let viewport_size = context.viewport_size();
        [
            i32::from(viewport_size.width) as f32,
            i32::from(viewport_size.height) as f32,
        ]
    }

    pub(super) fn draw_text_batch(
        &self,
        context: &mut MetalRenderContext<'_>,
        instances: &mut Vec<Instance>,
        stats: &mut MetalFrameStats,
    ) -> Result<()> {
        if instances.is_empty() {
            return Ok(());
        }
        if context.opacity() < 1. {
            for instance in instances.iter_mut() {
                instance.color[3] *= context.opacity();
            }
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

    pub(super) fn draw_image_batch(
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
                color: [1., 1., 1., context.opacity()],
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

    pub(super) fn image_texture(
        &mut self,
        image: &TerminalImageData,
    ) -> Result<(Texture, usize, Duration)> {
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
    pub(super) fn zero_copy_image_texture(
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
        let deallocator =
            block::ConcreteBlock::new(move |_ptr: *const std::ffi::c_void, _len: NSUInteger| {
                // Hold the mapping alive until Metal releases this block; drop does the munmap.
                let _keep_alive = &backing;
            })
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

    pub(super) fn build_instance(
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
                Ok(Instance {
                    origin: [
                        pixel_origin[0] + f32::from(cached.offset[0]),
                        pixel_origin[1] + f32::from(cached.offset[1]),
                    ],
                    size: [cached.entry.width as f32, cached.entry.height as f32],
                    color: rgba_components(*color),
                    uv_origin: [cached.entry.x as f32, cached.entry.y as f32],
                    uv_size: [cached.entry.width as f32, cached.entry.height as f32],
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

pub(super) fn image_batch_end(
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
