// The engine currently shares the stable wire bit assignments. Enforce this at compile time
// before forwarding bits, so an engine upgrade cannot silently change rendered attributes.
const _: () = {
    assert!(Flags::INVERSE.bits() == eggie_domain::TerminalCellFlags::INVERSE.bits());
    assert!(Flags::BOLD.bits() == eggie_domain::TerminalCellFlags::BOLD.bits());
    assert!(Flags::ITALIC.bits() == eggie_domain::TerminalCellFlags::ITALIC.bits());
    assert!(Flags::UNDERLINE.bits() == eggie_domain::TerminalCellFlags::UNDERLINE.bits());
    assert!(Flags::WRAPLINE.bits() == eggie_domain::TerminalCellFlags::WRAPLINE.bits());
    assert!(Flags::WIDE_CHAR.bits() == eggie_domain::TerminalCellFlags::WIDE_CHAR.bits());
    assert!(
        Flags::WIDE_CHAR_SPACER.bits() == eggie_domain::TerminalCellFlags::WIDE_CHAR_SPACER.bits()
    );
    assert!(Flags::DIM.bits() == eggie_domain::TerminalCellFlags::DIM.bits());
    assert!(Flags::HIDDEN.bits() == eggie_domain::TerminalCellFlags::HIDDEN.bits());
    assert!(Flags::STRIKEOUT.bits() == eggie_domain::TerminalCellFlags::STRIKEOUT.bits());
    assert!(
        Flags::LEADING_WIDE_CHAR_SPACER.bits()
            == eggie_domain::TerminalCellFlags::LEADING_WIDE_CHAR_SPACER.bits()
    );
    assert!(
        Flags::DOUBLE_UNDERLINE.bits() == eggie_domain::TerminalCellFlags::DOUBLE_UNDERLINE.bits()
    );
    assert!(Flags::UNDERCURL.bits() == eggie_domain::TerminalCellFlags::UNDERCURL.bits());
    assert!(
        Flags::DOTTED_UNDERLINE.bits() == eggie_domain::TerminalCellFlags::DOTTED_UNDERLINE.bits()
    );
    assert!(
        Flags::DASHED_UNDERLINE.bits() == eggie_domain::TerminalCellFlags::DASHED_UNDERLINE.bits()
    );
};
use super::*;

#[cfg(test)]
pub(super) fn snapshot_terminal(
    terminal: &Term<DaemonEventListener>,
    session_id: SessionId,
    size: TerminalSize,
    title: String,
    revision: u64,
    last_input_sequence: u64,
) -> TerminalSnapshot {
    let content = terminal.renderable_content();
    let display_offset = content.display_offset;
    let cells: Vec<TerminalCell> = content
        .display_iter
        .filter_map(|indexed| {
            let point = point_to_viewport(display_offset, indexed.point)?;
            let cell = indexed.cell;
            if snapshot_cell_is_empty(cell) {
                return None;
            }
            Some(TerminalCell {
                line: point.line as u16,
                column: point.column.0 as u16,
                character: cell.c,
                zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
                foreground: snapshot_color(cell.fg),
                background: snapshot_color(cell.bg),
                underline_color: cell.underline_color().map(snapshot_color),
                hyperlink: cell.hyperlink().map(|hyperlink| hyperlink.uri().to_owned()),
                flags: cell.flags.bits(),
            })
        })
        .collect();
    snapshot_metadata(
        terminal,
        session_id,
        size,
        title,
        revision,
        last_input_sequence,
        cells.into(),
    )
}

