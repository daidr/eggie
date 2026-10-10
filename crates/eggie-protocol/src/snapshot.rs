use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalColor {
    Rgb(u32),
    Indexed(u8),
    Named(u16),
}

pub use eggie_domain::{DEFAULT_PALETTE, TerminalAppearance};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColorOverride {
    #[serde(rename = "i")]
    pub index: u16,
    /// Dynamic terminal color encoded as `0xRRGGBBAA`.
    #[serde(rename = "c")]
    pub color: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    #[serde(rename = "l")]
    pub line: u16,
    #[serde(rename = "c")]
    pub column: u16,
    #[serde(rename = "ch")]
    pub character: char,
    #[serde(rename = "z", default, skip_serializing_if = "Vec::is_empty")]
    pub zerowidth: Vec<char>,
    #[serde(rename = "fg")]
    pub foreground: TerminalColor,
    #[serde(rename = "bg")]
    pub background: TerminalColor,
    #[serde(rename = "u", default, skip_serializing_if = "Option::is_none")]
    pub underline_color: Option<TerminalColor>,
    #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<String>,
    #[serde(rename = "f")]
    /// Bits from `eggie_domain::TerminalCellFlags`, never engine-private flags.
    pub flags: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerminalImageKey {
    #[serde(rename = "i")]
    pub id: u32,
    #[serde(rename = "g")]
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalImageDescriptor {
    #[serde(rename = "k")]
    pub key: TerminalImageKey,
    #[serde(rename = "w")]
    pub width: u32,
    #[serde(rename = "h")]
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalImagePlacement {
    #[serde(rename = "k")]
    pub image: TerminalImageKey,
    #[serde(rename = "p")]
    pub placement_id: u32,
    #[serde(rename = "l")]
    pub line: i32,
    #[serde(rename = "c")]
    pub column: u32,
    #[serde(rename = "x")]
    pub source_x: u32,
    #[serde(rename = "y")]
    pub source_y: u32,
    #[serde(rename = "w")]
    pub source_width: u32,
    #[serde(rename = "h")]
    pub source_height: u32,
    #[serde(rename = "xo", default, skip_serializing_if = "is_zero_u32")]
    pub x_offset: u32,
    #[serde(rename = "yo", default, skip_serializing_if = "is_zero_u32")]
    pub y_offset: u32,
    #[serde(rename = "cs")]
    pub columns: u32,
    #[serde(rename = "rs")]
    pub rows: u32,
    #[serde(rename = "dw")]
    pub destination_width: u32,
    #[serde(rename = "dh")]
    pub destination_height: u32,
    #[serde(rename = "z", default, skip_serializing_if = "is_zero_i32")]
    pub z: i32,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    #[serde(rename = "id")]
    pub session_id: SessionId,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    #[serde(rename = "c")]
    pub cells: Vec<TerminalCell>,
    #[serde(rename = "o", default, skip_serializing_if = "Vec::is_empty")]
    pub color_overrides: Vec<TerminalColorOverride>,
    #[serde(rename = "cl")]
    pub cursor_line: u16,
    #[serde(rename = "cc")]
    pub cursor_column: u16,
    #[serde(rename = "cs")]
    pub cursor_shape: TerminalCursorShape,
    #[serde(rename = "cw")]
    pub cursor_width: u8,
    #[serde(rename = "cb", default, skip_serializing_if = "is_false")]
    pub cursor_blinking: bool,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "is", default, skip_serializing_if = "is_zero_u64")]
    pub last_input_sequence: u64,
    #[serde(rename = "im", default, skip_serializing_if = "is_default_input_modes")]
    pub input_modes: TerminalInputModes,
    #[serde(rename = "gi", default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<TerminalImageDescriptor>,
    #[serde(rename = "gp", default, skip_serializing_if = "Vec::is_empty")]
    pub image_placements: Vec<TerminalImagePlacement>,
    #[serde(rename = "sel", default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TerminalSelectionRange>,
    #[serde(rename = "dl", default, skip_serializing_if = "Vec::is_empty")]
    pub detected_links: Vec<TerminalLinkRange>,
    /// Current scroll offset into the scrollback: 0 = at the live bottom, `history_size` = scrolled
    /// to the oldest line. Used by the UI to position the scrollbar thumb.
    #[serde(rename = "do", default, skip_serializing_if = "is_zero_u32")]
    pub display_offset: u32,
    /// Number of scrollback lines above the live screen. 0 = no scrollback (scrollbar hidden).
    #[serde(rename = "hs", default, skip_serializing_if = "is_zero_u32")]
    pub history_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerminalCellPosition {
    #[serde(rename = "l")]
    pub line: u16,
    #[serde(rename = "c")]
    pub column: u16,
}

/// Direction to advance when moving between terminal search matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSearchDirection {
    /// Toward the end of the buffer (later output).
    #[default]
    Forward,
    /// Toward the start of the buffer (earlier output / scrollback).
    Backward,
}

/// A terminal search query issued by the client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchRequest {
    /// The query text. Interpreted literally unless [`Self::regex`] is set.
    pub query: String,
    /// When true, `query` is a regular expression; otherwise it is matched literally.
    #[serde(default)]
    pub regex: bool,
    /// Which way to move from the current active match to select the next one.
    #[serde(default)]
    pub direction: TerminalSearchDirection,
    /// When true this is the first query of a session (typing a new pattern) and the search should
    /// start from the current viewport rather than advancing past the previous active match.
    #[serde(default)]
    pub fresh: bool,
}

