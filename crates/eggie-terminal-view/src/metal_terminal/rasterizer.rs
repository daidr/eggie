use super::*;

pub(super) fn rasterize_glyph(key: &GlyphKey) -> RasterizedGlyph {
    let width = key.width as usize;
    let height = key.height as usize;
    if key.sprite
        && let Some(pixels) = key.text.chars().next().and_then(|character| {
            terminal_sprites::rasterize(character, width, height, key.box_thickness)
        })
    {
        return RasterizedGlyph::exact_mask(pixels, width, height);
    }
    rasterize_text(key)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FontStyleKey {
    pub(super) family: Arc<str>,
    pub(super) point_size_bits: u64,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) variations: Arc<[eggie_config::font::FontVariation]>,
}

#[derive(Default)]
pub(super) struct GlyphRasterizer {
    pub(super) fonts: FxHashMap<FontStyleKey, CTFont>,
}

impl GlyphRasterizer {
    pub(super) fn font(&mut self, key: &GlyphKey) -> CTFont {
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

    pub(super) fn rasterize_text(&mut self, key: &GlyphKey) -> RasterizedGlyph {
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

pub(super) fn line_uses_last_resort(line: &CTLine) -> bool {
    line.glyph_runs().iter().any(|run| {
        run.attributes().is_some_and(|attributes| {
            attributes
                .find(CFString::new("NSFont"))
                .and_then(|value| value.downcast::<CTFont>())
                .is_some_and(|font| font.postscript_name().contains("LastResort"))
        })
    })
}

pub(super) fn unknown_glyph_outline(width: usize, height: usize) -> RasterizedGlyph {
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

pub(super) fn rasterize_text(key: &GlyphKey) -> RasterizedGlyph {
    GLYPH_RASTERIZER.with(|rasterizer| rasterizer.borrow_mut().rasterize_text(key))
}

pub(super) fn is_single_cell_fraction(text: &str) -> bool {
    let mut characters = text.chars();
    let Some(character) = characters.next() else {
        return false;
    };
    characters.next().is_none()
        && matches!(character, '\u{00bc}'..='\u{00be}' | '\u{2150}'..='\u{215f}' | '\u{2189}')
}

pub(super) fn fit_font_to_cell(text: &str, font: &CTFont, width: usize, height: usize) -> CTFont {
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
pub(super) struct IconMetrics {
    pub(super) icon_height: f64,
    pub(super) icon_height_single: f64,
}

/// Derive the Nerd Font icon constraint heights from the *primary* styled face (not the symbol
/// fallback face, which often lacks a cap-height metric), then apply the `adjust-icon-height`
/// modifier to both. Values are in device px because `primary` is built at `font_size * scale`.
///
/// Mirrors `ghostty/src/font/Metrics.zig`: `icon_height = face_height`,
/// `icon_height_single = (2 * cap_height + face_height) / 3`, with the cap-height estimate
/// (`0.75 * ascent`) used when the face reports no usable cap height.
pub(super) fn icon_constraint_metrics(
    primary: &CTFont,
    modifier: Option<eggie_config::font::MetricModifier>,
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
        f64::from(eggie_config::font::ResolvedMetricAdjustments::adjust(
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
pub(super) fn icon_fit_cover1_scale(
    glyph_w: f64,
    glyph_h: f64,
    max_width: f64,
    target_height: f64,
) -> f64 {
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
pub(super) fn is_nerd_icon_codepoint(character: char) -> bool {
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
pub(super) fn scale_icon_font(
    primary: &CTFont,
    icon_face: &CTFont,
    modifier: eggie_config::font::MetricModifier,
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

pub(super) fn font_for_text(base_font: &CTFont, text: &str) -> CTFont {
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
pub(super) const NERD_FONT_SYMBOLS: &[u8] =
    include_bytes!("../../assets/SymbolsNerdFontMono-Regular.ttf");

thread_local! {
    /// Lazily-built descriptor for the embedded Nerd Font (per-thread; CoreText objects aren't Sync).
    /// `None` if registration failed. The outer Option marks "not yet attempted".
    static NERD_FONT_DESCRIPTOR: RefCell<Option<Option<core_text::font_descriptor::CTFontDescriptor>>> =
        const { RefCell::new(None) };
}

/// A `CTFont` for the embedded Nerd Font symbols at `pt_size`, or `None` if it couldn't be built.
pub(super) fn nerd_font_fallback(pt_size: f64) -> Option<CTFont> {
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
pub(super) fn nerd_font_covers(font: &CTFont, text: &str) -> bool {
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

pub(super) fn attributed_line(text: &str, font: &CTFont) -> CTLine {
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
pub(super) fn font_with_features(
    font: &CTFont,
    features: &[eggie_config::font::FontFeature],
) -> CTFont {
    if features.is_empty() {
        return font.clone();
    }
    let tag_key = unsafe { CFString::wrap_under_get_rule(kCTFontOpenTypeFeatureTag) };
    let value_key = unsafe { CFString::wrap_under_get_rule(kCTFontOpenTypeFeatureValue) };
    let settings: Vec<CFDictionary<CFString, core_foundation::base::CFType>> = features
        .iter()
        .map(|feature| {
            let tag = CFString::new(
                std::str::from_utf8(&feature.tag)
                    .unwrap_or("")
                    .trim_end_matches('\0'),
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
    let attributes = CFDictionary::from_CFType_pairs(&[(feature_key, settings_array.as_CFType())]);
    let base_descriptor = font.copy_descriptor();
    match base_descriptor.create_copy_with_attributes(attributes.to_untyped()) {
        Ok(descriptor) => font::new_from_descriptor(&descriptor, font.pt_size()),
        Err(_) => font.clone(),
    }
}

/// Apply variable-font axis settings (`wght`, `slnt`, …) to a font via a copied descriptor. The
/// `kCTFontVariationAttribute` value is a dictionary keyed by each axis's four-char tag encoded as a
/// big-endian `u32` identifier → the axis value. Empty list returns the font unchanged.
pub(super) fn font_with_variations(
    font: &CTFont,
    variations: &[eggie_config::font::FontVariation],
) -> CTFont {
    if variations.is_empty() {
        return font.clone();
    }
    let pairs: Vec<(CFNumber, CFNumber)> = variations
        .iter()
        .map(|variation| {
            let identifier = u32::from_be_bytes(variation.tag) as i64;
            (
                CFNumber::from(identifier),
                CFNumber::from(variation.value as f64),
            )
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
pub(super) fn shape_run_glyph_ids(
    text: &str,
    font_data: &[u8],
    face_index: u32,
    features: &[eggie_config::font::FontFeature],
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
    features: &[eggie_config::font::FontFeature],
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
pub(super) fn styled_font(
    family: &str,
    font_size: f32,
    bold: bool,
    italic: bool,
) -> Option<CTFont> {
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
pub(super) enum FontBytes {
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
pub(super) static FONT_DATA_CACHE: OnceLock<Mutex<FxHashMap<String, Option<FontFileData>>>> =
    OnceLock::new();

/// Read a `CTFont`'s backing font file bytes and its face index within the file. Cached per
/// PostScript name. Returns `None` if the font isn't backed by a readable file on disk.
pub(super) fn font_file_data(font: &CTFont) -> Option<FontFileData> {
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
pub(super) fn load_font_file(font: &CTFont) -> Option<FontFileData> {
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
pub(super) fn postscript_name_matches(
    face: &rustybuzz::ttf_parser::Face<'_>,
    target: &str,
) -> bool {
    face.names()
        .into_iter()
        .filter(|name| name.name_id == rustybuzz::ttf_parser::name_id::POST_SCRIPT_NAME)
        .filter_map(|name| name.to_string())
        .any(|name| name == target)
}

pub(super) fn line_uses_only_color_glyphs(line: &CTLine) -> bool {
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
pub(super) struct RasterPlacement {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) offset: [i16; 2],
    pub(super) text_x: f64,
    pub(super) baseline: f64,
}

/// Baseline (in the bottom-origin bitmap context) that vertically centers `ascent + descent`
/// within a `cell_height`-tall cell, then applies the optional `adjust-font-baseline` modifier
/// (pre-scaled to device px; positive raises the glyph). Shared by the `CTLine` text path and the
/// pre-shaped `rasterize_glyph_id` path so both sit on the same line. The float operation order is
/// load-bearing for bit-identical output and must not change.
pub(super) fn resolve_baseline(
    ascent: f64,
    descent: f64,
    cell_height: usize,
    adjustment: Option<eggie_config::font::MetricModifier>,
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
pub(super) struct OverhangTile {
    pub(super) left: usize,
    pub(super) tile_width: usize,
    pub(super) offset: [i16; 2],
}

pub(super) fn overhang_tile(
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

pub(super) fn raster_placement(
    line: &CTLine,
    context: &CGContext,
    cell_width: usize,
    cell_height: usize,
    confine_to_cell: bool,
    baseline_adjustment: Option<eggie_config::font::MetricModifier>,
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

pub(super) fn mask_context(width: usize, height: usize, thicken: Option<u8>) -> CGContext {
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

pub(super) fn rasterize_mask_line(
    line: &CTLine,
    width: usize,
    height: usize,
    confine_to_cell: bool,
    baseline_adjustment: Option<eggie_config::font::MetricModifier>,
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
pub(super) fn rasterize_glyph_id(
    font: &CTFont,
    glyph_id: u16,
    cell_width: usize,
    cell_height: usize,
    baseline_adjustment: Option<eggie_config::font::MetricModifier>,
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

pub(super) fn color_context(width: usize, height: usize, thicken: Option<u8>) -> CGContext {
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

pub(super) fn rasterize_color_line(
    line: &CTLine,
    width: usize,
    height: usize,
    baseline_adjustment: Option<eggie_config::font::MetricModifier>,
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

pub(super) fn configure_text_context(context: &CGContext, thicken: Option<u8>) {
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

pub(super) fn draw_positioned_line(line: &CTLine, context: &CGContext, placement: RasterPlacement) {
    context.set_text_position(placement.text_x, placement.baseline);
    line.draw(context);
}

pub(super) fn rgba_components(color: u32) -> [f32; 4] {
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