pub(super) fn snapshot_terminal_incremental(
    terminal: &mut Term<DaemonEventListener>,
    base: Option<&TerminalSnapshot>,
    session_id: SessionId,
    size: TerminalSize,
    title: String,
    revision: u64,
    last_input_sequence: u64,
) -> TerminalSnapshot {
    let display_offset = terminal.grid().display_offset();
    let compatible = base
        .is_some_and(|base| base.size == size && base.display_offset as usize == display_offset);
    let dirty: Vec<usize> = match terminal.damage() {
        TermDamage::Partial(lines) if compatible => lines.map(|line| line.line).collect(),
        _ => (0..size.rows as usize).collect(),
    };
    let mut rows = if compatible {
        base.unwrap().cells.rows().to_vec()
    } else {
        Vec::new()
    };
    rows.resize_with(size.rows as usize, || TerminalRow::owned(Vec::new()));
    for line in dirty {
        if line >= rows.len() {
            continue;
        }
        let engine_row = &terminal.grid()[Line(line as i32 - display_offset as i32)];
        let cells = engine_row[..]
            .iter()
            .enumerate()
            .filter_map(|(column, cell)| {
                if snapshot_cell_is_empty(cell) {
                    return None;
                }
                Some(TerminalCell {
                    line: line as u16,
                    column: column as u16,
                    character: cell.c,
                    zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
                    foreground: snapshot_color(cell.fg),
                    background: snapshot_color(cell.bg),
                    underline_color: cell.underline_color().map(snapshot_color),
                    hyperlink: cell.hyperlink().map(|link| link.uri().to_owned()),
                    flags: cell.flags.bits(),
                })
            })
            .collect::<Vec<_>>();
        // Cursor damage may cover a row whose text did not change. Keep its storage identity.
        if !rows[line].iter().eq(cells.iter().map(TerminalCell::view)) {
            rows[line] = TerminalRow::owned(cells);
        }
    }
    let snapshot = snapshot_metadata(
        terminal,
        session_id,
        size,
        title,
        revision,
        last_input_sequence,
        TerminalCells::from_rows(rows),
    );
    terminal.reset_damage();
    snapshot
}

pub(super) fn snapshot_metadata(
    terminal: &Term<DaemonEventListener>,
    session_id: SessionId,
    size: TerminalSize,
    title: String,
    revision: u64,
    last_input_sequence: u64,
    cells: TerminalCells,
) -> TerminalSnapshot {
    let graphics = terminal.kitty_graphics_snapshot();
    let content = terminal.renderable_content();
    let display_offset = content.display_offset;
    let color_overrides = (0..COUNT)
        .filter_map(|index| {
            content.colors[index].map(|color| TerminalColorOverride {
                index: index as u16,
                color: rgba_from_rgb(color),
            })
        })
        .collect();
    let cursor_width = if terminal.grid()[content.cursor.point]
        .flags
        .contains(Flags::WIDE_CHAR)
    {
        2
    } else {
        1
    };
    let cursor_point = point_to_viewport(display_offset, content.cursor.point)
        .filter(|point| point.line < size.rows as usize);
    let selection = project_selection(terminal, display_offset, size);
    TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        session_id,
        size,
        cells,
        color_overrides,
        cursor_line: cursor_point.map_or(0, |point| point.line as u16),
        cursor_column: cursor_point.map_or(0, |point| point.column.0 as u16),
        cursor_shape: cursor_point.map_or(TerminalCursorShape::Hidden, |_| {
            snapshot_cursor_shape(content.cursor.shape)
        }),
        cursor_width,
        // The kernel tracks the blinking bit alongside the shape (set by DECSCUSR or DEC Mode 12);
        // it is only meaningful while the cursor is on-screen. The UI's blink setting decides
        // whether to actually honor this.
        cursor_blinking: cursor_point.is_some() && terminal.cursor_style().blinking,
        title,
        revision,
        last_input_sequence,
        input_modes: terminal_input_modes(*terminal.mode()),
        images: graphics
            .images
            .into_iter()
            .map(|image| TerminalImageDescriptor {
                key: TerminalImageKey {
                    id: image.key.id,
                    generation: image.key.generation,
                },
                width: image.width,
                height: image.height,
            })
            .collect(),
        image_placements: graphics
            .placements
            .into_iter()
            .map(|placement| TerminalImagePlacement {
                image: TerminalImageKey {
                    id: placement.image.id,
                    generation: placement.image.generation,
                },
                placement_id: placement.placement_id,
                line: placement.line,
                column: placement.column,
                source_x: placement.source_x,
                source_y: placement.source_y,
                source_width: placement.source_width,
                source_height: placement.source_height,
                x_offset: placement.x_offset,
                y_offset: placement.y_offset,
                columns: placement.columns,
                rows: placement.rows,
                destination_width: placement.destination_width,
                destination_height: placement.destination_height,
                z: placement.z,
            })
            .collect(),
        selection,
        // Filled in by `ListenerState::detect_urls_into` after this snapshot is built.
        detected_links: Vec::new(),
        display_offset: display_offset as u32,
        history_size: terminal.grid().history_size() as u32,
    }
}

