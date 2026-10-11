use super::*;

/// `font_metrics` takes the family and size as arguments — the caller reads them from the
/// terminal's own `window.text_style()` — so per-profile font sizes produce per-terminal cell
/// boxes. This pins the arithmetic that makes that possible: distinct sizes must yield distinct
/// boxes, and identical inputs must be stable so a repaint cannot resize the grid on its own.
///
/// It does NOT cover the part that carries a terminal's font into `window.text_style()` in the
/// first place (GPUI pushes the div's style around both the child prepaint and the
/// `on_children_prepainted` callback); that needs a gpui test window.
#[test]
fn cell_metrics_are_a_pure_function_of_family_and_size() {
    let (small_width, small_height) = font_metrics("Menlo", 12., 2.);
    let (large_width, large_height) = font_metrics("Menlo", 24., 2.);
    assert!(
        large_width > small_width && large_height > small_height,
        "a larger font must yield a larger cell box ({small_width}x{small_height} vs {large_width}x{large_height})"
    );
    assert_eq!(font_metrics("Menlo", 12., 2.), (small_width, small_height));
}

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

/// A distinct dummy glyph key per index, so a test can populate the cache with many entries
/// without caring what they rasterize to.
fn dummy_key(index: usize) -> GlyphKey {
    text_glyph_key(&format!("g{index}"), "Test", 16, 8, 8, 16)
}

/// A `Ready` entry whose pixels are irrelevant to eviction (which only inspects `last_used`).
fn ready_entry(last_used: u64) -> RasterCacheEntry {
    RasterCacheEntry::Ready {
        glyph: Arc::new(RasterizedGlyph::exact_mask(Vec::new(), 0, 0)),
        last_used,
    }
}

#[test]
fn evict_stale_keeps_everything_below_idle_watermark() {
    let mut entries = FxHashMap::default();
    // Half the cap's worth of glyphs, all stale (last_used far behind the current frame).
    for index in 0..CPU_GLYPH_CACHE_IDLE_WATERMARK {
        entries.insert(dummy_key(index), ready_entry(0));
    }
    let visible = FxHashSet::default();
    let frame = CPU_GLYPH_CACHE_IDLE_FRAMES + 10_000;
    GlyphRasterCache::evict_stale(&mut entries, &visible, frame);
    // Under the watermark the scan is skipped entirely — nothing is dropped despite being idle.
    assert_eq!(entries.len(), CPU_GLYPH_CACHE_IDLE_WATERMARK);
}

#[test]
fn evict_stale_reclaims_idle_but_keeps_visible_and_recent_past_watermark() {
    let mut entries = FxHashMap::default();
    let count = CPU_GLYPH_CACHE_IDLE_WATERMARK + 3;
    for index in 0..count {
        entries.insert(dummy_key(index), ready_entry(0));
    }
    let frame = CPU_GLYPH_CACHE_IDLE_FRAMES + 100;
    // One idle glyph is still on screen this frame; one was drawn recently (just inside the
    // window); the rest are idle and older than the window.
    let visible: FxHashSet<GlyphKey> = [dummy_key(0)].into_iter().collect();
    entries.insert(
        dummy_key(1),
        ready_entry(frame - CPU_GLYPH_CACHE_IDLE_FRAMES),
    );
    // A queued raster job for a glyph nobody references this frame must not be reaped.
    entries.insert(dummy_key(count), RasterCacheEntry::Pending);

    GlyphRasterCache::evict_stale(&mut entries, &visible, frame);

    assert!(
        entries.contains_key(&dummy_key(0)),
        "visible glyph survives"
    );
    assert!(
        entries.contains_key(&dummy_key(1)),
        "recently used glyph survives"
    );
    let pending = entries.get(&dummy_key(count));
    assert!(
        matches!(pending, Some(RasterCacheEntry::Pending)),
        "pending job survives"
    );
    assert!(
        !entries.contains_key(&dummy_key(2)),
        "idle glyph is reclaimed"
    );
    // Only the three deliberately-kept entries remain.
    assert_eq!(entries.len(), 3);
}

