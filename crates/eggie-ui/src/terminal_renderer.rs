use crate::{
    app::EggieApp,
    input_latency::InputLatencyTracker,
    metal_terminal::{GlyphRasterCache, MetalBackend},
    settings::{ResolvedMetricAdjustments, TerminalTheme, minimum_contrast_rgb},
    terminal_sprites::{self, SpriteKind},
};
use alacritty_terminal::term::cell::Flags;
use eggie_protocol::{
    TerminalCell, TerminalColor, TerminalCursorShape, TerminalImageKey, TerminalLinkRange,
    TerminalSearchMatch, TerminalSearchResult, TerminalSnapshot,
};
use gpui::{
    App, Bounds, Element, ElementId, FocusHandle, GlobalElementId, InputHandler,
    InspectorElementId, IntoElement, LayoutId, MetalPrimitiveRenderer, MetalRenderContext, Pixels,
    Style, UTF16Selection, WeakEntity, Window, point, px, relative, size,
};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use std::{ops::Range, sync::Arc, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TERMINAL_COLOR_COUNT: usize = 269;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TerminalPoint {
    pub(crate) line: u16,
    pub(crate) column: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalSelection {
    pub(crate) anchor: TerminalPoint,
    pub(crate) head: TerminalPoint,
}

impl TerminalSelection {
    pub(crate) fn ordered(self) -> (TerminalPoint, TerminalPoint) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalImeState {
    pub(crate) text: String,
    pub(crate) selected_range: Range<usize>,
}

#[derive(Clone)]
pub(crate) struct TerminalInputContext {
    app: WeakEntity<EggieApp>,
    focus: FocusHandle,
}

impl TerminalInputContext {
    pub(crate) fn new(app: WeakEntity<EggieApp>, focus: FocusHandle) -> Self {
        Self { app, focus }
    }
}

pub(crate) struct TerminalRenderOptions {
    theme: &'static TerminalTheme,
    accent: u32,
    minimum_contrast: f32,
    selection: Option<TerminalSelection>,
    ime: Option<TerminalImeState>,
    input: Option<TerminalInputContext>,
    input_latency: InputLatencyTracker,
    search: Option<TerminalSearchHighlights>,
    url_hover: Option<TerminalLinkRange>,
    /// Whether the cursor should be painted this frame. `false` during the "off" half of a blink.
    cursor_visible: bool,
    /// Font-metric adjustments (`adjust-*`) applied to decoration/cursor/box positions.
    adjustments: ResolvedMetricAdjustments,
    /// Resolved per-style font families + synthetic-style policy.
    font_families: Option<crate::settings::ResolvedFontFamilies>,
    /// OpenType feature overrides (ligature toggle + user features) applied during shaping.
    font_features: Arc<[crate::settings::FontFeature]>,
    /// Master ligature toggle; false takes the cheap per-grapheme path.
    ligatures: bool,
    /// Break shaping runs at the cursor cell so the character under the cursor renders un-ligated.
    shaping_break_cursor: bool,
    /// Variable-font axis settings applied to the face.
    font_variations: Arc<[crate::settings::FontVariation]>,
    /// macOS font-smoothing thickening: `None` = off, `Some(strength)` = on at 0–255.
    font_thicken: Option<u8>,
    /// Per-codepoint-range family overrides (`font-codepoint-map`).
    font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
}

/// Viewport-relative search highlights to overlay on the terminal grid. `active` is the currently
/// selected match (drawn in a stronger color); `matches` are all other visible matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TerminalSearchHighlights {
    pub(crate) active: Option<TerminalSearchMatch>,
    pub(crate) matches: Vec<TerminalSearchMatch>,
}

impl TerminalSearchHighlights {
    pub(crate) fn from_result(result: &TerminalSearchResult) -> Self {
        Self {
            active: result.active,
            matches: result.matches.clone(),
        }
    }

    fn is_empty(&self) -> bool {
        self.active.is_none() && self.matches.is_empty()
    }
}

impl TerminalRenderOptions {
    pub(crate) fn new(
        theme: &'static TerminalTheme,
        accent: u32,
        minimum_contrast: f32,
        selection: Option<TerminalSelection>,
        ime: Option<TerminalImeState>,
        input: Option<TerminalInputContext>,
        input_latency: InputLatencyTracker,
    ) -> Self {
        Self {
            theme,
            accent,
            minimum_contrast,
            selection,
            ime,
            input,
            input_latency,
            search: None,
            url_hover: None,
            cursor_visible: true,
            adjustments: ResolvedMetricAdjustments::default(),
            font_families: None,
            font_features: Arc::from(&[][..]),
            ligatures: false,
            shaping_break_cursor: true,
            font_variations: Arc::from(&[][..]),
            font_thicken: None,
            font_codepoint_map: Arc::from(&[][..]),
        }
    }

    pub(crate) fn with_search(mut self, search: Option<TerminalSearchHighlights>) -> Self {
        self.search = search.filter(|search| !search.is_empty());
        self
    }

    pub(crate) fn with_url_hover(mut self, url_hover: Option<TerminalLinkRange>) -> Self {
        self.url_hover = url_hover;
        self
    }

    pub(crate) fn with_cursor_visible(mut self, cursor_visible: bool) -> Self {
        self.cursor_visible = cursor_visible;
        self
    }

    pub(crate) fn with_metric_adjustments(
        mut self,
        adjustments: ResolvedMetricAdjustments,
    ) -> Self {
        self.adjustments = adjustments;
        self
    }

    pub(crate) fn with_font_families(
        mut self,
        font_families: crate::settings::ResolvedFontFamilies,
    ) -> Self {
        self.font_families = Some(font_families);
        self
    }

    pub(crate) fn with_font_features(
        mut self,
        font_features: Arc<[crate::settings::FontFeature]>,
        ligatures: bool,
        shaping_break_cursor: bool,
    ) -> Self {
        self.font_features = font_features;
        self.ligatures = ligatures;
        self.shaping_break_cursor = shaping_break_cursor;
        self
    }

    pub(crate) fn with_font_variations(
        mut self,
        font_variations: Arc<[crate::settings::FontVariation]>,
        font_thicken: Option<u8>,
    ) -> Self {
        self.font_variations = font_variations;
        self.font_thicken = font_thicken;
        self
    }

    pub(crate) fn with_codepoint_map(
        mut self,
        font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
    ) -> Self {
        self.font_codepoint_map = font_codepoint_map;
        self
    }
}

#[derive(Clone)]
pub(crate) struct MetalTerminalRenderer {
    state: Arc<Mutex<RendererState>>,
    preparation_cache: Arc<Mutex<PreparationCache>>,
    image_cache: Arc<Mutex<FxHashMap<TerminalTextureKey, Arc<TerminalImageData>>>>,
    glyph_raster_cache: Arc<GlyphRasterCache>,
}

#[derive(Default)]
struct RendererState {
    backend: Option<MetalBackend>,
    error_reported: bool,
}

#[derive(Default)]
struct PreparationCache {
    entries: FxHashMap<eggie_domain::SessionId, CachedPreparation>,
    last_ready: FxHashMap<eggie_domain::SessionId, ReadyPreparation>,
}

struct CachedPreparation {
    key: PreparationKey,
    terminal: Arc<PreparedMetalTerminal>,
}

struct ReadyPreparation {
    layout: RasterLayoutKey,
    terminal: Arc<PreparedMetalTerminal>,
}

#[derive(Clone, Debug, PartialEq)]
struct PreparationKey {
    revision: u64,
    viewport_width_bits: u32,
    viewport_height_bits: u32,
    cell_width_bits: u32,
    line_height_bits: u32,
    scale_factor_bits: u32,
    family: Arc<str>,
    font_size_bits: u32,
    theme: usize,
    accent: u32,
    minimum_contrast_bits: u32,
    selection: Option<TerminalSelection>,
    ime: Option<TerminalImeState>,
    search: Option<TerminalSearchHighlights>,
    url_hover: Option<TerminalLinkRange>,
    cursor_visible: bool,
    adjustments: ResolvedMetricAdjustments,
    font_families: Option<crate::settings::ResolvedFontFamilies>,
    font_features: Arc<[crate::settings::FontFeature]>,
    ligatures: bool,
    shaping_break_cursor: bool,
    font_variations: Arc<[crate::settings::FontVariation]>,
    font_thicken: Option<u8>,
    font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
}

#[derive(Clone, Debug, PartialEq)]
struct RasterLayoutKey {
    viewport_origin_x_bits: u32,
    viewport_origin_y_bits: u32,
    viewport_width_bits: u32,
    viewport_height_bits: u32,
    cell_width_bits: u32,
    line_height_bits: u32,
    scale_factor_bits: u32,
    family: Arc<str>,
    font_size_bits: u32,
}

impl Default for MetalTerminalRenderer {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(RendererState::default())),
            preparation_cache: Arc::new(Mutex::new(PreparationCache::default())),
            image_cache: Arc::new(Mutex::new(FxHashMap::default())),
            glyph_raster_cache: Arc::new(GlyphRasterCache::default()),
        }
    }
}

impl MetalTerminalRenderer {
    pub(crate) fn render(
        &self,
        snapshot: Arc<TerminalSnapshot>,
        options: TerminalRenderOptions,
    ) -> MetalTerminalElement {
        MetalTerminalElement {
            renderer: self.clone(),
            snapshot,
            theme: options.theme,
            accent: options.accent,
            minimum_contrast: options.minimum_contrast,
            selection: options.selection,
            ime: options.ime,
            input: options.input,
            input_latency: options.input_latency,
            search: options.search,
            url_hover: options.url_hover,
            cursor_visible: options.cursor_visible,
            adjustments: options.adjustments,
            font_families: options.font_families,
            font_features: options.font_features,
            ligatures: options.ligatures,
            shaping_break_cursor: options.shaping_break_cursor,
            font_variations: options.font_variations,
            font_thicken: options.font_thicken,
            font_codepoint_map: options.font_codepoint_map,
        }
    }

    pub(crate) fn has_image(
        &self,
        session_id: eggie_domain::SessionId,
        image: TerminalImageKey,
    ) -> bool {
        self.image_cache
            .lock()
            .contains_key(&TerminalTextureKey { session_id, image })
    }

    pub(crate) fn install_image(&self, image: TerminalImageData) {
        let session_id = image.key.session_id;
        self.image_cache.lock().insert(image.key, Arc::new(image));
        self.preparation_cache.lock().entries.remove(&session_id);
    }

    pub(crate) fn retain_snapshot_images(&self, snapshot: &TerminalSnapshot) {
        let live = snapshot
            .images
            .iter()
            .map(|image| TerminalTextureKey {
                session_id: snapshot.session_id,
                image: image.key,
            })
            .collect::<std::collections::HashSet<_>>();
        self.image_cache
            .lock()
            .retain(|key, _| key.session_id != snapshot.session_id || live.contains(key));
        if let Some(backend) = self.state.lock().backend.as_mut() {
            backend.retain_session_images(snapshot.session_id, &live);
        }
    }
}