pub(super) fn snapshot_delta(
    base: &TerminalSnapshot,
    current: &TerminalSnapshot,
) -> Option<TerminalSnapshotDelta> {
    if base.session_id != current.session_id
        || base.size != current.size
        || base.revision >= current.revision
    {
        return None;
    }
    let mut cells = Vec::new();
    let mut cleared = Vec::new();
    let mut base_iter = base.cells.iter().peekable();
    let mut current_iter = current.cells.iter().peekable();
    while base_iter.peek().is_some() || current_iter.peek().is_some() {
        let base_cell = base_iter.peek().copied();
        let current_cell = current_iter.peek().copied();
        let base_position = base_cell.map(|cell| (cell.line, cell.column));
        let current_position = current_cell.map(|cell| (cell.line, cell.column));
        match (base_position, current_position) {
            (Some(base_position), Some(current_position)) if base_position == current_position => {
                if base_cell != current_cell {
                    cells.push(current_cell.expect("current cell exists").to_owned());
                }
                base_iter.next();
                current_iter.next();
            }
            (Some(base_position), Some(current_position)) if base_position < current_position => {
                cleared.push(TerminalCellPosition {
                    line: base_position.0,
                    column: base_position.1,
                });
                base_iter.next();
            }
            (Some(_), Some(_)) => {
                cells.push(current_cell.expect("current cell exists").to_owned());
                current_iter.next();
            }
            (Some(base_position), None) => {
                cleared.push(TerminalCellPosition {
                    line: base_position.0,
                    column: base_position.1,
                });
                base_iter.next();
            }
            (None, Some(_)) => {
                cells.push(current_cell.expect("current cell exists").to_owned());
                current_iter.next();
            }
            (None, None) => break,
        }
    }

    if cells.len() + cleared.len() >= current.cells.len().max(1) {
        return None;
    }
    Some(TerminalSnapshotDelta {
        prompt_editable: current.prompt_editable,
        prompt_input: current.prompt_input.clone(),
        session_id: current.session_id,
        base_revision: base.revision,
        size: current.size,
        cells,
        cleared,
        color_overrides: current.color_overrides.clone(),
        cursor_line: current.cursor_line,
        cursor_column: current.cursor_column,
        cursor_shape: current.cursor_shape,
        cursor_width: current.cursor_width,
        cursor_blinking: current.cursor_blinking,
        title: current.title.clone(),
        revision: current.revision,
        last_input_sequence: current.last_input_sequence,
        input_modes: current.input_modes,
        images: (base.images != current.images).then(|| current.images.clone()),
        image_placements: (base.image_placements != current.image_placements)
            .then(|| current.image_placements.clone()),
        selection: current.selection,
        detected_links: current.detected_links.clone(),
        display_offset: current.display_offset,
        history_size: current.history_size,
    })
}

pub(super) fn snapshot_cell_is_empty(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\t')
        && cell.bg == Color::Named(NamedColor::Background)
        && cell.fg == Color::Named(NamedColor::Foreground)
        && !cell.flags.intersects(
            Flags::INVERSE
                | Flags::ALL_UNDERLINES
                | Flags::STRIKEOUT
                | Flags::WRAPLINE
                | Flags::WIDE_CHAR_SPACER
                | Flags::LEADING_WIDE_CHAR_SPACER,
        )
        && cell
            .zerowidth()
            .is_none_or(|zerowidth| zerowidth.is_empty())
}

/// Resize the terminal, protecting the active shell prompt/input region from a reflow that the
/// shell is about to redraw anyway.
///
/// Alacritty reflows the whole primary screen on a column change, tracking each logical line by
/// its `WRAPLINE` continuation markers. That is correct for completed command output, but the
/// active prompt/input region is a special case: after `SIGWINCH` the shell reprints its prompt
/// from scratch. Any copy of the prompt that resize leaves behind (or pushes into scrollback)
/// becomes a stale duplicate — the "duplicate prompt fragments" bug, most visible with multiline
/// prompts like Powerlevel10k where each resize sediments another copy into history.
///
/// When OSC 133 shell integration tells us the cursor is on a prompt/input region (`phase` is
/// [`TerminalSemanticPhase::Prompt`] or [`TerminalSemanticPhase::Input`]), we clear that region —
/// from the recorded prompt-start line down to the cursor — before resizing, blanking every cell
/// so no stale glyph or `WRAPLINE`/`WIDE_CHAR` continuation survives, and let the shell redraw it.
/// The cursor is left in place (like Ghostty's `clearCells`) so Alacritty's native resize tracks it
/// and the shell's redraw repaints from the same anchor. `prompt_start_line` comes from the cursor
/// position captured when the OSC 133 prompt-start marker arrived (mirroring Ghostty's approach),
/// so command output above the prompt is preserved.
///
/// In every other case (command output, no shell integration, or a row-only resize) we fall back
/// to Alacritty's native, reversible reflow — the same behavior Ghostty uses when it lacks
/// semantic information.
#[cfg(test)]
pub(super) fn resize_terminal_with_history_reflow(
    terminal: &mut Term<DaemonEventListener>,
    size: TerminalSize,
    phase: TerminalSemanticPhase,
    prompt_start_line: Option<i32>,
) {
    resize_terminal_with_anchors(terminal, size, phase, prompt_start_line, &mut []);
}