/// A single terminal search match, in viewport-relative coordinates. Both endpoints are inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchMatch {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
}

/// Result of a [`ClientRequest::TerminalSearch`]. When `active` is `None` the query did not match
/// anywhere in the buffer. `matches` lists every match currently visible in the viewport (which the
/// daemon has already scrolled so that `active` is on screen); `total` counts all matches across
/// the whole buffer and `index` is the 0-based position of the active match within that total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchResult {
    #[serde(rename = "a", default, skip_serializing_if = "Option::is_none")]
    pub active: Option<TerminalSearchMatch>,
    #[serde(rename = "m", default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<TerminalSearchMatch>,
    #[serde(rename = "i", default)]
    pub index: usize,
    #[serde(rename = "n", default)]
    pub total: usize,
    /// Revision of the snapshot these coordinates apply to. The client should render the highlight
    /// against a snapshot with at least this revision.
    #[serde(rename = "r")]
    pub revision: u64,
}

/// Which edge of a cell a selection endpoint sits on. Mirrors the terminal core's `Side`; the client
/// derives it from the sub-cell x position of the mouse (left half vs right half).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSelectionSide {
    #[default]
    Left,
    Right,
}

/// The semantic kind of an interactive selection, chosen by click count. Mirrors the terminal core's
/// `SelectionType` (minus `Block`, which is reserved for a future rectangular-selection modifier).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSelectionKind {
    /// Character-granularity selection (single click + drag).
    #[default]
    Simple,
    /// Word/semantic selection (double click).
    Semantic,
    /// Whole-line selection (triple click).
    Lines,
    /// Rectangular/block selection.
    Block,
}

/// The active selection projected into the current viewport, in viewport-relative coordinates. Both
/// endpoints are inclusive. The daemon owns the authoritative selection (a terminal-core `Selection`
/// spanning scrollback); this is only its visible projection for the current snapshot. `None` in a
/// snapshot means the selection is empty or entirely scrolled out of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSelectionRange {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
    #[serde(rename = "b", default, skip_serializing_if = "is_false")]
    pub is_block: bool,
}

/// A URL auto-detected in the terminal output, in viewport-relative coordinates. Both endpoints are
/// inclusive. A URL spanning multiple rows (soft-wrapped) is emitted as one range per row so the
/// client can draw and hit-test each row independently. Distinct from [`TerminalCell::hyperlink`]
/// (explicit OSC 8 links), which take priority when both cover the same cell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLinkRange {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
    #[serde(rename = "u")]
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshotDelta {
    #[serde(rename = "id")]
    pub session_id: SessionId,
    #[serde(rename = "b")]
    pub base_revision: u64,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    #[serde(rename = "c", default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<TerminalCell>,
    #[serde(rename = "x", default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<TerminalCellPosition>,
    #[serde(rename = "o", default, skip_serializing_if = "Vec::is_empty")]
    pub color_overrides: Vec<TerminalColorOverride>,
    #[serde(rename = "cl")]
    pub cursor_line: u16,
    #[serde(rename = "cc")]
    pub cursor_column: u16,
    #[serde(rename = "cs")]
    pub cursor_shape: TerminalCursorShape,
    #[serde(rename = "cw")]
    pub cursor_width: u8,
    #[serde(rename = "cb", default, skip_serializing_if = "is_false")]
    pub cursor_blinking: bool,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "is", default, skip_serializing_if = "is_zero_u64")]
    pub last_input_sequence: u64,
    #[serde(rename = "im", default, skip_serializing_if = "is_default_input_modes")]
    pub input_modes: TerminalInputModes,
    #[serde(rename = "gi", default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<TerminalImageDescriptor>>,
    #[serde(rename = "gp", default, skip_serializing_if = "Option::is_none")]
    pub image_placements: Option<Vec<TerminalImagePlacement>>,
    #[serde(rename = "sel", default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TerminalSelectionRange>,
    #[serde(rename = "dl", default, skip_serializing_if = "Vec::is_empty")]
    pub detected_links: Vec<TerminalLinkRange>,
    #[serde(rename = "do", default, skip_serializing_if = "is_zero_u32")]
    pub display_offset: u32,
    #[serde(rename = "hs", default, skip_serializing_if = "is_zero_u32")]
    pub history_size: u32,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_default_input_modes(value: &TerminalInputModes) -> bool {
    *value == TerminalInputModes::default()
}