pub(crate) struct MetalTerminalElement {
    renderer: MetalTerminalRenderer,
    snapshot: Arc<TerminalSnapshot>,
    theme: &'static TerminalTheme,
    accent: u32,
    minimum_contrast: f32,
    selection: Option<TerminalSelection>,
    ime: Option<TerminalImeState>,
    input: Option<TerminalInputContext>,
    input_latency: InputLatencyTracker,
    search: Option<TerminalSearchHighlights>,
    url_hover: Option<TerminalLinkRange>,
    cursor_visible: bool,
    adjustments: ResolvedMetricAdjustments,
    font_families: Option<crate::settings::ResolvedFontFamilies>,
    font_features: Arc<[crate::settings::FontFeature]>,
    ligatures: bool,
    shaping_break_cursor: bool,
    font_variations: Arc<[crate::settings::FontVariation]>,
    font_thicken: Option<u8>,
    font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
}

#[derive(Clone, Debug)]
pub(crate) enum MetalCommand {
    Rect {
        rect: [f32; 4],
        color: u32,
    },
    Glyph {
        rect: [f32; 4],
        text: Arc<str>,
        color: u32,
        bold: bool,
        italic: bool,
        sprite: bool,
        background: u32,
        minimum_contrast: f32,
        minimum_contrast_disabled: bool,
        /// A pre-shaped glyph id to draw instead of re-shaping `text`. Set for cells inside a
        /// ligature run so the contextually-substituted glyph (e.g. the joined half of `=>`)
        /// renders; `None` means shape `text` normally.
        glyph_id: Option<u16>,
    },
    Image {
        image: Arc<TerminalImageData>,
        rect: [f32; 4],
        source: [f32; 4],
    },
}