pub(super) fn resize_terminal_with_anchors(
    terminal: &mut Term<DaemonEventListener>,
    size: TerminalSize,
    phase: TerminalSemanticPhase,
    prompt_start_line: Option<i32>,
    anchors: &mut [Option<Point>],
) {
    let dimensions = GridSize(size);

    // Alternate-screen apps (vim, less, tmux, …) repaint themselves on resize, so neither
    // reflow nor prompt-clearing applies. Hand straight off to the native resize.
    if terminal.mode().contains(TermMode::ALT_SCREEN) {
        terminal.resize_with_primary_anchors(dimensions, anchors);
        return;
    }

    let columns_changed = terminal.columns() != dimensions.columns();
    let cursor_on_prompt = matches!(
        phase,
        TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
    );

    // A wrap-reflow only happens on a column change; a row-only resize never reflows, so there is
    // nothing to protect the prompt from.
    if columns_changed && cursor_on_prompt {
        let grid = terminal.grid_mut();
        let old_columns = grid.columns();
        let cursor_line = grid.cursor.point.line.0;

        // Clear from the recorded prompt start (captured when OSC 133 A arrived) down to the cursor.
        // If we somehow have no recorded start, fall back to the cursor row alone rather than
        // guessing a wider span — clearing too little only risks a small fragment, while clearing
        // too much would wipe command output above the prompt.
        //
        // Clamp the start into the visible screen: scrollback above the prompt is settled history
        // and must not be touched, and the cursor row is always the lower bound.
        let start = prompt_start_line
            .unwrap_or(cursor_line)
            .clamp(0, cursor_line.max(0));

        for line in start..=cursor_line.max(0) {
            for column in 0..old_columns {
                grid[Line(line)][Column(column)] = Cell::default();
            }
        }

        terminal.resize_with_primary_anchors(dimensions, anchors);
        return;
    }

    // Command output, no shell integration, or a row-only resize: native reflow is correct and
    // reversible.
    terminal.resize_with_primary_anchors(dimensions, anchors);
}

pub(super) fn snapshot_color(color: Color) -> eggie_protocol::TerminalColor {
    use eggie_protocol::TerminalColor;

    match color {
        Color::Spec(color) => TerminalColor::Rgb(rgba_from_rgb(color)),
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Named(named) => TerminalColor::Named(named as u16),
    }
}

pub(super) fn snapshot_cursor_shape(shape: CursorShape) -> TerminalCursorShape {
    match shape {
        CursorShape::Block => TerminalCursorShape::Block,
        CursorShape::Underline => TerminalCursorShape::Underline,
        CursorShape::Beam => TerminalCursorShape::Beam,
        CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
        CursorShape::Hidden => TerminalCursorShape::Hidden,
    }
}

/// Map a protocol cursor shape back to the terminal core's `CursorShape`. `Hidden` has no meaning as
/// a *default* shape (it would make the cursor permanently invisible), so it falls back to `Block`.
pub(super) fn kernel_cursor_shape(shape: TerminalCursorShape) -> CursorShape {
    match shape {
        TerminalCursorShape::Block => CursorShape::Block,
        TerminalCursorShape::Underline => CursorShape::Underline,
        TerminalCursorShape::Beam => CursorShape::Beam,
        TerminalCursorShape::HollowBlock => CursorShape::HollowBlock,
        TerminalCursorShape::Hidden => CursorShape::Block,
    }
}

/// Build the terminal core `Config` for a session with the given default cursor shape and scrollback
/// depth. Kept in one place so session creation and runtime `set_options` stay in sync on every other
/// field. Both runtime knobs are passed explicitly so a change to one never resets the other.
pub(super) fn terminal_config(
    default_cursor_shape: CursorShape,
    scrollback_limit: usize,
) -> Config {
    Config {
        scrolling_history: scrollback_limit,
        kitty_keyboard: true,
        // Permission is enforced by ListenerState so it can be changed at runtime without
        // rebuilding the terminal. Reads remain denied by default.
        osc52: Osc52::CopyPaste,
        default_cursor_style: CursorStyle {
            shape: default_cursor_shape,
            blinking: false,
        },
        ..Config::default()
    }
}