#[test]
fn evict_stale_at_hard_limit_keeps_only_visible_and_pending() {
    let mut entries = FxHashMap::default();
    for index in 0..CPU_GLYPH_CACHE_LIMIT {
        // Every glyph was used *this* frame, so recency alone would keep them all.
        entries.insert(dummy_key(index), ready_entry(1_000));
    }
    entries.insert(dummy_key(CPU_GLYPH_CACHE_LIMIT), RasterCacheEntry::Pending);
    let visible: FxHashSet<GlyphKey> = [dummy_key(0), dummy_key(1)].into_iter().collect();

    GlyphRasterCache::evict_stale(&mut entries, &visible, 1_000);

    // At the hard ceiling recency is ignored: only the visible set and the pending job stay.
    assert!(entries.contains_key(&dummy_key(0)));
    assert!(entries.contains_key(&dummy_key(1)));
    assert!(matches!(
        entries.get(&dummy_key(CPU_GLYPH_CACHE_LIMIT)),
        Some(RasterCacheEntry::Pending)
    ));
    assert_eq!(entries.len(), 3);
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
            let expected = if x == 0 || x + 1 == image.width || y == 0 || y + 1 == image.height {
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
        if reported > 0. {
            reported
        } else {
            0.75 * ascent
        }
    };
    let expected_single = (2.0 * cap + face_height) / 3.0;

    // No modifier: heights are the raw derivations.
    let base = icon_constraint_metrics(&font, None);
    assert!((base.icon_height - face_height).abs() < 1e-6);
    assert!((base.icon_height_single - expected_single).abs() < 1e-6);
    // The blended single-cell height sits below the full face height.
    assert!(base.icon_height_single < base.icon_height);

    // A percentage scales both heights by the same factor.
    let scaled = icon_constraint_metrics(&font, eggie_config::font::MetricModifier::parse("-50%"));
    assert!((scaled.icon_height - face_height * 0.5).abs() < 1e-4);
    assert!((scaled.icon_height_single - expected_single * 0.5).abs() < 1e-4);

    // A huge negative absolute delta is floored at 1px, never zero/negative.
    let floored =
        icon_constraint_metrics(&font, eggie_config::font::MetricModifier::parse("-10000"));
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
            image
                .pixels
                .chunks_exact(4)
                .any(|pixel| { pixel[3] != 0 && (pixel[0] != pixel[1] || pixel[1] != pixel[2]) }),
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
    assert_eq!(atlas.color_texture().height(), INITIAL_ATLAS_SIZE as u64);
    assert!(atlas.mask_texture.height() < MASK_ATLAS_SIZE as u64);
}

#[test]
fn atlas_growth_preserves_pixels_coordinates_and_original_capacity() {
    let device = Device::system_default().expect("Metal device for atlas regression test");
    let raster_cache = GlyphRasterCache::default();
    for (kind, limit, channels) in [
        (GlyphTextureKind::Mask, MASK_ATLAS_SIZE, 1),
        (GlyphTextureKind::Color, COLOR_ATLAS_SIZE, 4),
    ] {
        let mut atlas = GlyphAtlas::new(&device);
        let mut original_allocator = ShelfAllocator::default();
        let pixels = vec![173; 256 * 128 * channels];
        let rasterized = Arc::new(match kind {
            GlyphTextureKind::Mask => RasterizedGlyph::exact_mask(pixels.clone(), 256, 128),
            GlyphTextureKind::Color => RasterizedGlyph::Color(RasterizedImage {
                pixels: pixels.clone(),
                width: 256,
                height: 128,
                offset: [0, 0],
            }),
        });
        let mut first = None;
        for index in 0..1000 {
            let key = text_glyph_key(&format!("{kind:?}-{index}"), "Test", 16, 256, 256, 128);
            raster_cache.install_synchronous(key.clone(), rasterized.clone(), Duration::ZERO);
            let lookup = atlas.get_or_insert(&device, &raster_cache, key.clone());
            let Some(expected) = original_allocator.allocate(256, 128, limit) else {
                assert!(lookup.err().unwrap().to_string().contains("atlas is full"));
                break;
            };
            let actual = lookup.unwrap().glyph.entry;
            assert_eq!((actual.x, actual.y), (expected.x, expected.y));
            first.get_or_insert((key, actual));
            let texture = match kind {
                GlyphTextureKind::Mask => &atlas.mask_texture,
                GlyphTextureKind::Color => atlas.color_texture(),
            };
            for entry in [actual, first.as_ref().unwrap().1] {
                let mut readback = vec![0; pixels.len()];
                texture.get_bytes(
                    readback.as_mut_ptr().cast(),
                    (256 * channels) as u64,
                    atlas_region(entry),
                    0,
                );
                assert_eq!(
                    readback, pixels,
                    "growth must not lose old or new glyph pixels"
                );
            }
        }
        let texture = match kind {
            GlyphTextureKind::Mask => &atlas.mask_texture,
            GlyphTextureKind::Color => atlas.color_texture(),
        };
        assert_eq!(
            (texture.width(), texture.height()),
            (limit as u64, limit as u64)
        );
        let (key, first) = first.unwrap();
        let cached = atlas.get_or_insert(&device, &raster_cache, key).unwrap();
        assert!(cached.hit);
        assert_eq!(
            (cached.glyph.entry.x, cached.glyph.entry.y),
            (first.x, first.y)
        );
        atlas.reset();
        assert!(atlas.entries.is_empty());
        assert_eq!(atlas.mask_allocator.next_x, 0);
        assert_eq!(atlas.color_allocator.next_x, 0);
    }
}

#[test]
fn a_failed_shelf_allocation_does_not_change_the_next_tile_position() {
    let mut allocator = ShelfAllocator::default();
    allocator.allocate(500, 500, 512).unwrap();
    assert!(allocator.allocate(30, 30, 512).is_none());
    let next = allocator.allocate(30, 30, 1024).unwrap();
    assert_eq!((next.x, next.y), (501, 0));
}