pub(crate) struct PreparedMetalTerminal {
    pub(crate) commands: Vec<MetalCommand>,
    pub(crate) family: Arc<str>,
    pub(crate) font_size: f32,
    pub(crate) cell_width: f32,
    pub(crate) last_input_sequence: u64,
    /// `adjust-font-baseline` modifier (logical px), applied to text glyph vertical placement.
    pub(crate) baseline_adjustment: Option<crate::settings::MetricModifier>,
    /// `adjust-box-thickness` modifier (logical px), applied to box-drawing sprite stroke width.
    pub(crate) box_thickness_adjustment: Option<crate::settings::MetricModifier>,
    /// `adjust-icon-height` modifier (logical px), applied to Nerd Font icon glyph scaling.
    pub(crate) icon_height_adjustment: Option<crate::settings::MetricModifier>,
    /// Resolved per-style families + synthetic-style policy. `None` keeps the legacy behavior of
    /// deriving bold/italic from the single `family` via CoreText trait synthesis.
    pub(crate) font_families: Option<crate::settings::ResolvedFontFamilies>,
    /// OpenType feature overrides applied when shaping text glyphs. Empty = font defaults.
    pub(crate) font_features: Arc<[crate::settings::FontFeature]>,
    /// Variable-font axis settings applied to the face. Empty = font defaults.
    pub(crate) font_variations: Arc<[crate::settings::FontVariation]>,
    /// macOS font-smoothing thickening: `None` = off, `Some(strength)` = on at 0–255.
    pub(crate) font_thicken: Option<u8>,
    /// Per-codepoint-range family overrides (`font-codepoint-map`). Empty = none.
    pub(crate) font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TerminalTextureKey {
    pub(crate) session_id: eggie_domain::SessionId,
    pub(crate) image: TerminalImageKey,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalImageData {
    pub(crate) key: TerminalTextureKey,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Arc<Vec<u8>>,
}

struct TerminalMetalPrimitive {
    state: Arc<Mutex<RendererState>>,
    terminal: Arc<PreparedMetalTerminal>,
    glyph_raster_cache: Arc<GlyphRasterCache>,
    scale_factor: f32,
    input_latency: InputLatencyTracker,
    session_id: eggie_domain::SessionId,
    last_input_sequence: u64,
}

impl MetalPrimitiveRenderer for TerminalMetalPrimitive {
    fn paint(&self, context: &mut MetalRenderContext<'_>) {
        let started = Instant::now();
        {
            let mut state = self.state.lock();
            if state.backend.is_none() {
                match MetalBackend::new(context.device()) {
                    Ok(backend) => state.backend = Some(backend),
                    Err(error) => {
                        if !state.error_reported {
                            eprintln!("failed to initialize Metal terminal renderer: {error:#}");
                            state.error_reported = true;
                        }
                        return;
                    }
                }
            }
            let result = state
                .backend
                .as_mut()
                .expect("terminal backend was initialized")
                .render(
                    context,
                    self.scale_factor,
                    &self.terminal,
                    &self.glyph_raster_cache,
                );
            let mut stats = match result {
                Ok(stats) => stats,
                Err(error) => {
                    if !state.error_reported {
                        eprintln!("failed to render terminal with Metal: {error:#}");
                        state.error_reported = true;
                    }
                    Default::default()
                }
            };
            let raster = self.glyph_raster_cache.take_stats();
            stats.background_raster_jobs = raster.background_jobs;
            stats.background_raster_time = raster.background_time;
            stats.synchronous_raster_jobs = raster.synchronous_jobs;
            stats.synchronous_raster_time = raster.synchronous_time;
            stats.pending_rasters = raster.pending;
            self.input_latency.record_metal(
                self.session_id,
                self.last_input_sequence,
                started.elapsed(),
                stats,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedCell {
    foreground: u32,
    background: u32,
    underline: u32,
    flags: Flags,
}

#[derive(Clone)]
struct TerminalPalette {
    appearance: eggie_protocol::TerminalAppearance,
    overrides: [Option<u32>; TERMINAL_COLOR_COUNT],
}

impl TerminalPalette {
    fn new(snapshot: &TerminalSnapshot, theme: &TerminalTheme) -> Self {
        let mut overrides = [None; TERMINAL_COLOR_COUNT];
        for entry in &snapshot.color_overrides {
            if let Some(slot) = overrides.get_mut(entry.index as usize) {
                *slot = Some(entry.color);
            }
        }
        Self {
            appearance: theme.appearance(),
            overrides,
        }
    }

    fn color(&self, index: usize) -> u32 {
        self.overrides
            .get(index)
            .copied()
            .flatten()
            .or_else(|| self.appearance.color(index).map(with_alpha))
            .unwrap_or_else(|| with_alpha(self.appearance.foreground))
    }

    fn foreground(&self, color: &TerminalColor, flags: Flags) -> u32 {
        match *color {
            TerminalColor::Rgb(color) if flags.contains(Flags::DIM) => dim_rgba(color),
            TerminalColor::Rgb(color) => color,
            TerminalColor::Indexed(index) => {
                let index = if flags & Flags::DIM_BOLD == Flags::BOLD && index <= 7 {
                    index as usize + 8
                } else {
                    index as usize
                };
                self.color(index)
            }
            TerminalColor::Named(index) => {
                let dim_bold = flags & Flags::DIM_BOLD;
                let index = if index == 256 && dim_bold == Flags::DIM_BOLD {
                    268
                } else if dim_bold == Flags::BOLD {
                    bright_named(index)
                } else if dim_bold == Flags::DIM {
                    dim_named(index)
                } else {
                    index
                };
                if index as usize >= TERMINAL_COLOR_COUNT {
                    self.color(256)
                } else {
                    self.color(index as usize)
                }
            }
        }
    }

    fn background(&self, color: &TerminalColor) -> u32 {
        match *color {
            TerminalColor::Rgb(color) => color,
            TerminalColor::Indexed(index) => self.color(index as usize),
            TerminalColor::Named(index) if index as usize >= TERMINAL_COLOR_COUNT => {
                self.color(257)
            }
            TerminalColor::Named(index) => self.color(index as usize),
        }
    }

    fn resolve_cell(&self, cell: &TerminalCell, use_cursor_text: bool) -> ResolvedCell {
        let flags = Flags::from_bits_retain(cell.flags);
        let mut foreground = self.foreground(&cell.foreground, flags);
        let mut background = self.background(&cell.background);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }
        let explicit_underline = cell
            .underline_color
            .as_ref()
            .map(|color| self.foreground(color, flags));
        if use_cursor_text && !flags.contains(Flags::HIDDEN) {
            foreground = with_alpha(self.appearance.cursor_text);
        }
        ResolvedCell {
            foreground,
            background,
            underline: explicit_underline.unwrap_or(foreground),
            flags,
        }
    }
}

pub(crate) fn terminal_cell_metrics(
    window: &mut Window,
    adjustments: ResolvedMetricAdjustments,
) -> (Pixels, Pixels) {
    let text_style = window.text_style();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let (cell_width, line_height) = crate::metal_terminal::font_metrics(
        text_style.font_family.as_ref(),
        f32::from(font_size),
        window.scale_factor(),
    );
    // `adjust-cell-width` / `adjust-cell-height` scale the font-derived cell box (Ghostty
    // semantics). Clamp to ≥1px so a hostile percent never collapses the grid.
    let cell_width = ResolvedMetricAdjustments::adjust(adjustments.cell_width, cell_width, 1.);
    let line_height = ResolvedMetricAdjustments::adjust(adjustments.cell_height, line_height, 1.);
    (px(cell_width), px(line_height))
}

pub(crate) fn terminal_background(snapshot: &TerminalSnapshot, theme: &TerminalTheme) -> u32 {
    TerminalPalette::new(snapshot, theme).color(257)
}

struct TerminalTextInputHandler {
    app: WeakEntity<EggieApp>,
    session_id: uuid::Uuid,
    cursor_bounds: Bounds<Pixels>,
    cell_width: Pixels,
    element_bounds: Bounds<Pixels>,
}

impl TerminalTextInputHandler {
    fn ime_state(&self, cx: &App) -> Option<TerminalImeState> {
        self.app
            .read_with(cx, |app, _| {
                app.terminal_ime_state(self.session_id).cloned()
            })
            .ok()
            .flatten()
    }
}

impl InputHandler for TerminalTextInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let range = self
            .ime_state(cx)
            .map(|state| state.selected_range)
            .unwrap_or(0..0);
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        let state = self.ime_state(cx)?;
        (!state.text.is_empty()).then(|| 0..state.text.encode_utf16().count())
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.app
            .update(cx, |app, cx| {
                app.commit_terminal_text(self.session_id, text, cx)
            })
            .ok();
        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.app
            .update(cx, |app, cx| {
                app.set_terminal_marked_text(self.session_id, new_text, new_selected_range, cx)
            })
            .ok();
        window.invalidate_character_coordinates();
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.app
            .update(cx, |app, cx| {
                app.clear_terminal_marked_text(self.session_id, cx)
            })
            .ok();
        window.invalidate_character_coordinates();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let columns = self
            .ime_state(cx)
            .map(|state| display_width_for_utf16_prefix(&state.text, range_utf16.start))
            .unwrap_or(0);
        let mut bounds = self.cursor_bounds;
        bounds.origin.x += self.cell_width * columns as f32;
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn element_bounds(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Bounds<Pixels>> {
        Some(self.element_bounds)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}

fn display_width_for_utf16_prefix(text: &str, utf16_offset: usize) -> usize {
    let mut utf16_length = 0;
    let mut byte_end = 0;
    for (byte_index, character) in text.char_indices() {
        let next_utf16_length = utf16_length + character.len_utf16();
        if next_utf16_length > utf16_offset {
            break;
        }
        utf16_length = next_utf16_length;
        byte_end = byte_index + character.len_utf8();
    }
    UnicodeWidthStr::width(&text[..byte_end])
}

impl IntoElement for MetalTerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MetalTerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Arc<PreparedMetalTerminal>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = f32::from(text_style.font_size.to_pixels(window.rem_size()));
        let (cell_width, line_height) = terminal_cell_metrics(window, self.adjustments);
        let scale_factor = window.scale_factor();
        let started = Instant::now();
        let family = Arc::from(text_style.font_family.as_ref());
        let key = PreparationKey {
            revision: self.snapshot.revision,
            viewport_width_bits: f32::from(bounds.size.width).to_bits(),
            viewport_height_bits: f32::from(bounds.size.height).to_bits(),
            cell_width_bits: f32::from(cell_width).to_bits(),
            line_height_bits: f32::from(line_height).to_bits(),
            scale_factor_bits: scale_factor.to_bits(),
            family: Arc::clone(&family),
            font_size_bits: font_size.to_bits(),
            theme: self.theme as *const TerminalTheme as usize,
            accent: self.accent,
            minimum_contrast_bits: self.minimum_contrast.to_bits(),
            selection: self.selection,
            ime: self.ime.clone(),
            search: self.search.clone(),
            url_hover: self.url_hover.clone(),
            cursor_visible: self.cursor_visible,
            adjustments: self.adjustments,
            font_families: self.font_families.clone(),
            font_features: Arc::clone(&self.font_features),
            ligatures: self.ligatures,
            shaping_break_cursor: self.shaping_break_cursor,
            font_variations: Arc::clone(&self.font_variations),
            font_thicken: self.font_thicken,
            font_codepoint_map: Arc::clone(&self.font_codepoint_map),
        };
        let cached = self
            .renderer
            .preparation_cache
            .lock()
            .entries
            .get(&self.snapshot.session_id)
            .filter(|cached| cached.key == key)
            .map(|cached| cached.terminal.clone());
        let cache_hit = cached.is_some();
        let prepared = if let Some(cached) = cached {
            cached
        } else {
            let image_cache = self.renderer.image_cache.lock();
            let prepared = Arc::new(prepare_terminal(
                &self.snapshot,
                self.theme,
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                f32::from(cell_width),
                f32::from(line_height),
                scale_factor,
                Arc::clone(&family),
                font_size,
                self.accent,
                self.minimum_contrast,
                self.selection,
                self.ime.as_ref(),
                self.search.as_ref(),
                self.url_hover.as_ref(),
                self.cursor_visible,
                self.adjustments,
                self.font_families.clone(),
                Arc::clone(&self.font_features),
                self.ligatures,
                self.shaping_break_cursor,
                Arc::clone(&self.font_variations),
                self.font_thicken,
                Arc::clone(&self.font_codepoint_map),
                &image_cache,
            ));
            drop(image_cache);
            let mut cache = self.renderer.preparation_cache.lock();
            if cache.entries.len() >= 64
                && !cache.entries.contains_key(&self.snapshot.session_id)
                && let Some(oldest) = cache.entries.keys().next().copied()
            {
                cache.entries.remove(&oldest);
                cache.last_ready.remove(&oldest);
            }
            cache.entries.insert(
                self.snapshot.session_id,
                CachedPreparation {
                    key,
                    terminal: prepared.clone(),
                },
            );
            prepared
        };

        let physical_origin = [
            (f32::from(bounds.origin.x) * scale_factor).round(),
            (f32::from(bounds.origin.y) * scale_factor).round(),
        ];
        let glyphs = self.renderer.glyph_raster_cache.prepare_terminal(
            &prepared,
            scale_factor,
            physical_origin,
        );
        let layout = RasterLayoutKey {
            viewport_origin_x_bits: physical_origin[0].to_bits(),
            viewport_origin_y_bits: physical_origin[1].to_bits(),
            viewport_width_bits: f32::from(bounds.size.width).to_bits(),
            viewport_height_bits: f32::from(bounds.size.height).to_bits(),
            cell_width_bits: f32::from(cell_width).to_bits(),
            line_height_bits: f32::from(line_height).to_bits(),
            scale_factor_bits: scale_factor.to_bits(),
            family,
            font_size_bits: font_size.to_bits(),
        };
        let selected = {
            let mut cache = self.renderer.preparation_cache.lock();
            if glyphs.ready {
                cache.last_ready.insert(
                    self.snapshot.session_id,
                    ReadyPreparation {
                        layout,
                        terminal: prepared.clone(),
                    },
                );
                prepared
            } else {
                cache
                    .last_ready
                    .get(&self.snapshot.session_id)
                    .filter(|ready| ready.layout == layout)
                    .map(|ready| ready.terminal.clone())
                    .unwrap_or(prepared)
            }
        };
        if !glyphs.ready {
            // Worker completion is intentionally not coupled to App/Window lifetimes. A bounded
            // animation-frame retry keeps the last coherent terminal visible and notices newly
            // ready raster results without posting cross-thread GPUI callbacks.
            window.request_animation_frame();
        }
        self.input_latency
            .record_prepare(started.elapsed(), cache_hit);
        selected
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepared: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        if let Some(input) = &self.input {
            let (cell_width, line_height) = terminal_cell_metrics(window, self.adjustments);
            let cursor_bounds = Bounds::new(
                point(
                    bounds.origin.x + cell_width * self.snapshot.cursor_column as f32,
                    bounds.origin.y + line_height * self.snapshot.cursor_line as f32,
                ),
                size(cell_width, line_height),
            );
            window.handle_input(
                &input.focus,
                TerminalTextInputHandler {
                    app: input.app.clone(),
                    session_id: self.snapshot.session_id,
                    cursor_bounds,
                    cell_width,
                    element_bounds: bounds,
                },
                _cx,
            );
        }
        window.paint_metal(
            bounds,
            Arc::new(TerminalMetalPrimitive {
                state: self.renderer.state.clone(),
                terminal: prepared.clone(),
                glyph_raster_cache: self.renderer.glyph_raster_cache.clone(),
                scale_factor: window.scale_factor(),
                input_latency: self.input_latency.clone(),
                session_id: self.snapshot.session_id,
                last_input_sequence: prepared.last_input_sequence,
            }),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_terminal(
    snapshot: &TerminalSnapshot,
    theme: &TerminalTheme,
    viewport_width: f32,
    viewport_height: f32,
    cell_width: f32,
    line_height: f32,
    scale_factor: f32,
    family: Arc<str>,
    font_size: f32,
    accent: u32,
    minimum_contrast: f32,
    selection: Option<TerminalSelection>,
    ime: Option<&TerminalImeState>,
    search: Option<&TerminalSearchHighlights>,
    url_hover: Option<&TerminalLinkRange>,
    cursor_visible: bool,
    adjustments: ResolvedMetricAdjustments,
    font_families: Option<crate::settings::ResolvedFontFamilies>,
    font_features: Arc<[crate::settings::FontFeature]>,
    ligatures: bool,
    shaping_break_cursor: bool,
    font_variations: Arc<[crate::settings::FontVariation]>,
    font_thicken: Option<u8>,
    font_codepoint_map: Arc<[crate::settings::CodepointMapEntry]>,
    image_cache: &FxHashMap<TerminalTextureKey, Arc<TerminalImageData>>,
) -> PreparedMetalTerminal {
    let palette = TerminalPalette::new(snapshot, theme);
    let grid = terminal_grid(snapshot);
    let default_background = palette.color(257);
    let default_background_command = MetalCommand::Rect {
        rect: [0., 0., viewport_width, viewport_height],
        color: default_background,
    };
    let mut backgrounds = Vec::new();
    prepare_backgrounds(
        &grid,
        &palette,
        default_background,
        cell_width,
        line_height,
        &mut backgrounds,
    );
    let mut selection_commands = prepare_selection(
        snapshot,
        selection,
        cell_width,
        line_height,
        (accent << 8) | 0x55,
    );
    // Search highlights sit in the same overlay layer as the selection. Non-active matches use a
    // muted yellow; the active match uses a stronger orange so it stands out.
    let mut search_commands = prepare_search(search, snapshot, cell_width, line_height);
    let (mut cursor_background, mut cursor_foreground) =
        if !cursor_visible || ime.is_some_and(|ime| !ime.text.is_empty()) {
            (Vec::new(), Vec::new())
        } else {
            prepare_cursor(snapshot, &palette, cell_width, line_height, adjustments)
        };
    let mut glyphs = prepare_glyphs(
        &grid,
        snapshot,
        &palette,
        cell_width,
        line_height,
        minimum_contrast,
        &ShapingContext {
            family: &family,
            font_families: font_families.as_ref(),
            font_size,
            features: &font_features,
            shaping_break_cursor,
            ligatures_enabled: ligatures,
        },
    );
    let mut decorations = prepare_decorations(
        &grid,
        &palette,
        cell_width,
        line_height,
        minimum_contrast,
        adjustments,
    );
    // Hovered auto-detected URL: underline it in the accent color so it reads as clickable.
    let mut url_hover_commands =
        prepare_url_hover(url_hover, snapshot, cell_width, line_height, (accent << 8) | 0xff);
    let (mut images_below_background, mut images_below_text, mut images_above_text) =
        prepare_images(snapshot, cell_width, line_height, scale_factor, image_cache);
    let mut commands = Vec::with_capacity(
        1 + images_below_background.len()
            + backgrounds.len()
            + images_below_text.len()
            + selection_commands.len()
            + search_commands.len()
            + cursor_background.len()
            + glyphs.len()
            + decorations.len()
            + url_hover_commands.len()
            + cursor_foreground.len()
            + images_above_text.len(),
    );
    commands.push(default_background_command);
    commands.append(&mut images_below_background);
    commands.append(&mut backgrounds);
    commands.append(&mut images_below_text);
    commands.append(&mut selection_commands);
    commands.append(&mut search_commands);
    commands.append(&mut cursor_background);
    commands.append(&mut glyphs);
    commands.append(&mut decorations);
    commands.append(&mut url_hover_commands);
    commands.append(&mut cursor_foreground);
    commands.append(&mut images_above_text);
    if let Some(ime) = ime.filter(|ime| !ime.text.is_empty()) {
        prepare_ime(
            snapshot,
            ime,
            &palette,
            cell_width,
            line_height,
            minimum_contrast,
            &mut commands,
        );
    }
    PreparedMetalTerminal {
        commands,
        family,
        font_size,
        cell_width,
        last_input_sequence: snapshot.last_input_sequence,
        baseline_adjustment: adjustments.font_baseline,
        box_thickness_adjustment: adjustments.box_thickness,
        icon_height_adjustment: adjustments.icon_height,
        font_families,
        font_features,
        font_variations,
        font_thicken,
        font_codepoint_map,
    }
}

fn prepare_images(
    snapshot: &TerminalSnapshot,
    cell_width: f32,
    line_height: f32,
    scale_factor: f32,
    image_cache: &FxHashMap<TerminalTextureKey, Arc<TerminalImageData>>,
) -> (Vec<MetalCommand>, Vec<MetalCommand>, Vec<MetalCommand>) {
    const BELOW_BACKGROUND_LIMIT: i32 = i32::MIN / 2;
    let mut below_background = Vec::new();
    let mut below_text = Vec::new();
    let mut above_text = Vec::new();
    let pixel_scale = scale_factor.max(1.);
    // The terminal core already emits Kitty's (z-index, internal image id, internal ref id)
    // order. Client-provided image and placement IDs are not ordering keys.
    for placement in &snapshot.image_placements {
        let key = TerminalTextureKey {
            session_id: snapshot.session_id,
            image: placement.image,
        };
        let Some(image) = image_cache.get(&key) else {
            continue;
        };
        if image.width == 0 || image.height == 0 {
            continue;
        }
        let x_offset = placement.x_offset as f32 / pixel_scale;
        let y_offset = placement.y_offset as f32 / pixel_scale;
        // The terminal core computes this rectangle in device pixels using Kitty's placement
        // rules. It is also the clipped destination for Unicode-placeholder fragments, so
        // rebuilding it from c/r here would incorrectly stretch letterboxed edge fragments back
        // to full cells. Convert it to GPUI logical pixels without changing its geometry.
        let width = placement.destination_width.max(1) as f32 / pixel_scale;
        let height = placement.destination_height.max(1) as f32 / pixel_scale;
        let command = MetalCommand::Image {
            image: Arc::clone(image),
            rect: [
                placement.column as f32 * cell_width + x_offset,
                placement.line as f32 * line_height + y_offset,
                width,
                height,
            ],
            source: [
                placement.source_x as f32 / image.width as f32,
                placement.source_y as f32 / image.height as f32,
                placement.source_width as f32 / image.width as f32,
                placement.source_height as f32 / image.height as f32,
            ],
        };
        if placement.z < BELOW_BACKGROUND_LIMIT {
            below_background.push(command);
        } else if placement.z < 0 {
            below_text.push(command);
        } else {
            above_text.push(command);
        }
    }
    (below_background, below_text, above_text)
}

struct TerminalGrid<'a> {
    cells: Vec<Option<&'a TerminalCell>>,
    columns: usize,
    rows: usize,
}

impl<'a> TerminalGrid<'a> {
    fn row(&self, line: usize) -> &[Option<&'a TerminalCell>] {
        let start = line * self.columns;
        &self.cells[start..start + self.columns]
    }
}

fn terminal_grid(snapshot: &TerminalSnapshot) -> TerminalGrid<'_> {
    let columns = snapshot.size.columns as usize;
    let rows = snapshot.size.rows as usize;
    let mut grid = TerminalGrid {
        cells: vec![None; columns.saturating_mul(rows)],
        columns,
        rows,
    };
    for cell in &snapshot.cells {
        if let Some(index) = (cell.line as usize)
            .checked_mul(columns)
            .and_then(|start| start.checked_add(cell.column as usize))
            && let Some(slot) = grid.cells.get_mut(index)
        {
            *slot = Some(cell);
        }
    }
    grid
}

fn prepare_selection(
    snapshot: &TerminalSnapshot,
    selection: Option<TerminalSelection>,
    cell_width: f32,
    line_height: f32,
    color: u32,
) -> Vec<MetalCommand> {
    let Some(selection) = selection else {
        return Vec::new();
    };
    let rows = snapshot.size.rows;
    let columns = snapshot.size.columns;
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let (mut start, mut end) = selection.ordered();
    start.line = start.line.min(rows - 1);
    end.line = end.line.min(rows - 1);
    start.column = start.column.min(columns - 1);
    end.column = end.column.min(columns - 1);
    let mut commands = Vec::with_capacity((end.line - start.line + 1) as usize);
    for line in start.line..=end.line {
        let first_column = if line == start.line { start.column } else { 0 };
        let last_column = if line == end.line {
            end.column
        } else {
            columns - 1
        };
        push_rect(
            &mut commands,
            first_column as f32 * cell_width,
            line as f32 * line_height,
            (last_column - first_column + 1) as f32 * cell_width,
            line_height,
            color,
        );
    }
    commands
}

/// Non-active search matches: muted yellow. RGBA packed as `(rgb << 8) | alpha`.
const SEARCH_MATCH_COLOR: u32 = (0xFFD54F << 8) | 0x66;
/// The active search match: stronger orange so the current hit stands out.
const SEARCH_ACTIVE_COLOR: u32 = (0xFF9800 << 8) | 0xBB;

/// Build overlay rectangles for search matches. Non-active matches are drawn first (muted), then
/// the active match on top (stronger), so an overlapping active highlight always wins visually.
fn prepare_search(
    search: Option<&TerminalSearchHighlights>,
    snapshot: &TerminalSnapshot,
    cell_width: f32,
    line_height: f32,
) -> Vec<MetalCommand> {
    let Some(search) = search else {
        return Vec::new();
    };
    let rows = snapshot.size.rows;
    let columns = snapshot.size.columns;
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let mut commands = Vec::with_capacity(search.matches.len() + 1);
    for search_match in &search.matches {
        // Skip the match that is also the active one; it is drawn separately on top.
        if search.active == Some(*search_match) {
            continue;
        }
        push_search_match(
            &mut commands,
            *search_match,
            rows,
            columns,
            cell_width,
            line_height,
            SEARCH_MATCH_COLOR,
        );
    }
    if let Some(active) = search.active {
        push_search_match(
            &mut commands,
            active,
            rows,
            columns,
            cell_width,
            line_height,
            SEARCH_ACTIVE_COLOR,
        );
    }
    commands
}

/// Push the overlay rectangles for a single (possibly multi-line) match, clamped to the grid.
fn push_search_match(
    commands: &mut Vec<MetalCommand>,
    search_match: TerminalSearchMatch,
    rows: u16,
    columns: u16,
    cell_width: f32,
    line_height: f32,
    color: u32,
) {
    let mut start = search_match.start;
    let mut end = search_match.end;
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    start.line = start.line.min(rows - 1);
    end.line = end.line.min(rows - 1);
    start.column = start.column.min(columns - 1);
    end.column = end.column.min(columns - 1);
    for line in start.line..=end.line {
        let first_column = if line == start.line { start.column } else { 0 };
        let last_column = if line == end.line { end.column } else { columns - 1 };
        // Guard against first > last (e.g. a multi-row match whose endpoints clamped onto the same
        // row) so the u16 width computation never underflows.
        let (lo, hi) = (first_column.min(last_column), first_column.max(last_column));
        push_rect(
            commands,
            lo as f32 * cell_width,
            line as f32 * line_height,
            (hi - lo + 1) as f32 * cell_width,
            line_height,
            color,
        );
    }
}

/// Underline a hovered auto-detected URL. `detected_links` are already emitted one range per row, so
/// this draws a single thin rect along the bottom of the hovered range's cells.
fn prepare_url_hover(
    url_hover: Option<&TerminalLinkRange>,
    snapshot: &TerminalSnapshot,
    cell_width: f32,
    line_height: f32,
    color: u32,
) -> Vec<MetalCommand> {
    let Some(link) = url_hover else {
        return Vec::new();
    };
    let rows = snapshot.size.rows;
    let columns = snapshot.size.columns;
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let line = link.start.line.min(rows - 1);
    let first_column = link.start.column.min(columns - 1);
    let last_column = link.end.column.min(columns - 1);
    let (lo, hi) = (first_column.min(last_column), first_column.max(last_column));
    // A 1px underline sitting just above the cell's bottom edge.
    let thickness = (line_height * 0.08).clamp(1., 2.);
    let mut commands = Vec::with_capacity(1);
    push_rect(
        &mut commands,
        lo as f32 * cell_width,
        (line as f32 + 1.) * line_height - thickness,
        (hi - lo + 1) as f32 * cell_width,
        thickness,
        color,
    );
    commands
}

#[allow(clippy::too_many_arguments)]
fn prepare_ime(
    snapshot: &TerminalSnapshot,
    ime: &TerminalImeState,
    palette: &TerminalPalette,
    cell_width: f32,
    line_height: f32,
    minimum_contrast: f32,
    commands: &mut Vec<MetalCommand>,
) {
    if snapshot.cursor_line >= snapshot.size.rows || snapshot.cursor_column >= snapshot.size.columns
    {
        return;
    }
    let columns = UnicodeWidthStr::width(ime.text.as_str()).max(1);
    let x = snapshot.cursor_column as f32 * cell_width;
    let y = snapshot.cursor_line as f32 * line_height;
    let width = columns as f32 * cell_width;
    let background = palette.color(257);
    let foreground = palette.color(256);
    push_rect(commands, x, y, width, line_height, background);
    commands.push(MetalCommand::Glyph {
        rect: [x, y, width, line_height],
        text: Arc::from(ime.text.as_str()),
        color: foreground,
        bold: false,
        italic: false,
        sprite: false,
        background,
        minimum_contrast,
        minimum_contrast_disabled: false,
        glyph_id: None,
    });
    push_rect(
        commands,
        x,
        y + line_height - 1.,
        width,
        1.,
        minimum_contrast_rgba(foreground, background, minimum_contrast),
    );
}

fn prepare_backgrounds(
    grid: &TerminalGrid<'_>,
    palette: &TerminalPalette,
    default_background: u32,
    cell_width: f32,
    line_height: f32,
    commands: &mut Vec<MetalCommand>,
) {
    for row_index in 0..grid.rows {
        let row = grid.row(row_index);
        let mut start = None;
        let mut color = default_background;
        for column in 0..=row.len() {
            let next_color = row
                .get(column)
                .and_then(Option::as_ref)
                .map(|cell| {
                    let flags = Flags::from_bits_retain(cell.flags);
                    if cell.background == TerminalColor::Named(257)
                        && !flags.contains(Flags::INVERSE)
                    {
                        default_background
                    } else {
                        palette.resolve_cell(cell, false).background
                    }
                })
                .filter(|next| *next != default_background);
            if let Some(start_column) = start
                && next_color != Some(color)
            {
                push_rect(
                    commands,
                    start_column as f32 * cell_width,
                    row_index as f32 * line_height,
                    (column - start_column) as f32 * cell_width,
                    line_height,
                    color,
                );
                start = None;
            }
            if start.is_none()
                && let Some(next_color) = next_color
            {
                start = Some(column);
                color = next_color;
            }
        }
    }
}

/// Inputs the glyph-preparation pass needs to shape ligature runs. Borrowed for the duration of one
/// `prepare_terminal` call.
struct ShapingContext<'a> {
    family: &'a Arc<str>,
    font_families: Option<&'a crate::settings::ResolvedFontFamilies>,
    font_size: f32,
    features: &'a [crate::settings::FontFeature],
    shaping_break_cursor: bool,
    /// Whether run shaping should run at all. False takes the cheap per-grapheme path for every
    /// cell (ligatures disabled), avoiding wasted CoreText shaping.
    ligatures_enabled: bool,
}

impl ShapingContext<'_> {
    /// The family used for a `(bold, italic)` style — the per-style resolved family when set, else
    /// the single base family.
    fn family_for(&self, bold: bool, italic: bool) -> &str {
        match self.font_families {
            Some(families) => families.family(bold, italic).as_ref(),
            None => self.family.as_ref(),
        }
    }
}

/// Maximum number of cells a single shaping run may cover. Bounds per-frame shaping cost for
/// pathologically long same-style runs. Each cell still renders at cell width (no wide tiles), so
/// this only splits shaping context — set high enough that a real code line's ligatures never
/// straddle the boundary.
const MAX_SHAPING_RUN_CELLS: usize = 256;

#[allow(clippy::too_many_arguments)]
fn prepare_glyphs(
    grid: &TerminalGrid<'_>,
    snapshot: &TerminalSnapshot,
    palette: &TerminalPalette,
    cell_width: f32,
    line_height: f32,
    minimum_contrast: f32,
    shaping: &ShapingContext<'_>,
) -> Vec<MetalCommand> {
    let mut commands = Vec::new();
    let cursor_is_block = snapshot.cursor_shape == TerminalCursorShape::Block;
    let ligatures_possible = shaping.ligatures_enabled;
    for row_index in 0..grid.rows {
        let row = grid.row(row_index);
        let cursor_column = (snapshot.cursor_line as usize == row_index)
            .then_some(snapshot.cursor_column as usize);
        let mut column = 0;
        while column < row.len() {
            let Some(cell) = row[column].as_ref() else {
                column += 1;
                continue;
            };
            let flags = Flags::from_bits_retain(cell.flags);
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                column += 1;
                continue;
            }
            let span = if flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let hidden = flags.contains(Flags::HIDDEN);
            let visible = !hidden
                && ((cell.character != ' ' && cell.character != '\t')
                    || !cell.zerowidth.is_empty());
            if !visible {
                column += span;
                continue;
            }

            // Try to assemble and shape a ligature run starting here. On success it emits the run's
            // commands and returns how many cells it consumed; otherwise fall through to per-cell.
            if ligatures_possible
                && let Some(consumed) = try_emit_shaped_run(
                    &mut commands,
                    row,
                    row_index,
                    column,
                    cursor_column,
                    cursor_is_block,
                    palette,
                    cell_width,
                    line_height,
                    minimum_contrast,
                    shaping,
                )
            {
                column += consumed;
                continue;
            }

            let use_cursor_text = cursor_is_block && cursor_column == Some(column);
            let (text, source_span) = collect_grapheme(row, column);
            emit_glyph_command(
                &mut commands,
                row_index,
                column,
                &text,
                source_span,
                cell,
                palette.resolve_cell(cell, use_cursor_text),
                use_cursor_text,
                cell_width,
                line_height,
                minimum_contrast,
                None,
            );
            column += source_span;
        }
    }
    commands
}

/// Emit a single glyph command for a grapheme cluster at `column`. Shared by the per-cell path and
/// the ligature run so their geometry/attributes stay identical. `glyph_id` carries a pre-shaped
/// glyph (from a ligature run) to draw instead of re-shaping `text`; `None` for normal cells.
#[allow(clippy::too_many_arguments)]
fn emit_glyph_command(
    commands: &mut Vec<MetalCommand>,
    row_index: usize,
    column: usize,
    text: &str,
    source_span: usize,
    cell: &TerminalCell,
    resolved: ResolvedCell,
    use_cursor_text: bool,
    cell_width: f32,
    line_height: f32,
    minimum_contrast: f32,
    glyph_id: Option<u16>,
) {
    let flags = Flags::from_bits_retain(cell.flags);
    let span = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
    let render_span = UnicodeWidthStr::width(text)
        .clamp(1, 2)
        .min(source_span);
    let sprite = source_span == span
        && cell.zerowidth.is_empty()
        && SpriteKind::for_char(cell.character).is_some();
    commands.push(MetalCommand::Glyph {
        rect: [
            column as f32 * cell_width,
            row_index as f32 * line_height,
            render_span as f32 * cell_width,
            line_height,
        ],
        text: Arc::from(text),
        color: resolved.foreground,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        sprite,
        background: resolved.background,
        minimum_contrast,
        minimum_contrast_disabled: terminal_sprites::skips_minimum_contrast(cell.character)
            || use_cursor_text,
        // A shaped glyph never applies to a sprite (those bypass the font); guard so a stray gid
        // can't hijack a box-drawing cell.
        glyph_id: if sprite { None } else { glyph_id },
    });
}

/// Whether a cell may participate in a shaping run: a visible, single-cell, non-sprite, non-wide
/// character with no combining marks. CJK / emoji / box-drawing / combining sequences never ligate
/// in a terminal, so they always break the run (and use the per-grapheme path).
fn shapeable_cell(cell: &TerminalCell) -> bool {
    let flags = Flags::from_bits_retain(cell.flags);
    if flags.intersects(
        Flags::WIDE_CHAR
            | Flags::WIDE_CHAR_SPACER
            | Flags::LEADING_WIDE_CHAR_SPACER
            | Flags::HIDDEN,
    ) {
        return false;
    }
    if !cell.zerowidth.is_empty() {
        return false;
    }
    if cell.character == ' ' || cell.character == '\t' {
        return false;
    }
    if SpriteKind::for_char(cell.character).is_some() {
        return false;
    }
    // A char that isn't a single UTF-16 unit (astral plane) can't map cleanly to one cell index.
    cell.character.len_utf16() == 1
}

/// Attempt to shape a ligature run starting at `start`. Assembles a maximal contiguous same-style
/// run of shapeable cells (bounded by [`MAX_SHAPING_RUN_CELLS`] and split at the cursor when
/// requested), shapes it, and emits one command per resulting glyph cluster. Returns the number of
/// cells consumed, or `None` if no ligature merged (so the caller uses the cheaper per-cell path).
#[allow(clippy::too_many_arguments)]
fn try_emit_shaped_run(
    commands: &mut Vec<MetalCommand>,
    row: &[Option<&TerminalCell>],
    row_index: usize,
    start: usize,
    cursor_column: Option<usize>,
    cursor_is_block: bool,
    palette: &TerminalPalette,
    cell_width: f32,
    line_height: f32,
    minimum_contrast: f32,
    shaping: &ShapingContext<'_>,
) -> Option<usize> {
    let first = row.get(start).and_then(Option::as_ref).copied()?;
    if !shapeable_cell(first) {
        return None;
    }
    // Grow a run of contiguous cells that share the first cell's style and are all shapeable. Stop
    // before the cursor cell when breaking there so it renders un-ligated (Ghostty semantics).
    let mut end = start;
    while end < row.len() && end - start < MAX_SHAPING_RUN_CELLS {
        let Some(cell) = row.get(end).and_then(Option::as_ref).copied() else {
            break;
        };
        if !shapeable_cell(cell) || !same_grapheme_style(first, cell) {
            break;
        }
        if shaping.shaping_break_cursor
            && cursor_is_block
            && cursor_column == Some(end)
            && end != start
        {
            break;
        }
        end += 1;
    }
    let cell_count = end - start;
    if cell_count < 2 {
        return None;
    }
    let bold = Flags::from_bits_retain(first.flags).contains(Flags::BOLD);
    let italic = Flags::from_bits_retain(first.flags).contains(Flags::ITALIC);
    let run_text: String = (start..end)
        .filter_map(|column| row.get(column).and_then(Option::as_ref))
        .map(|cell| cell.character)
        .collect();
    let glyph_ids = crate::metal_terminal::shape_terminal_run(
        &run_text,
        shaping.family_for(bold, italic),
        shaping.font_size,
        bold,
        italic,
        shaping.features,
        cell_count,
    )?;
    // Per-cell contextual substitution: each cell keeps its own slot/advance, but renders the
    // shaped glyph id (a variant that joins its neighbors) instead of re-shaping the character.
    for (offset, glyph_id) in glyph_ids.into_iter().enumerate() {
        let column = start + offset;
        let cell = row.get(column).and_then(Option::as_ref).copied()?;
        let use_cursor_text = cursor_is_block && cursor_column == Some(column);
        let resolved = palette.resolve_cell(cell, use_cursor_text);
        let (text, source_span) = collect_grapheme(row, column);
        emit_glyph_command(
            commands,
            row_index,
            column,
            &text,
            source_span,
            cell,
            resolved,
            use_cursor_text,
            cell_width,
            line_height,
            minimum_contrast,
            Some(glyph_id),
        );
    }
    Some(cell_count)
}

fn collect_grapheme(row: &[Option<&TerminalCell>], start: usize) -> (String, usize) {
    let Some(first) = row.get(start).and_then(Option::as_ref) else {
        return (String::new(), 1);
    };
    let first_flags = Flags::from_bits_retain(first.flags);
    let first_span = cell_span(first_flags);
    let mut text = cell_text(first);
    let mut source_span = first_span;

    loop {
        let next_column = start + source_span;
        let Some(next) = row.get(next_column).and_then(Option::as_ref) else {
            break;
        };
        let next_flags = Flags::from_bits_retain(next.flags);
        if next_flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            || !same_grapheme_style(first, next)
            || !could_extend_grapheme(&text, next.character)
        {
            break;
        }

        let next_text = cell_text(next);
        let previous_len = text.len();
        text.push_str(&next_text);
        if text
            .graphemes(true)
            .next()
            .is_none_or(|cluster| cluster.len() != text.len())
        {
            text.truncate(previous_len);
            break;
        }
        source_span += cell_span(next_flags);
    }

    (text, source_span)
}

fn cell_text(cell: &TerminalCell) -> String {
    let mut text = String::with_capacity((1 + cell.zerowidth.len()) * 4);
    text.push(if cell.character == '\t' {
        ' '
    } else {
        cell.character
    });
    text.extend(cell.zerowidth.iter().copied());
    text
}

fn could_extend_grapheme(current: &str, next: char) -> bool {
    current.ends_with('\u{200d}')
        || ('\u{1f3fb}'..='\u{1f3ff}').contains(&next)
        || (current
            .chars()
            .next()
            .is_some_and(|character| ('\u{1f1e6}'..='\u{1f1ff}').contains(&character))
            && ('\u{1f1e6}'..='\u{1f1ff}').contains(&next))
}

fn cell_span(flags: Flags) -> usize {
    if flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

fn same_grapheme_style(first: &TerminalCell, next: &TerminalCell) -> bool {
    const LAYOUT_FLAGS: Flags = Flags::WIDE_CHAR
        .union(Flags::WIDE_CHAR_SPACER)
        .union(Flags::LEADING_WIDE_CHAR_SPACER);
    first.foreground == next.foreground
        && first.background == next.background
        && first.underline_color == next.underline_color
        && (Flags::from_bits_retain(first.flags) - LAYOUT_FLAGS)
            == (Flags::from_bits_retain(next.flags) - LAYOUT_FLAGS)
}

fn prepare_decorations(
    grid: &TerminalGrid<'_>,
    palette: &TerminalPalette,
    cell_width: f32,
    line_height: f32,
    minimum_contrast: f32,
    adjustments: ResolvedMetricAdjustments,
) -> Vec<MetalCommand> {
    // Base decoration metrics (device-independent px). These mirror the previous hardcoded values;
    // `adjust-underline-*` / `adjust-strikethrough-*` shift/scale them (Ghostty semantics).
    // `*_position` is the distance from the cell top (larger = lower); thickness clamps to ≥1px.
    let underline_thickness =
        ResolvedMetricAdjustments::adjust(adjustments.underline_thickness, 1., 1.);
    let underline_position = ResolvedMetricAdjustments::adjust(
        adjustments.underline_position,
        line_height - 2.,
        0.,
    );
    let strikethrough_thickness =
        ResolvedMetricAdjustments::adjust(adjustments.strikethrough_thickness, 1., 1.);
    let strikethrough_position = ResolvedMetricAdjustments::adjust(
        adjustments.strikethrough_position,
        line_height * 0.55,
        0.,
    );
    let mut commands = Vec::new();
    for row_index in 0..grid.rows {
        let row = grid.row(row_index);
        for (column, cell) in row.iter().enumerate() {
            let Some(cell) = cell else {
                continue;
            };
            let flags = Flags::from_bits_retain(cell.flags);
            if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            if !flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT) {
                continue;
            }
            let resolved = palette.resolve_cell(cell, false);
            let width = cell_width
                * if flags.contains(Flags::WIDE_CHAR) {
                    2.
                } else {
                    1.
                };
            let x = column as f32 * cell_width;
            let y = row_index as f32 * line_height;
            let underline =
                minimum_contrast_rgba(resolved.underline, resolved.background, minimum_contrast);
            let foreground =
                minimum_contrast_rgba(resolved.foreground, resolved.background, minimum_contrast);
            // Underline baseline (bottom of the drawn line). Kept within the cell.
            let underline_y = (y + underline_position).min(y + line_height - underline_thickness);
            match flags & Flags::ALL_UNDERLINES {
                Flags::UNDERLINE => push_rect(
                    &mut commands,
                    x,
                    underline_y,
                    width,
                    underline_thickness,
                    underline,
                ),
                Flags::DOUBLE_UNDERLINE => {
                    push_rect(
                        &mut commands,
                        x,
                        underline_y,
                        width,
                        underline_thickness,
                        underline,
                    );
                    push_rect(
                        &mut commands,
                        x,
                        underline_y - (underline_thickness + 2.),
                        width,
                        underline_thickness,
                        underline,
                    );
                }
                Flags::UNDERCURL => {
                    push_undercurl(&mut commands, x, underline_y - 2., width, underline)
                }
                Flags::DOTTED_UNDERLINE => push_patterned_line(
                    &mut commands,
                    x,
                    underline_y,
                    width,
                    underline_thickness,
                    2.,
                    underline,
                ),
                Flags::DASHED_UNDERLINE => push_patterned_line(
                    &mut commands,
                    x,
                    underline_y,
                    width,
                    4.,
                    2.,
                    underline,
                ),
                _ => {}
            }
            if flags.contains(Flags::STRIKEOUT) {
                push_rect(
                    &mut commands,
                    x,
                    y + strikethrough_position,
                    width,
                    strikethrough_thickness,
                    foreground,
                );
            }
        }
    }
    commands
}

fn prepare_cursor(
    snapshot: &TerminalSnapshot,
    palette: &TerminalPalette,
    cell_width: f32,
    line_height: f32,
    adjustments: ResolvedMetricAdjustments,
) -> (Vec<MetalCommand>, Vec<MetalCommand>) {
    if snapshot.cursor_shape == TerminalCursorShape::Hidden
        || snapshot.cursor_line >= snapshot.size.rows
        || snapshot.cursor_column >= snapshot.size.columns
    {
        return (Vec::new(), Vec::new());
    }
    let x = cell_width * snapshot.cursor_column as f32;
    let y = line_height * snapshot.cursor_line as f32;
    let width = cell_width * snapshot.cursor_width.clamp(1, 2) as f32;
    let color = palette.color(258);
    // `adjust-cursor-thickness` scales the stroke of the bar/underline/hollow cursors (Ghostty
    // applies it to the bar and outlined-rect cursors). Base is 2px for bar/underline, 1px for the
    // hollow outline; clamp to ≥1px.
    let bar_thickness = ResolvedMetricAdjustments::adjust(adjustments.cursor_thickness, 2., 1.);
    let hollow_thickness = ResolvedMetricAdjustments::adjust(adjustments.cursor_thickness, 1., 1.);
    let mut background = Vec::new();
    let mut foreground = Vec::new();
    match snapshot.cursor_shape {
        TerminalCursorShape::Block => push_rect(&mut background, x, y, width, line_height, color),
        TerminalCursorShape::Underline => push_rect(
            &mut foreground,
            x,
            y + line_height - bar_thickness,
            width,
            bar_thickness,
            color,
        ),
        TerminalCursorShape::Beam => {
            push_rect(&mut foreground, x, y, bar_thickness, line_height, color)
        }
        TerminalCursorShape::HollowBlock => {
            push_rect(&mut foreground, x, y, width, hollow_thickness, color);
            push_rect(
                &mut foreground,
                x,
                y + line_height - hollow_thickness,
                width,
                hollow_thickness,
                color,
            );
            push_rect(&mut foreground, x, y, hollow_thickness, line_height, color);
            push_rect(
                &mut foreground,
                x + width - hollow_thickness,
                y,
                hollow_thickness,
                line_height,
                color,
            );
        }
        TerminalCursorShape::Hidden => {}
    }
    (background, foreground)
}

fn push_rect(
    commands: &mut Vec<MetalCommand>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: u32,
) {
    if width > 0. && height > 0. {
        commands.push(MetalCommand::Rect {
            rect: [x, y, width, height],
            color,
        });
    }
}

fn push_patterned_line(
    commands: &mut Vec<MetalCommand>,
    x: f32,
    y: f32,
    width: f32,
    dash: f32,
    gap: f32,
    color: u32,
) {
    let mut offset = 0.;
    while offset < width {
        let segment = dash.min(width - offset);
        push_rect(commands, x + offset, y, segment, 1., color);
        offset += dash + gap;
    }
}

fn push_undercurl(commands: &mut Vec<MetalCommand>, x: f32, y: f32, width: f32, color: u32) {
    const WAVE: [f32; 4] = [1., 2., 1., 0.];
    for offset in 0..width.ceil() as usize {
        push_rect(
            commands,
            x + offset as f32,
            y + WAVE[offset % WAVE.len()],
            1.,
            1.,
            color,
        );
    }
}

fn bright_named(index: u16) -> u16 {
    match index {
        0..=7 => index + 8,
        256 => 267,
        259..=266 => index - 259,
        _ => index,
    }
}

fn dim_named(index: u16) -> u16 {
    match index {
        0..=7 => index + 259,
        8..=15 => index - 8,
        256 => 268,
        _ => index,
    }
}

fn minimum_contrast_rgba(foreground: u32, background: u32, minimum_contrast: f32) -> u32 {
    let alpha = foreground & 0xff;
    let foreground_rgb = foreground >> 8;
    let background_rgb = background >> 8;
    (minimum_contrast_rgb(foreground_rgb, background_rgb, minimum_contrast) << 8) | alpha
}

fn with_alpha(color: u32) -> u32 {
    (color << 8) | 0xff
}

fn dim_rgba(color: u32) -> u32 {
    let alpha = color & 0xff;
    let dim = |shift: u32| ((((color >> shift) & 0xff) as f32 * 0.66) as u32) & 0xff;
    (dim(24) << 24) | (dim(16) << 16) | (dim(8) << 8) | alpha
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{AppSettings, ThemeMode, theme_catalog};
    use eggie_protocol::{TerminalCellPosition, TerminalColorOverride, TerminalSize};
    use uuid::Uuid;

    fn menlo_family() -> Arc<str> {
        Arc::from("Menlo")
    }

    /// A shaping context with ligatures disabled — the glyph tests exercise the per-cell path and
    /// don't want CoreText run shaping to merge cells.
    fn test_shaping_context(family: &Arc<str>) -> ShapingContext<'_> {
        ShapingContext {
            family,
            font_families: None,
            font_size: 14.,
            features: &[],
            shaping_break_cursor: true,
            ligatures_enabled: false,
        }
    }

    fn theme() -> &'static TerminalTheme {
        let name = theme_catalog()
            .dark_names()
            .into_iter()
            .find(|name| name == "Catppuccin Mocha")
            .unwrap();
        AppSettings {
            dark_theme: name,
            theme_mode: ThemeMode::Dark,
            ..Default::default()
        }
        .effective_theme(true)
    }