impl TerminalSnapshot {
    pub fn plain_lines(&self) -> Vec<String> {
        let mut rows = vec![vec![' '; self.size.columns as usize]; self.size.rows as usize];
        for cell in &self.cells {
            if let Some(character) = rows
                .get_mut(cell.line as usize)
                .and_then(|row| row.get_mut(cell.column as usize))
            {
                *character = cell.character;
            }
        }
        rows.into_iter()
            .map(|row| row.into_iter().collect::<String>().trim_end().to_owned())
            .collect()
    }

    /// Whether a URL scheme separator (`://` or `mailto:`) appears on any row, scanning the sparse
    /// `cells` directly without materializing the grid.
    ///
    /// Equivalent to `plain_lines().iter().any(|l| l.contains("://") || l.contains("mailto:"))`
    /// but allocation-free: `cells` is emitted row-major with strictly ascending columns (the grid
    /// iterator's order, preserved through the empty-cell filter), so a scheme substring can only
    /// span cells that are on the same row at consecutive columns. This runs on every published
    /// frame as a cheap precheck that short-circuits the regex pass on high-throughput output,
    /// which rarely contains URLs; `plain_lines()`'s per-row `String` allocation made that precheck
    /// as expensive as the thing it guards.
    pub fn has_scheme_candidate(&self) -> bool {
        // Longest needle is `mailto:` (7 chars). Track the current run of consecutive same-row
        // cells as a small ring so a match check never allocates.
        const MAILTO: [char; 7] = ['m', 'a', 'i', 'l', 't', 'o', ':'];
        const SCHEME_SEP: [char; 3] = [':', '/', '/'];
        let mut window: [char; 7] = [' '; 7];
        let mut len: usize = 0;
        let mut prev: Option<(u16, u16)> = None;
        for cell in &self.cells {
            // A gap (new row, or a skipped empty column) breaks character adjacency, so the needle
            // cannot straddle it: reset the run before considering this cell.
            let contiguous = matches!(prev, Some((line, column)) if line == cell.line && column + 1 == cell.column);
            if !contiguous {
                len = 0;
            }
            if len == window.len() {
                window.rotate_left(1);
                window[window.len() - 1] = cell.character;
            } else {
                window[len] = cell.character;
                len += 1;
            }
            if len >= SCHEME_SEP.len() && window[len - SCHEME_SEP.len()..len] == SCHEME_SEP {
                return true;
            }
            if len >= MAILTO.len() && window[len - MAILTO.len()..len] == MAILTO {
                return true;
            }
            prev = Some((cell.line, cell.column));
        }
        false
    }

    pub fn apply_delta(&self, delta: &TerminalSnapshotDelta) -> Option<Self> {
        if self.session_id != delta.session_id
            || self.revision != delta.base_revision
            || self.size != delta.size
        {
            return None;
        }

        let mut replaced = delta
            .cells
            .iter()
            .map(|cell| TerminalCellPosition {
                line: cell.line,
                column: cell.column,
            })
            .chain(delta.cleared.iter().copied())
            .collect::<Vec<_>>();
        replaced.sort_unstable();
        replaced.dedup();
        let mut cells = Vec::with_capacity(
            self.cells
                .len()
                .saturating_add(delta.cells.len())
                .saturating_sub(delta.cleared.len()),
        );
        cells.extend(
            self.cells
                .iter()
                .filter(|cell| {
                    replaced
                        .binary_search(&TerminalCellPosition {
                            line: cell.line,
                            column: cell.column,
                        })
                        .is_err()
                })
                .cloned(),
        );
        cells.extend(delta.cells.iter().cloned());
        cells.sort_unstable_by_key(|cell| (cell.line, cell.column));

        Some(Self {
            session_id: delta.session_id,
            size: delta.size,
            cells,
            color_overrides: delta.color_overrides.clone(),
            cursor_line: delta.cursor_line,
            cursor_column: delta.cursor_column,
            cursor_shape: delta.cursor_shape,
            cursor_width: delta.cursor_width,
            cursor_blinking: delta.cursor_blinking,
            title: delta.title.clone(),
            revision: delta.revision,
            last_input_sequence: delta.last_input_sequence,
            input_modes: delta.input_modes,
            images: delta.images.clone().unwrap_or_else(|| self.images.clone()),
            image_placements: delta
                .image_placements
                .clone()
                .unwrap_or_else(|| self.image_placements.clone()),
            selection: delta.selection,
            detected_links: delta.detected_links.clone(),
            display_offset: delta.display_offset,
            history_size: delta.history_size,
        })
    }
}