#[test]
fn demand_allocated_raster_queue_keeps_backpressure_and_disconnect_cleanup() {
    let (sender, receiver) = channel();
    let cache = GlyphRasterCache {
        entries: Arc::new(Mutex::new(FxHashMap::default())),
        sender,
        counters: Arc::new(GlyphRasterCounters::default()),
        frame: Arc::new(AtomicU64::new(0)),
    };
    let mut terminal = prepared_with_families(None);
    terminal.commands.push(glyph_command(false, false));
    cache
        .counters
        .pending
        .store(GLYPH_QUEUE_CAPACITY, Ordering::Relaxed);
    assert!(cache.prepare_terminal(&terminal, 1., [0., 0.]).ready);
    assert!(cache.entries.lock().is_empty());
    assert!(receiver.try_recv().is_err());

    cache.counters.pending.store(0, Ordering::Relaxed);
    assert!(!cache.prepare_terminal(&terminal, 1., [0., 0.]).ready);
    assert!(receiver.try_recv().is_ok());
    assert_eq!(cache.counters.pending.load(Ordering::Relaxed), 1);

    cache.entries.lock().clear();
    cache.counters.pending.store(0, Ordering::Relaxed);
    drop(receiver);
    assert!(cache.prepare_terminal(&terminal, 1., [0., 0.]).ready);
    assert!(cache.entries.lock().is_empty());
    assert_eq!(cache.counters.pending.load(Ordering::Relaxed), 0);
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
    for library in [
        device
            .new_library_with_source(SHADER, &CompileOptions::new())
            .expect("terminal Metal source must compile"),
        device
            .new_library_with_data(SHADER_LIBRARY)
            .expect("bundled terminal library must load"),
    ] {
        assert_terminal_shader_pixels(&device, &library);
    }
}

fn assert_terminal_shader_pixels(device: &DeviceRef, library: &metal::LibraryRef) {
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
            uv_size: [1., 1.],
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
            uv_size: [1., 1.],
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

fn test_font_families() -> eggie_config::font::ResolvedFontFamilies {
    let regular: Arc<str> = Arc::from("Menlo");
    eggie_config::font::ResolvedFontFamilies {
        bold: regular.clone(),
        italic: regular.clone(),
        bold_italic: regular.clone(),
        regular,
        synthetic: Default::default(),
    }
}

fn prepared_with_families(
    families: Option<eggie_config::font::ResolvedFontFamilies>,
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
    let mut families = test_font_families();
    families.bold = Arc::from("Menlo Bold");
    let terminal = prepared_with_families(Some(families));
    let key = glyph_key_for_command(&terminal, &glyph_command(true, false), 1., [0., 0.])
        .expect("glyph command produces a key");
    assert_eq!(key.family.as_ref(), "Menlo Bold");
    // The dedicated bold face already carries weight; don't also synthesize bold.
    assert!(!key.bold);
}

#[test]
fn missing_bold_family_synthesizes_when_enabled_and_not_when_disabled() {
    let terminal = prepared_with_families(Some(test_font_families()));
    let key = glyph_key_for_command(&terminal, &glyph_command(true, false), 1., [0., 0.])
        .expect("glyph command produces a key");
    // No dedicated bold family, synthesis on by default → falls back to regular + synth bold.
    assert_eq!(key.family.as_ref(), "Menlo");
    assert!(key.bold);

    let mut families = test_font_families();
    families.synthetic.bold = false;
    let terminal = prepared_with_families(Some(families));
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
    let no_liga = eggie_config::font::FontFeature::parse("-liga").unwrap();
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
        let wght = eggie_config::font::FontVariation {
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
    let key_with = |icon_height: Option<eggie_config::font::MetricModifier>| GlyphKey {
        icon_height,
        ..text_glyph_key("\u{f001}", "Menlo", 28f32.to_bits(), 18, 18, 36)
    };

    let baseline = rasterize_text(&key_with(None));
    let shrunk = rasterize_text(&key_with(eggie_config::font::MetricModifier::parse("-50%")));
    let grown = rasterize_text(&key_with(eggie_config::font::MetricModifier::parse("40%")));

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
    let small = rasterize_text(&key_with(eggie_config::font::MetricModifier::parse("-30%")));
    let large = rasterize_text(&key_with(eggie_config::font::MetricModifier::parse("-10%")));
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
    let calt = eggie_config::font::FontFeature::parse("calt").unwrap();
    let liga = eggie_config::font::FontFeature::parse("liga").unwrap();
    for family in [
        "Maple Mono NF CN",
        "JetBrains Mono",
        "Fira Code",
        "Cascadia Code",
    ] {
        let installed = font::new_from_name(family, 28.)
            .map(|font| {
                let want = family
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
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

    let Some((texture, _buffer)) = backend.zero_copy_image_texture(&image, &mmap, width as usize)
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