    fn snapshot(cells: Vec<TerminalCell>) -> TerminalSnapshot {
        TerminalSnapshot {
            session_id: Uuid::nil(),
            size: TerminalSize {
                columns: 8,
                rows: 1,
                ..TerminalSize::default()
            },
            cells,
            color_overrides: Vec::new(),
            cursor_line: 0,
            cursor_column: 0,
            cursor_shape: TerminalCursorShape::Hidden,
            cursor_width: 1,
            cursor_blinking: false,
            title: String::new(),
            revision: 0,
            last_input_sequence: 0,
            input_modes: Default::default(),
            images: Vec::new(),
            image_placements: Vec::new(),
            selection: None,
            detected_links: Vec::new(),
            display_offset: 0,
            history_size: 0,
        }
    }

    fn cell(column: u16, character: char, foreground: TerminalColor, flags: Flags) -> TerminalCell {
        TerminalCell {
            line: 0,
            column,
            character,
            zerowidth: Vec::new(),
            foreground,
            background: TerminalColor::Named(257),
            underline_color: None,
            hyperlink: None,
            flags: flags.bits(),
        }
    }

    #[test]
    fn dynamic_palette_overrides_preserve_semantic_color_references() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.color_overrides = vec![TerminalColorOverride {
            index: 4,
            color: 0x123456ff,
        }];
        let palette = TerminalPalette::new(&snapshot, theme());
        assert_eq!(
            palette.foreground(&TerminalColor::Indexed(4), Flags::empty()),
            0x123456ff
        );
        assert_eq!(
            palette.foreground(&TerminalColor::Named(4), Flags::empty()),
            0x123456ff
        );
    }

    #[test]
    fn alacritty_bold_dim_and_inverse_color_rules_are_applied() {
        let snapshot = snapshot(Vec::new());
        let palette = TerminalPalette::new(&snapshot, theme());
        assert_eq!(
            palette.foreground(&TerminalColor::Indexed(2), Flags::BOLD),
            palette.color(10)
        );
        assert_eq!(
            palette.foreground(&TerminalColor::Named(2), Flags::DIM),
            palette.color(261)
        );
        assert_eq!(
            palette.foreground(&TerminalColor::Rgb(0x6496c8ff), Flags::DIM),
            0x426384ff
        );
        let inverse = cell(0, 'x', TerminalColor::Named(1), Flags::INVERSE);
        let resolved = palette.resolve_cell(&inverse, false);
        assert_eq!(resolved.foreground, palette.color(257));
        assert_eq!(resolved.background, palette.color(1));
    }

    #[test]
    fn wide_combining_and_sprite_cells_stay_on_exact_grid_columns() {
        let mut wide = cell(1, '界', TerminalColor::Named(256), Flags::WIDE_CHAR);
        wide.zerowidth.push('\u{fe0f}');
        let spacer = cell(2, ' ', TerminalColor::Named(256), Flags::WIDE_CHAR_SPACER);
        let block = cell(3, '▒', TerminalColor::Named(256), Flags::empty());
        let snapshot = snapshot(vec![wide, spacer, block]);
        let palette = TerminalPalette::new(&snapshot, theme());
        let glyphs = prepare_glyphs(&terminal_grid(&snapshot), &snapshot, &palette, 8., 18., 1., &test_shaping_context(&menlo_family()));
        assert_eq!(glyphs.len(), 2);
        let MetalCommand::Glyph {
            rect, text, sprite, ..
        } = &glyphs[0]
        else {
            panic!()
        };
        assert_eq!(*rect, [8., 0., 16., 18.]);
        assert_eq!(text.as_ref(), "界\u{fe0f}");
        assert!(!sprite);
        let MetalCommand::Glyph { rect, sprite, .. } = &glyphs[1] else {
            panic!()
        };
        assert_eq!(*rect, [24., 0., 8., 18.]);
        assert!(sprite);
    }

    #[test]
    fn split_emoji_components_are_rasterized_as_complete_graphemes() {
        let foreground = || TerminalColor::Named(256);
        let mut woman = cell(0, '👩', foreground(), Flags::WIDE_CHAR);
        woman.zerowidth.push('\u{200d}');
        let woman_spacer = cell(1, ' ', foreground(), Flags::WIDE_CHAR_SPACER);
        let microscope = cell(2, '🔬', foreground(), Flags::WIDE_CHAR);
        let microscope_spacer = cell(3, ' ', foreground(), Flags::WIDE_CHAR_SPACER);

        let fist = cell(4, '✊', foreground(), Flags::WIDE_CHAR);
        let fist_spacer = cell(5, ' ', foreground(), Flags::WIDE_CHAR_SPACER);
        let skin_tone = cell(6, '🏿', foreground(), Flags::WIDE_CHAR);
        let skin_tone_spacer = cell(7, ' ', foreground(), Flags::WIDE_CHAR_SPACER);

        let regional_a = cell(8, '🇦', foreground(), Flags::empty());
        let regional_q = cell(9, '🇶', foreground(), Flags::empty());

        let mut black_flag = cell(10, '🏴', foreground(), Flags::WIDE_CHAR);
        black_flag.zerowidth.push('\u{200d}');
        let black_flag_spacer = cell(11, ' ', foreground(), Flags::WIDE_CHAR_SPACER);
        let mut skull = cell(12, '☠', foreground(), Flags::empty());
        skull.zerowidth.push('\u{fe0f}');

        let water_polo = cell(13, '🤽', foreground(), Flags::WIDE_CHAR);
        let water_polo_spacer = cell(14, ' ', foreground(), Flags::WIDE_CHAR_SPACER);
        let mut medium_skin_tone = cell(15, '🏼', foreground(), Flags::WIDE_CHAR);
        medium_skin_tone.zerowidth.push('\u{200d}');
        let medium_skin_tone_spacer = cell(16, ' ', foreground(), Flags::WIDE_CHAR_SPACER);
        let mut female = cell(17, '♀', foreground(), Flags::empty());
        female.zerowidth.push('\u{fe0f}');
        let trailing_text = cell(18, 'x', foreground(), Flags::empty());

        let mut snapshot = snapshot(vec![
            woman,
            woman_spacer,
            microscope,
            microscope_spacer,
            fist,
            fist_spacer,
            skin_tone,
            skin_tone_spacer,
            regional_a,
            regional_q,
            black_flag,
            black_flag_spacer,
            skull,
            water_polo,
            water_polo_spacer,
            medium_skin_tone,
            medium_skin_tone_spacer,
            female,
            trailing_text,
        ]);
        snapshot.size.columns = 24;
        let palette = TerminalPalette::new(&snapshot, theme());
        let glyphs = prepare_glyphs(&terminal_grid(&snapshot), &snapshot, &palette, 8., 18., 1., &test_shaping_context(&menlo_family()));
        let glyph_data = glyphs
            .iter()
            .map(|command| match command {
                MetalCommand::Glyph { rect, text, .. } => (rect[0], rect[2], text.as_ref()),
                _ => panic!(),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            glyph_data,
            vec![
                (0., 16., "👩‍🔬"),
                (32., 16., "✊🏿"),
                (64., 16., "🇦🇶"),
                (80., 16., "🏴‍☠️"),
                (104., 16., "🤽🏼‍♀️"),
                (144., 8., "x"),
            ]
        );
    }

    #[test]
    fn grapheme_reconstruction_does_not_cross_style_boundaries() {
        let first = cell(0, '🇦', TerminalColor::Named(1), Flags::empty());
        let second = cell(1, '🇶', TerminalColor::Named(2), Flags::empty());
        let snapshot = snapshot(vec![first, second]);
        let palette = TerminalPalette::new(&snapshot, theme());
        let glyphs = prepare_glyphs(&terminal_grid(&snapshot), &snapshot, &palette, 8., 18., 1., &test_shaping_context(&menlo_family()));
        assert_eq!(glyphs.len(), 2);
    }

    #[test]
    fn minimum_contrast_metadata_skips_terminal_graphics_but_covers_decorations() {
        let theme = theme();
        let low_contrast = (theme.background << 8) | 0xff;
        let text = cell(0, 'x', TerminalColor::Rgb(low_contrast), Flags::UNDERLINE);
        let graphic = cell(1, '▒', TerminalColor::Rgb(low_contrast), Flags::empty());
        let font_powerline = cell(
            2,
            '\u{e0c0}',
            TerminalColor::Rgb(low_contrast),
            Flags::empty(),
        );
        let snapshot = snapshot(vec![text, graphic, font_powerline]);
        let palette = TerminalPalette::new(&snapshot, theme);
        let grid = terminal_grid(&snapshot);
        let glyphs = prepare_glyphs(
            &grid,
            &snapshot,
            &palette,
            8.,
            18.,
            3.,
            &test_shaping_context(&menlo_family()),
        );

        let MetalCommand::Glyph {
            background,
            minimum_contrast,
            minimum_contrast_disabled,
            ..
        } = &glyphs[0]
        else {
            panic!()
        };
        assert_eq!(*background, palette.color(257));
        assert_eq!(*minimum_contrast, 3.);
        assert!(!*minimum_contrast_disabled);
        let MetalCommand::Glyph {
            minimum_contrast_disabled,
            ..
        } = &glyphs[1]
        else {
            panic!()
        };
        assert!(*minimum_contrast_disabled);
        let MetalCommand::Glyph {
            sprite,
            minimum_contrast_disabled,
            ..
        } = &glyphs[2]
        else {
            panic!()
        };
        assert!(!*sprite);
        assert!(*minimum_contrast_disabled);

        let decorations = prepare_decorations(
            &grid,
            &palette,
            8.,
            18.,
            3.,
            ResolvedMetricAdjustments::default(),
        );
        let MetalCommand::Rect { color, .. } = &decorations[0] else {
            panic!()
        };
        assert_eq!(
            *color,
            minimum_contrast_rgba(low_contrast, palette.color(257), 3.)
        );
    }

    #[test]
    fn every_alacritty_decoration_flag_produces_metal_geometry() {
        for flag in [
            Flags::UNDERLINE,
            Flags::DOUBLE_UNDERLINE,
            Flags::UNDERCURL,
            Flags::DOTTED_UNDERLINE,
            Flags::DASHED_UNDERLINE,
            Flags::STRIKEOUT,
        ] {
            let snapshot = snapshot(vec![cell(0, 'x', TerminalColor::Named(256), flag)]);
            let palette = TerminalPalette::new(&snapshot, theme());
            let commands = prepare_decorations(
                &terminal_grid(&snapshot),
                &palette,
                8.,
                18.,
                1.,
                ResolvedMetricAdjustments::default(),
            );
            assert!(!commands.is_empty(), "missing geometry for {flag:?}");
        }
    }

    #[test]
    fn every_cursor_shape_is_represented_in_the_metal_scene() {
        for (shape, backgrounds, foregrounds) in [
            (TerminalCursorShape::Block, 1, 0),
            (TerminalCursorShape::Underline, 0, 1),
            (TerminalCursorShape::Beam, 0, 1),
            (TerminalCursorShape::HollowBlock, 0, 4),
            (TerminalCursorShape::Hidden, 0, 0),
        ] {
            let mut snapshot = snapshot(Vec::new());
            snapshot.cursor_shape = shape;
            let palette = TerminalPalette::new(&snapshot, theme());
            let (background, foreground) =
                prepare_cursor(&snapshot, &palette, 8., 18., ResolvedMetricAdjustments::default());
            assert_eq!(background.len(), backgrounds);
            assert_eq!(foreground.len(), foregrounds);
        }
    }

    #[test]
    fn adjust_underline_thickness_and_position_shift_the_decoration_rect() {
        let snapshot = snapshot(vec![cell(0, 'x', TerminalColor::Named(256), Flags::UNDERLINE)]);
        let palette = TerminalPalette::new(&snapshot, theme());
        let grid = terminal_grid(&snapshot);
        // Baseline: default underline is 1px tall, near the bottom of an 18px cell.
        let base = prepare_decorations(&grid, &palette, 8., 18., 1., ResolvedMetricAdjustments::default());
        let MetalCommand::Rect { rect: base_rect, .. } = base[0] else {
            panic!("underline must be a rect");
        };
        assert_eq!(base_rect[3], 1., "default underline thickness is 1px");

        // Thicken to 3px and lift the position higher up the cell.
        let adjustments = ResolvedMetricAdjustments {
            underline_thickness: Some(crate::settings::MetricModifier::Absolute(2)),
            underline_position: Some(crate::settings::MetricModifier::Absolute(-4)),
            ..ResolvedMetricAdjustments::default()
        };
        let adjusted = prepare_decorations(&grid, &palette, 8., 18., 1., adjustments);
        let MetalCommand::Rect { rect: adjusted_rect, .. } = adjusted[0] else {
            panic!("underline must be a rect");
        };
        assert_eq!(adjusted_rect[3], 3., "thickness +2 → 3px");
        assert!(
            adjusted_rect[1] < base_rect[1],
            "position -4 lifts the underline ({} !< {})",
            adjusted_rect[1],
            base_rect[1]
        );
    }

    #[test]
    fn adjust_cursor_thickness_widens_the_bar_cursor() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.cursor_shape = TerminalCursorShape::Beam;
        let palette = TerminalPalette::new(&snapshot, theme());
        let (_, base) =
            prepare_cursor(&snapshot, &palette, 8., 18., ResolvedMetricAdjustments::default());
        let MetalCommand::Rect { rect: base_rect, .. } = base[0] else {
            panic!("beam cursor must be a rect");
        };
        assert_eq!(base_rect[2], 2., "default beam is 2px wide");

        let adjustments = ResolvedMetricAdjustments {
            cursor_thickness: Some(crate::settings::MetricModifier::Absolute(2)),
            ..ResolvedMetricAdjustments::default()
        };
        let (_, adjusted) = prepare_cursor(&snapshot, &palette, 8., 18., adjustments);
        let MetalCommand::Rect { rect: adjusted_rect, .. } = adjusted[0] else {
            panic!("beam cursor must be a rect");
        };
        assert_eq!(adjusted_rect[2], 4., "thickness +2 → 4px wide beam");
    }

    #[test]
    fn selection_and_ime_geometry_stay_on_terminal_cells() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.size.rows = 3;
        snapshot.cursor_line = 1;
        snapshot.cursor_column = 2;
        let selection = TerminalSelection {
            anchor: TerminalPoint { line: 0, column: 6 },
            head: TerminalPoint { line: 2, column: 1 },
        };
        let commands = prepare_selection(&snapshot, Some(selection), 8., 18., 0x12345678);
        assert_eq!(commands.len(), 3);
        assert!(matches!(
            commands[0],
            MetalCommand::Rect {
                rect: [48., 0., 16., 18.],
                color: 0x12345678
            }
        ));
        assert!(matches!(
            commands[2],
            MetalCommand::Rect {
                rect: [0., 36., 16., 18.],
                color: 0x12345678
            }
        ));

        let palette = TerminalPalette::new(&snapshot, theme());
        let mut ime_commands = Vec::new();
        prepare_ime(
            &snapshot,
            &TerminalImeState {
                text: "a界".to_owned(),
                selected_range: 2..2,
            },
            &palette,
            8.,
            18.,
            1.,
            &mut ime_commands,
        );
        let MetalCommand::Glyph { rect, text, .. } = &ime_commands[1] else {
            panic!("IME must produce a glyph command")
        };
        assert_eq!(*rect, [16., 18., 24., 18.]);
        assert_eq!(text.as_ref(), "a界");
    }

    #[test]
    fn url_hover_underlines_the_hovered_range() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.size.rows = 3;
        let link = TerminalLinkRange {
            start: TerminalCellPosition { line: 1, column: 2 },
            end: TerminalCellPosition { line: 1, column: 5 },
            url: "https://example.com".to_owned(),
        };
        let commands = prepare_url_hover(Some(&link), &snapshot, 8., 18., 0xabcdef99);
        assert_eq!(commands.len(), 1);
        let MetalCommand::Rect { rect, color } = commands[0] else {
            panic!("hover underline must be a rect");
        };
        assert_eq!(color, 0xabcdef99);
        // Spans columns 2..=5 (x = 16, width = 4 cells = 32) and sits at the bottom of row 1.
        assert_eq!(rect[0], 16.);
        assert_eq!(rect[2], 32.);
        let thickness = rect[3];
        assert!((1.0..=2.0).contains(&thickness), "thickness {thickness}");
        assert!((rect[1] - (2. * 18. - thickness)).abs() < 0.001, "y = {}", rect[1]);
        // No hover → no commands.
        assert!(prepare_url_hover(None, &snapshot, 8., 18., 0xabcdef99).is_empty());
    }

    #[test]
    fn ime_candidate_offsets_follow_utf16_and_display_width() {
        assert_eq!(display_width_for_utf16_prefix("a界😀", 0), 0);
        assert_eq!(display_width_for_utf16_prefix("a界😀", 1), 1);
        assert_eq!(display_width_for_utf16_prefix("a界😀", 2), 3);
        assert_eq!(display_width_for_utf16_prefix("a界😀", 3), 3);
        assert_eq!(display_width_for_utf16_prefix("a界😀", 4), 5);
    }

    #[test]
    fn terminal_images_are_prepared_without_copying_pixels_and_keep_z_layers() {
        let mut snapshot = snapshot(Vec::new());
        let image_key = TerminalImageKey {
            id: 7,
            generation: 9,
        };
        let texture_key = TerminalTextureKey {
            session_id: snapshot.session_id,
            image: image_key,
        };
        let pixels = Arc::new(vec![255; 4 * 4 * 4]);
        let image = Arc::new(TerminalImageData {
            key: texture_key,
            width: 4,
            height: 4,
            pixels: Arc::clone(&pixels),
        });
        let mut image_cache = FxHashMap::default();
        image_cache.insert(texture_key, Arc::clone(&image));
        let placement = |z| eggie_protocol::TerminalImagePlacement {
            image: image_key,
            placement_id: z as u32,
            line: 2,
            column: 3,
            source_x: 1,
            source_y: 1,
            source_width: 2,
            source_height: 2,
            x_offset: 2,
            y_offset: 3,
            columns: 2,
            rows: 1,
            destination_width: 14,
            destination_height: 15,
            z,
        };
        snapshot.image_placements = vec![placement(i32::MIN), placement(-1), placement(0)];

        let (below_background, below_text, above_text) =
            prepare_images(&snapshot, 8., 18., 1., &image_cache);
        assert_eq!(
            (below_background.len(), below_text.len(), above_text.len()),
            (1, 1, 1)
        );
        let MetalCommand::Image {
            image: prepared,
            rect,
            source,
        } = &below_text[0]
        else {
            panic!("image placement must stay on the dedicated Metal image path")
        };
        assert!(Arc::ptr_eq(prepared, &image));
        assert!(Arc::ptr_eq(&prepared.pixels, &pixels));
        assert_eq!(*rect, [26., 39., 14., 15.]);
        assert_eq!(*source, [0.25, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn terminal_image_layers_match_kitty_cell_render_order() {
        let mut background_cell = cell(0, 'x', TerminalColor::Named(256), Flags::empty());
        background_cell.background = TerminalColor::Indexed(1);
        let mut snapshot = snapshot(vec![background_cell]);
        let mut image_cache = FxHashMap::default();
        for (id, z) in [(1, i32::MIN), (2, -1), (3, 0)] {
            let image_key = TerminalImageKey { id, generation: 1 };
            let texture_key = TerminalTextureKey {
                session_id: snapshot.session_id,
                image: image_key,
            };
            image_cache.insert(
                texture_key,
                Arc::new(TerminalImageData {
                    key: texture_key,
                    width: 1,
                    height: 1,
                    pixels: Arc::new(vec![255; 4]),
                }),
            );
            snapshot
                .image_placements
                .push(eggie_protocol::TerminalImagePlacement {
                    image: image_key,
                    placement_id: id,
                    line: 0,
                    column: id,
                    source_x: 0,
                    source_y: 0,
                    source_width: 1,
                    source_height: 1,
                    x_offset: 0,
                    y_offset: 0,
                    columns: 1,
                    rows: 1,
                    destination_width: 8,
                    destination_height: 18,
                    z,
                });
        }
        let prepared = prepare_terminal(
            &snapshot,
            theme(),
            64.,
            18.,
            8.,
            18.,
            1.,
            Arc::from("Menlo"),
            14.,
            0x3399ffff,
            1.,
            None,
            None,
            None,
            None,
            true,
            ResolvedMetricAdjustments::default(),
            None,
            Arc::from(&[][..]),
            false,
            true,
            Arc::from(&[][..]),
            None,
            Arc::from(&[][..]),
            &image_cache,
        );
        let image_index = |id| {
            prepared
                .commands
                .iter()
                .position(|command| {
                    matches!(command, MetalCommand::Image { image, .. } if image.key.image.id == id)
                })
                .unwrap()
        };
        let non_default_background = prepared
            .commands
            .iter()
            .position(|command| {
                matches!(command, MetalCommand::Rect { rect, .. } if *rect == [0., 0., 8., 18.])
            })
            .unwrap();
        let glyph = prepared
            .commands
            .iter()
            .position(|command| matches!(command, MetalCommand::Glyph { .. }))
            .unwrap();
        assert!(matches!(
            prepared.commands.first(),
            Some(MetalCommand::Rect {
                rect: [0., 0., 64., 18.],
                ..
            })
        ));
        assert!(image_index(1) < non_default_background);
        assert!(non_default_background < image_index(2));
        assert!(image_index(2) < glyph);
        assert!(glyph < image_index(3));
    }

    #[test]
    fn terminal_image_layout_uses_device_pixels_without_retina_drift() {
        let mut snapshot = snapshot(Vec::new());
        snapshot.size.cell_width = 17;
        snapshot.size.cell_height = 36;
        let image_key = TerminalImageKey {
            id: 7,
            generation: 1,
        };
        let texture_key = TerminalTextureKey {
            session_id: snapshot.session_id,
            image: image_key,
        };
        let mut image_cache = FxHashMap::default();
        image_cache.insert(
            texture_key,
            Arc::new(TerminalImageData {
                key: texture_key,
                width: 4,
                height: 4,
                pixels: Arc::new(vec![255; 64]),
            }),
        );
        snapshot
            .image_placements
            .push(eggie_protocol::TerminalImagePlacement {
                image: image_key,
                placement_id: 1,
                line: 0,
                column: 3,
                source_x: 0,
                source_y: 0,
                source_width: 4,
                source_height: 4,
                x_offset: 4,
                y_offset: 6,
                columns: 2,
                rows: 1,
                destination_width: 30,
                destination_height: 30,
                z: 0,
            });
        let (_, _, above) = prepare_images(&snapshot, 8.5, 18., 2., &image_cache);
        let MetalCommand::Image { rect, .. } = &above[0] else {
            panic!("image command expected")
        };
        assert_eq!(*rect, [27.5, 3., 15., 15.]);
    }

    #[test]
    fn terminal_image_fragments_preserve_core_clipped_destination_geometry() {
        let mut snapshot = snapshot(Vec::new());
        let image_key = TerminalImageKey {
            id: 12,
            generation: 1,
        };
        let texture_key = TerminalTextureKey {
            session_id: snapshot.session_id,
            image: image_key,
        };
        let mut image_cache = FxHashMap::default();
        image_cache.insert(
            texture_key,
            Arc::new(TerminalImageData {
                key: texture_key,
                width: 32,
                height: 32,
                pixels: Arc::new(vec![255; 32 * 32 * 4]),
            }),
        );
        snapshot
            .image_placements
            .push(eggie_protocol::TerminalImagePlacement {
                image: image_key,
                placement_id: 5,
                line: 1,
                column: 2,
                source_x: 4,
                source_y: 8,
                source_width: 9,
                source_height: 12,
                x_offset: 6,
                y_offset: 4,
                // A Unicode placeholder run can span two cells while its letterboxed source only
                // occupies part of that box. These fields must not override the clipped destination.
                columns: 2,
                rows: 1,
                destination_width: 9,
                destination_height: 12,
                z: -1,
            });

        let (_, below_text, _) = prepare_images(&snapshot, 8., 18., 1., &image_cache);
        let MetalCommand::Image { rect, .. } = &below_text[0] else {
            panic!("image command expected")
        };
        assert_eq!(*rect, [22., 22., 9., 12.]);
    }

    #[test]
    #[ignore = "manual latency benchmark"]
    fn benchmark_dense_terminal_preparation() {
        let columns = 229;
        let rows = 74;
        let mut cells = Vec::with_capacity(columns as usize * rows as usize);
        for line in 0..rows {
            for column in 0..columns {
                let mut terminal_cell = cell(
                    column,
                    char::from(b'a' + (column % 26) as u8),
                    TerminalColor::Indexed((column % 16) as u8),
                    if column % 17 == 0 {
                        Flags::UNDERLINE
                    } else {
                        Flags::empty()
                    },
                );
                terminal_cell.line = line;
                if column % 11 == 0 {
                    terminal_cell.background = TerminalColor::Indexed((column % 8) as u8);
                }
                cells.push(terminal_cell);
            }
        }
        let mut snapshot = snapshot(cells);
        snapshot.size = TerminalSize {
            columns,
            rows,
            ..TerminalSize::default()
        };
        let iterations = 200;
        let started = Instant::now();
        let mut command_count = 0;
        let image_cache = FxHashMap::default();
        for _ in 0..iterations {
            command_count = std::hint::black_box(prepare_terminal(
                &snapshot,
                theme(),
                columns as f32 * 8.,
                rows as f32 * 18.,
                8.,
                18.,
                1.,
                Arc::from("Menlo"),
                14.,
                0x3399ffff,
                1.,
                None,
                None,
                None,
                None,
                true,
                ResolvedMetricAdjustments::default(),
                None,
                Arc::from(&[][..]),
                false,
                true,
                Arc::from(&[][..]),
                None,
                Arc::from(&[][..]),
                &image_cache,
            ))
            .commands
            .len();
        }
        let average = started.elapsed() / iterations;
        eprintln!(
            "dense terminal preparation: {columns}x{rows}, {command_count} commands, average {:.3}ms",
            average.as_secs_f64() * 1_000.
        );
    }
}
