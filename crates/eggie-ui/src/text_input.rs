//! A reusable, unstyled single-line text input built on GPUI's `EntityInputHandler`.
//!
//! The component owns its content, selection, cursor, and IME (marked text) state, and renders
//! only the text, caret, and selection — no border, background, or padding. Callers wrap it in
//! their own styled container and inject visual style via [`TextInputStyle`] plus the ambient
//! `text_size` / `text_color` of the surrounding element.
//!
//! It handles focus, mouse selection, keyboard navigation, and the standard editing shortcuts
//! (select-all / copy / cut / paste) itself. "Semantic" keys the component cannot interpret on its
//! own — Enter, Shift+Enter, Escape — are surfaced to the parent as [`TextInputEvent`]s so the
//! same component works for a search box, a filter field, and so on.
//!
//! The structure follows GPUI's official `examples/input.rs`, extended with a blinking caret, a
//! pluggable style, and an event channel.

use std::ops::Range;
use std::time::Instant;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, prelude::*, px,
    relative, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::native_menu::{NativeEditMenuCommand, prepare_edit_menu};
use crate::settings::Language;

actions!(
    eggie_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        Paste,
        Copy,
        Cut,
        Confirm,
        ConfirmReverse,
        Submit,
        Cancel,
        Undo,
        Redo,
    ]
);

/// Full on+off period of the caret blink, in seconds.
const CARET_BLINK_PERIOD: f32 = 1.06;

/// Colors for a [`TextInput`]. RGBA values are packed as `(rgb << 8) | alpha`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextInputStyle {
    pub(crate) text_color: u32,
    pub(crate) placeholder_color: u32,
    pub(crate) cursor_color: u32,
    pub(crate) selection_color: u32,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            text_color: 0xffffffff,
            placeholder_color: 0x888888ff,
            cursor_color: 0xffffffff,
            selection_color: 0x3311ff55,
        }
    }
}

/// Events emitted to the parent view that owns the [`TextInput`] entity.
#[derive(Clone, Debug)]
pub(crate) enum TextInputEvent {
    /// The content changed (typing, paste, cut, delete, or `set_content`).
    Changed,
    /// Enter pressed.
    Confirm,
    /// Shift+Enter pressed.
    ConfirmReverse,
    /// Escape pressed.
    Cancel,
}

/// One shaped line's geometry, cached by the element each paint for multi-line navigation and
/// hit-testing. `byte_start` is the line's absolute byte offset in `content` (the line text
/// excludes its trailing `\n`); `top` is the line's y offset from the content's top edge.
struct LineLayout {
    line: ShapedLine,
    byte_start: usize,
    byte_len: usize,
    top: Pixels,
}

/// A point-in-time snapshot of the editable state, used for undo/redo. Snapshot-based (not diff):
/// the content here is short (search terms, snippets, notes), so full copies are simplest and
/// always correct.
#[derive(Clone)]
struct EditSnapshot {
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

/// How an edit relates to the previous one, so consecutive same-kind edits coalesce into a single
/// undo step (typing a word is one undo, not one-per-keystroke).
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// Inserting text at the caret (typing).
    Insert,
    /// Deleting (backspace/delete).
    Delete,
    /// Anything else (paste, replace-selection, programmatic) — never coalesces.
    Other,
}

pub(crate) struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// Per-line shaped layout for multi-line mode, populated by the element each paint. Empty in
    /// single-line mode (which uses `last_layout`). Read by Up/Down navigation and multi-line
    /// hit-testing, which need pixel geometry the pure row/column model can't provide.
    line_layouts: Vec<LineLayout>,
    is_selecting: bool,
    style: TextInputStyle,
    /// Single-line (default) vs multi-line behavior. In multi-line mode `Enter` inserts a newline
    /// (instead of emitting [`TextInputEvent::Confirm`]), paste keeps newlines, and Up/Down move the
    /// caret across visual lines. Set via [`TextInput::multiline`] at construction.
    multiline: bool,
    /// Localized labels for the right-click edit menu. The parent pushes the current language each
    /// frame via [`set_language`], so switching language at runtime never leaves stale menu text.
    language: Language,
    /// Whether the field currently holds focus (maintained by focus_in/out observers). The caret
    /// is only drawn and only blinks while focused.
    focused: bool,
    /// Reset point for the blink phase; bumped on every edit so the caret stays solid while typing.
    blink_epoch: Instant,
    /// Whether a blink timer loop is currently running (avoids stacking loops).
    blink_scheduled: bool,
    /// Desired x-position (pixels, relative to line start) the caret aims for during vertical
    /// (Up/Down) movement, so walking across lines of differing lengths keeps a stable column.
    /// `None` resets it; any horizontal movement or edit clears it. Resolved against shaped lines
    /// in the element layer, which owns pixel geometry.
    vertical_goal_x: Option<Pixels>,
    /// Undo stack: snapshots taken *before* each committed edit. Redo stack: snapshots popped off
    /// undo. The kind of the last committed edit lets consecutive same-kind edits coalesce.
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    last_edit_kind: Option<EditKind>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>, style: TextInputStyle) -> Self {
        let focus_handle = cx.focus_handle();
        // Track our own focus so the caret is only drawn/blinks while focused, and so the blink
        // loop can be started on focus-in and can exit on focus-out.
        cx.on_focus_in(&focus_handle, window, |input, _window, cx| {
            input.focused = true;
            input.blink_epoch = Instant::now();
            input.ensure_blink(cx);
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&focus_handle, window, |input, _window, _event, cx| {
            input.focused = false;
            cx.notify();
        })
        .detach();
        Self {
            focus_handle,
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            line_layouts: Vec::new(),
            is_selecting: false,
            style,
            multiline: false,
            language: Language::English,
            focused: false,
            blink_epoch: Instant::now(),
            blink_scheduled: false,
            vertical_goal_x: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Enable multi-line editing. Builder-style so existing single-line call sites
    /// (`TextInput::new(...)`) are unaffected. In multi-line mode `Enter` inserts a newline, paste
    /// preserves newlines, and Up/Down navigate across visual lines.
    pub(crate) fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Whether this input is in multi-line mode.
    pub(crate) fn is_multiline(&self) -> bool {
        self.multiline
    }

    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub(crate) fn content(&self) -> &str {
        self.content.as_ref()
    }

    pub(crate) fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    /// Update the language used for the right-click edit menu labels. Cheap enough to call every
    /// frame from the parent's render; no `cx.notify()` because the menu reads `self.language`
    /// lazily when it pops up, so a language change never affects already-painted content.
    pub(crate) fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    /// Replace the content programmatically, clamping the selection and clearing any marked text.
    pub(crate) fn set_content(&mut self, content: impl Into<SharedString>, cx: &mut Context<Self>) {
        let content = content.into();
        if content == self.content {
            return;
        }
        self.content = content;
        let len = self.content.len();
        self.selected_range = len..len;
        self.selection_reversed = false;
        self.marked_range = None;
        // Programmatic reset (e.g. switching which snippet is being edited): the old undo history
        // belongs to the previous content and must not survive into the new one.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit_kind = None;
        self.vertical_goal_x = None;
        self.blink_epoch = Instant::now();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    // --- Cursor / selection movement -------------------------------------------------------

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.do_select_all(cx);
    }

    fn do_select_all(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.move_to(self.line_bounds_at(self.cursor_offset()).0, cx);
        } else {
            self.move_to(0, cx);
        }
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.move_to(self.line_bounds_at(self.cursor_offset()).1, cx);
        } else {
            self.move_to(self.content.len(), cx);
        }
    }

    /// The `[start, end]` byte offsets of the line containing `offset` (line text excludes its
    /// trailing `\n`). Used by multi-line Home/End.
    fn line_bounds_at(&self, offset: usize) -> (usize, usize) {
        let starts = Self::line_starts(&self.content);
        let (row, _) = Self::offset_to_line_col(&self.content, offset);
        let range = Self::line_range(&self.content, &starts, row);
        (range.start, range.end)
    }

    // --- Vertical (Up/Down) caret movement -------------------------------------------------
    //
    // Only meaningful in multi-line mode, and only once the element has cached `line_layouts` (it
    // populates them each paint). Before the first paint, or in single-line mode, these are no-ops.

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(self.cursor_offset(), -1) {
            self.move_to_keep_goal(offset, cx);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(self.cursor_offset(), 1) {
            self.move_to_keep_goal(offset, cx);
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(self.cursor_offset(), -1) {
            self.select_to_keep_goal(offset, cx);
        }
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(self.cursor_offset(), 1) {
            self.select_to_keep_goal(offset, cx);
        }
    }

    /// Resolve the byte offset one visual line above (`dir = -1`) or below (`dir = 1`) `offset`,
    /// preserving the desired x-column across lines of differing length. Returns `None` when there
    /// is no such line, or geometry isn't available yet (single-line / pre-first-paint).
    fn vertical_target(&mut self, offset: usize, dir: i32) -> Option<usize> {
        if !self.multiline || self.line_layouts.is_empty() {
            return None;
        }
        let cur_idx = self.line_layout_index_for_offset(offset)?;
        let target_idx = cur_idx as i32 + dir;
        if target_idx < 0 || target_idx as usize >= self.line_layouts.len() {
            return None;
        }
        let target_idx = target_idx as usize;

        // Desired x: the remembered goal, else the caret's current x on its line.
        let current = &self.line_layouts[cur_idx];
        let goal_x = self.vertical_goal_x.unwrap_or_else(|| {
            current
                .line
                .x_for_index(offset.saturating_sub(current.byte_start))
        });
        self.vertical_goal_x = Some(goal_x);

        let target = &self.line_layouts[target_idx];
        let col = target.line.closest_index_for_x(goal_x);
        Some(target.byte_start + col.min(target.byte_len))
    }

    /// Index into `line_layouts` of the line containing `offset` (the last line whose `byte_start`
    /// is ≤ `offset`).
    fn line_layout_index_for_offset(&self, offset: usize) -> Option<usize> {
        if self.line_layouts.is_empty() {
            return None;
        }
        let idx = self
            .line_layouts
            .iter()
            .rposition(|layout| layout.byte_start <= offset)
            .unwrap_or(0);
        Some(idx)
    }

    /// Like [`move_to`] but preserves `vertical_goal_x` (set by [`vertical_target`]) so consecutive
    /// Up/Down keeps aiming at the same column.
    fn move_to_keep_goal(&mut self, offset: usize, cx: &mut Context<Self>) {
        let goal = self.vertical_goal_x;
        self.move_to(offset, cx);
        self.vertical_goal_x = goal;
    }

    fn select_to_keep_goal(&mut self, offset: usize, cx: &mut Context<Self>) {
        let goal = self.vertical_goal_x;
        self.select_to(offset, cx);
        self.vertical_goal_x = goal;
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.do_paste(window, cx);
    }

    fn do_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if self.multiline {
                // Multi-line: keep newlines (normalize CRLF/CR to LF so row math stays consistent).
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                self.replace_text_in_range(None, &normalized, window, cx);
            } else {
                // Single-line field: collapse any newlines to spaces.
                self.replace_text_in_range(None, &text.replace(['\r', '\n'], " "), window, cx);
            }
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.do_copy(cx);
    }

    fn do_copy(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.do_cut(window, cx);
    }

    fn do_cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    // --- Semantic keys forwarded to the parent ---------------------------------------------

    /// `Enter`. Single-line: emit [`TextInputEvent::Confirm`] for the parent to act on. Multi-line:
    /// insert a newline (submission there is `Cmd/Ctrl+Enter` → [`Self::submit`]).
    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(TextInputEvent::Confirm);
        }
    }

    /// `Shift+Enter`. Single-line: emit [`TextInputEvent::ConfirmReverse`] (e.g. search-backwards).
    /// Multi-line: insert a newline, same as `Enter`.
    fn confirm_reverse(&mut self, _: &ConfirmReverse, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            cx.emit(TextInputEvent::ConfirmReverse);
        }
    }

    /// `Cmd/Ctrl+Enter`. The multi-line submit gesture: always emit [`TextInputEvent::Confirm`] so a
    /// parent (e.g. the snippet editor) can save. In single-line mode `Enter` already confirms, so
    /// this is a harmless alias.
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Confirm);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Cancel);
    }

    // --- Mouse -----------------------------------------------------------------------------

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Grab focus ourselves and stop the event here. Otherwise it bubbles to an ancestor (e.g. the
        // terminal content region) whose own mouse-down handler would steal focus straight back,
        // leaving keystrokes to fall through into the terminal.
        window.focus(&self.focus_handle, cx);
        cx.stop_propagation();
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    /// Right-click: pop up the native macOS edit menu (Cut / Copy / Paste / Select All).
    fn on_right_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same reasoning as the left handler: grab focus and stop the event here so it doesn't bubble
        // to the terminal content region (which would steal focus and start a terminal selection).
        window.focus(&self.focus_handle, cx);
        cx.stop_propagation();

        let has_selection = !self.selected_range.is_empty();
        let has_content = !self.content.is_empty();
        let can_paste = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some();
        let enabled = edit_menu_enabled(has_selection, has_content, can_paste);

        let Some(menu) = prepare_edit_menu(window, enabled, self.language) else {
            return; // Non-macOS, or the AppKit view / current event was unavailable.
        };

        // The AppKit menu runs a modal nested event loop, so it must run in a foreground future
        // rather than inside this listener (which still holds the App borrow). `spawn_in` yields an
        // `AsyncWindowContext`, so `update_in` can hand us back a `&mut Window` to run the action —
        // required because `replace_text_in_range` (paste/cut) takes a `Window`.
        cx.spawn_in(window, async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            this.update_in(cx, |input, window, cx| {
                input.perform_edit_command(command, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn perform_edit_command(
        &mut self,
        command: NativeEditMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            NativeEditMenuCommand::Cut => self.do_cut(window, cx),
            NativeEditMenuCommand::Copy => self.do_copy(cx),
            NativeEditMenuCommand::Paste => self.do_paste(window, cx),
            NativeEditMenuCommand::SelectAll => self.do_select_all(cx),
        }
    }

    // --- Selection helpers -----------------------------------------------------------------

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.vertical_goal_x = None;
        // Moving the caret breaks an edit run: the next edit starts a fresh undo step even if it's
        // the same kind (typing, moving, then typing again shouldn't coalesce across the gap).
        self.last_edit_kind = None;
        self.blink_epoch = Instant::now();
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    // --- Undo / redo -----------------------------------------------------------------------

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    /// Record the pre-edit state onto the undo stack before an edit of the given `kind`. Consecutive
    /// edits of the same coalescing kind (Insert after Insert, Delete after Delete) don't push a new
    /// snapshot, so typing or deleting a run collapses into one undo step. Any edit clears the redo
    /// stack (a new branch of history). Call this *before* mutating `content`.
    fn record_history(&mut self, kind: EditKind) {
        let coalesce = Self::should_coalesce(kind, self.last_edit_kind);
        if !coalesce {
            self.undo_stack.push(self.snapshot());
            // Cap the stack so a long editing session can't grow it without bound.
            const MAX_UNDO: usize = 500;
            if self.undo_stack.len() > MAX_UNDO {
                self.undo_stack.remove(0);
            }
        }
        self.redo_stack.clear();
        self.last_edit_kind = Some(kind);
    }

    /// Whether an edit of `kind` should merge into the previous undo step (of `last`) rather than
    /// start a new one. Only same-kind Insert/Delete runs coalesce; `Other` (paste, replace) never
    /// does. Pure so the coalescing rule is unit-tested without a GPUI context.
    fn should_coalesce(kind: EditKind, last: Option<EditKind>) -> bool {
        kind != EditKind::Other && last == Some(kind)
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.restore(prev);
            self.last_edit_kind = None; // Force the next edit to start a fresh undo step.
            cx.emit(TextInputEvent::Changed);
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.restore(next);
            self.last_edit_kind = None;
            cx.emit(TextInputEvent::Changed);
            cx.notify();
        }
    }

    fn restore(&mut self, snapshot: EditSnapshot) {
        self.content = snapshot.content;
        // Clamp in case a snapshot's offsets don't align (defensive; they always should).
        self.selected_range = self.clamp_range(snapshot.selected_range);
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.vertical_goal_x = None;
        self.blink_epoch = Instant::now();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };

        if self.multiline && !self.line_layouts.is_empty() {
            // Pick the line whose vertical band contains the pointer; clamp above/below to the
            // first/last line. `top` is relative to the content's top edge, which is `bounds.top()`
            // shifted by the scroll offset the element applied when it built the layouts.
            let local_y = position.y - bounds.top();
            let row = self
                .line_layouts
                .iter()
                .rposition(|layout| layout.top <= local_y)
                .unwrap_or(0);
            let layout = &self.line_layouts[row];
            let col = layout.line.closest_index_for_x(position.x - bounds.left());
            return layout.byte_start + col.min(layout.byte_len);
        }

        let Some(line) = self.last_layout.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.vertical_goal_x = None;
        self.blink_epoch = Instant::now();
        cx.notify();
    }

    // --- UTF-16 <-> UTF-8 offset conversion (required by the IME contract) -----------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    /// Snap a byte offset into `content`, clamping to its length and down to the nearest char
    /// boundary. IME callbacks can hand us offsets derived from a stale `content`, so every offset
    /// that will index into `content` must pass through here first to avoid slicing panics.
    fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn clamp_range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.clamp_offset(range.start);
        let end = self.clamp_offset(range.end.max(range.start));
        start..end
    }

    /// Byte length of the prefix of `text` spanning its first `utf16_units` UTF-16 code units.
    /// Used to map an IME selection offset (relative to the marked text, in UTF-16) into a UTF-8
    /// offset within that same text.
    fn utf8_len_for_utf16_prefix(text: &str, utf16_units: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in text.chars() {
            if utf16 >= utf16_units {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    // --- Multi-line row/column model -------------------------------------------------------
    //
    // Lines are split on '\n'. A trailing '\n' yields a final empty line (so the caret can sit on a
    // blank last line). Columns are byte offsets *within* a line, so `line_col_to_offset` composes
    // exactly with `offset_to_line_col`. These are pure associative functions (no `&self`, no GPUI)
    // so the row/column math is unit-tested in isolation from shaping and painting.

    /// Byte offset where each line begins. Always starts with `0`; there are
    /// `1 + count('\n')` entries. For `"ab\ncd"` → `[0, 3]`; for `"a\n"` → `[0, 2]`.
    fn line_starts(text: &str) -> Vec<usize> {
        let mut starts = vec![0];
        for (idx, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(idx + 1);
            }
        }
        starts
    }

    /// The byte range of line `row` *excluding* its trailing newline. `row` is clamped to the last
    /// line. For `"ab\ncd"`: row 0 → `0..2`, row 1 → `3..5`.
    fn line_range(text: &str, starts: &[usize], row: usize) -> Range<usize> {
        let row = row.min(starts.len().saturating_sub(1));
        let start = starts[row];
        // End is the next line's start minus the '\n', or the text end for the last line.
        let end = if row + 1 < starts.len() {
            starts[row + 1] - 1
        } else {
            text.len()
        };
        start..end
    }

    /// Map an absolute byte offset to `(row, col)` where `col` is the byte offset within the line.
    /// An offset landing exactly on a line's end (before its `\n`) stays on that line.
    fn offset_to_line_col(text: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(text.len());
        let starts = Self::line_starts(text);
        // Find the last line whose start is <= offset.
        let row = match starts.binary_search(&offset) {
            Ok(idx) => idx,
            Err(idx) => idx - 1, // idx is the first start > offset, so the line before it.
        };
        (row, offset - starts[row])
    }

    /// Inverse of [`offset_to_line_col`]. `row`/`col` are clamped into range: `row` to the last
    /// line, `col` to that line's length. Returns an absolute byte offset.
    fn line_col_to_offset(text: &str, row: usize, col: usize) -> usize {
        let starts = Self::line_starts(text);
        let range = Self::line_range(text, &starts, row);
        (range.start + col).min(range.end)
    }

    /// Number of lines (≥ 1).
    fn line_count(text: &str) -> usize {
        Self::line_starts(text).len()
    }

    // --- Caret blink -----------------------------------------------------------------------

    /// Whether the caret should be painted this frame: only while focused and on the "on" half of
    /// the blink cycle.
    fn caret_visible(&self) -> bool {
        if !self.focused {
            return false;
        }
        let elapsed = self.blink_epoch.elapsed().as_secs_f32();
        elapsed.rem_euclid(CARET_BLINK_PERIOD) < CARET_BLINK_PERIOD / 2.
    }

    /// Drive the caret blink while focused. Uses a background timer (not a frame callback) so it
    /// can be started from a focus observer without a `Window`, and exits as soon as focus is lost.
    fn ensure_blink(&mut self, cx: &mut Context<Self>) {
        if self.blink_scheduled || !self.focused {
            return;
        }
        self.blink_scheduled = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor
                    .timer(std::time::Duration::from_secs_f32(CARET_BLINK_PERIOD / 2.))
                    .await;
                let keep_going = this
                    .update(cx, |input, cx| {
                        if input.focused {
                            cx.notify();
                        } else {
                            input.blink_scheduled = false;
                        }
                        input.focused
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Compute the enabled flags for the edit menu, in native_menu tag order: `[cut, copy, paste,
/// select_all]`. Cut/copy need a non-empty selection, paste needs clipboard text, select-all needs
/// any content. Kept as a free function so it can be unit-tested without a GPUI context.
fn edit_menu_enabled(has_selection: bool, has_content: bool, can_paste: bool) -> [bool; 4] {
    [has_selection, has_selection, can_paste, has_content]
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.clamp_range(self.range_from_utf16(&range_utf16));
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // The range may be derived from a stale marked/selected range; clamp before slicing.
        let range = self.clamp_range(range);

        // Record undo history before mutating. Classify the edit so runs of the same kind coalesce
        // into one undo step. A committed edit that ends an IME composition counts as Insert.
        let kind = if range.is_empty() && !new_text.is_empty() {
            EditKind::Insert
        } else if !range.is_empty() && new_text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Other
        };
        self.record_history(kind);

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.vertical_goal_x = None;
        self.blink_epoch = Instant::now();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        // The range may be derived from a stale marked/selected range; clamp before slicing.
        let range = self.clamp_range(range);

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        // `new_selected_range_utf16` is relative to the just-inserted `new_text`, so map it inside
        // `new_text` and offset by `range.start`. (The upstream example added it to `range.end`,
        // which — under multi-keystroke IME composition — produces an out-of-bounds `selected_range`
        // that later panics when used to slice `content`.)
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|sel_utf16| {
                let start =
                    range.start + Self::utf8_len_for_utf16_prefix(new_text, sel_utf16.start);
                let end = range.start + Self::utf8_len_for_utf16_prefix(new_text, sel_utf16.end);
                start..end
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.vertical_goal_x = None;
        self.blink_epoch = Instant::now();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);

        if self.multiline && !self.line_layouts.is_empty() {
            // Anchor the IME candidate window at the range start's line. Locate the line, then take
            // the x within it. `top` is relative to the content top edge (`bounds.top()`).
            let row = self
                .line_layouts
                .iter()
                .rposition(|layout| layout.byte_start <= range.start)
                .unwrap_or(0);
            let layout = &self.line_layouts[row];
            let line_height = if self.line_layouts.len() > row + 1 {
                self.line_layouts[row + 1].top - layout.top
            } else {
                bounds.bottom() - (bounds.top() + layout.top)
            };
            let start_x = layout.line.x_for_index(range.start - layout.byte_start);
            let end_col = range.end.saturating_sub(layout.byte_start).min(layout.byte_len);
            let end_x = layout.line.x_for_index(end_col);
            let line_top = bounds.top() + layout.top;
            return Some(Bounds::from_corners(
                point(bounds.left() + start_x, line_top),
                point(bounds.left() + end_x, line_top + line_height),
            ));
        }

        let last_layout = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.multiline && !self.line_layouts.is_empty() {
            let utf8_index = self.index_for_mouse_position(point);
            return Some(self.offset_to_utf16(utf8_index));
        }
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .key_context("EggieTextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::confirm_reverse))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement {
                input: cx.entity(),
            })
    }
}

/// Register the input's keybindings. Must be called AFTER any global (context-less) bindings that
/// share these chords (e.g. the terminal's `cmd-c`), so the `EggieTextInput`-context bindings win
/// the binding-index tiebreak when the field is focused.
pub(crate) fn install_keybindings(cx: &mut App) {
    let ctx = Some("EggieTextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("shift-enter", ConfirmReverse, ctx),
        // Multi-line submit gesture; harmless alias of Enter in single-line mode.
        KeyBinding::new("cmd-enter", Submit, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("cmd-z", Undo, ctx),
        KeyBinding::new("cmd-shift-z", Redo, ctx),
    ]);
}

/// The custom element that shapes and paints text, caret, and selection — one line in single-line
/// mode, or a stack of lines in multi-line mode.
struct TextElement {
    input: Entity<TextInput>,
}

/// A single shaped line positioned within the multi-line stack, carried from prepaint to paint and
/// then cached back onto the input for navigation/hit-testing.
struct RenderedLine {
    line: ShapedLine,
    byte_start: usize,
    byte_len: usize,
    /// y offset from the content's top edge.
    top: Pixels,
}

struct TextElementPrepaint {
    /// Single-line shaped text (single-line mode only).
    line: Option<ShapedLine>,
    /// Per-line shaped text (multi-line mode only).
    lines: Vec<RenderedLine>,
    cursor: Option<PaintQuad>,
    /// Selection may span multiple lines, so it's a list of rectangles (one per covered line).
    selections: Vec<PaintQuad>,
    /// Baseline origin for the shaped line, vertically centered within the paint bounds. Painting
    /// the line here (instead of `bounds.origin`) keeps text centered no matter how tall the flex
    /// container stretches the element. Multi-line uses each line's own top instead.
    text_origin: Point<Pixels>,
    /// Line height used to position multi-line rows and size the caret.
    line_height: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextElement {
    /// Multi-line prepaint: shape each line independently, stack them top-to-bottom, and compute a
    /// caret plus one selection rectangle per covered line. `top` values are relative to the paint
    /// bounds' top edge (which the parent scroll container translates for overflow).
    fn prepaint_multiline(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> TextElementPrepaint {
        let input = self.input.read(cx);
        let style = input.style;
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked_range = input.marked_range.clone();
        let caret_visible = input.caret_visible();
        let is_empty = content.is_empty();
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        // Empty content: show the placeholder on line 0 and a caret at the start.
        if is_empty {
            let placeholder = input.placeholder.clone();
            let run = TextRun {
                len: placeholder.len(),
                font: font.clone(),
                color: rgba(style.placeholder_color).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs: Vec<TextRun> = if placeholder.is_empty() {
                Vec::new()
            } else {
                vec![run]
            };
            let line = window
                .text_system()
                .shape_line(placeholder, font_size, &runs, None);
            let cursor_quad = caret_visible.then(|| {
                fill(
                    Bounds::new(
                        point(bounds.left(), bounds.top()),
                        size(px(1.5), line_height),
                    ),
                    rgba(style.cursor_color),
                )
            });
            let lines = vec![RenderedLine {
                line,
                byte_start: 0,
                byte_len: 0,
                top: px(0.),
            }];
            return TextElementPrepaint {
                line: None,
                lines,
                cursor: cursor_quad,
                selections: Vec::new(),
                text_origin: point(bounds.left(), bounds.top()),
                line_height,
            };
        }

        let text_color = rgba(style.text_color);
        let starts = TextInput::line_starts(&content);
        let mut lines: Vec<RenderedLine> = Vec::with_capacity(starts.len());
        let mut selections: Vec<PaintQuad> = Vec::new();
        let mut cursor_quad: Option<PaintQuad> = None;

        for (row, &line_start) in starts.iter().enumerate() {
            let line_end = if row + 1 < starts.len() {
                starts[row + 1] - 1 // exclude the '\n'
            } else {
                content.len()
            };
            let line_text: SharedString = content[line_start..line_end].to_string().into();
            let byte_len = line_end - line_start;
            let top = line_height * (row as f32);

            // Base run for the whole line, splitting out any marked (IME) sub-range for underline.
            let base = TextRun {
                len: byte_len,
                font: font.clone(),
                color: text_color.into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs: Vec<TextRun> = match marked_range.as_ref().and_then(|m| {
                // Intersection of the marked range with this line, in line-local offsets.
                let s = m.start.max(line_start);
                let e = m.end.min(line_end);
                (s < e).then(|| (s - line_start, e - line_start))
            }) {
                Some((ms, me)) => [
                    TextRun { len: ms, ..base.clone() },
                    TextRun {
                        len: me - ms,
                        underline: Some(UnderlineStyle {
                            color: Some(base.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..base.clone()
                    },
                    TextRun { len: byte_len - me, ..base.clone() },
                ]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect(),
                None => {
                    if byte_len > 0 {
                        vec![base]
                    } else {
                        Vec::new()
                    }
                }
            };
            let line = window
                .text_system()
                .shape_line(line_text, font_size, &runs, None);

            // Selection rectangle for the part of this line inside the selection range.
            if !selected_range.is_empty() {
                let s = selected_range.start.max(line_start);
                // A selection that spans past this line's end (into the newline) should visually
                // extend to the line end; cap the highlight at byte_len.
                let e = selected_range.end.min(line_end);
                if s <= e && selected_range.start <= line_end && selected_range.end >= line_start {
                    let x0 = line.x_for_index(s - line_start);
                    let x1 = line.x_for_index(e - line_start);
                    selections.push(fill(
                        Bounds::from_corners(
                            point(bounds.left() + x0, bounds.top() + top),
                            point(bounds.left() + x1, bounds.top() + top + line_height),
                        ),
                        rgba(style.selection_color),
                    ));
                }
            }

            // Caret on the line that contains it. The cursor sits at a line's end offset on that
            // line (not the next line's start), except the very last line owns content.len().
            let on_this_line = if row + 1 < starts.len() {
                cursor >= line_start && cursor < starts[row + 1]
            } else {
                cursor >= line_start && cursor <= line_end
            };
            if caret_visible && cursor_quad.is_none() && on_this_line {
                let cx_x = line.x_for_index(cursor.saturating_sub(line_start).min(byte_len));
                cursor_quad = Some(fill(
                    Bounds::new(
                        point(bounds.left() + cx_x, bounds.top() + top),
                        size(px(1.5), line_height),
                    ),
                    rgba(style.cursor_color),
                ));
            }

            lines.push(RenderedLine {
                line,
                byte_start: line_start,
                byte_len,
                top,
            });
        }

        TextElementPrepaint {
            line: None,
            lines,
            cursor: cursor_quad,
            selections,
            text_origin: point(bounds.left(), bounds.top()),
            line_height,
        }
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = TextElementPrepaint;

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
        let line_height = window.line_height();

        if self.input.read(cx).multiline {
            // Multi-line: grow to fit all lines so the parent's scroll container can overflow. Don't
            // stretch to the parent height (that would vertically center a single line).
            let lines = TextInput::line_count(&self.input.read(cx).content).max(1);
            let content_height = line_height * (lines as f32);
            style.size.height = content_height.into();
            style.min_size.height = content_height.into();
        } else {
            // Single-line: fill the parent's height (so text can be centered within it), but never
            // collapse below one line when the parent doesn't constrain height.
            style.size.height = relative(1.).into();
            style.min_size.height = line_height.into();
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = input.style;
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked_range = input.marked_range.clone();
        let text_style = window.text_style();

        if input.multiline {
            return self.prepaint_multiline(bounds, window, cx);
        }

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), rgba(style.placeholder_color))
        } else {
            (content, rgba(style.text_color))
        };

        let run = TextRun {
            len: display_text.len(),
            font: text_style.font(),
            color: text_color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        // Vertically center a single line within the paint bounds. The flex parent may stretch this
        // element taller than one line, so anchoring to `bounds.top()` would push text to the top.
        let line_height = window.line_height();
        let vertical_offset = ((bounds.size.height - line_height) / 2.).max(px(0.));
        let content_top = bounds.top() + vertical_offset;
        let content_bottom = content_top + line_height;
        let text_origin = point(bounds.left(), content_top);

        // Caret and selection are computed against the shaped line. When there is a placeholder
        // (empty content) the caret sits at x=0 (start).
        let cursor_x = if input.content.is_empty() {
            px(0.)
        } else {
            line.x_for_index(cursor)
        };
        let caret_visible = input.caret_visible();

        let (selection, cursor_quad) = if selected_range.is_empty() || input.content.is_empty() {
            let cursor = caret_visible.then(|| {
                fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, content_top),
                        size(px(1.5), content_bottom - content_top),
                    ),
                    rgba(style.cursor_color),
                )
            });
            (None, cursor)
        } else {
            let selection = fill(
                Bounds::from_corners(
                    point(
                        bounds.left() + line.x_for_index(selected_range.start),
                        content_top,
                    ),
                    point(
                        bounds.left() + line.x_for_index(selected_range.end),
                        content_bottom,
                    ),
                ),
                rgba(style.selection_color),
            );
            let cursor = caret_visible.then(|| {
                fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, content_top),
                        size(px(1.5), content_bottom - content_top),
                    ),
                    rgba(style.cursor_color),
                )
            });
            (Some(selection), cursor)
        };

        TextElementPrepaint {
            line: Some(line),
            lines: Vec::new(),
            cursor: cursor_quad,
            selections: selection.into_iter().collect(),
            text_origin,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        // Selection (one rect in single-line mode, one per covered line in multi-line mode).
        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }

        let line_height = prepaint.line_height;

        if !prepaint.lines.is_empty() {
            // Multi-line: paint each line at its own top, then cache the layout for navigation.
            let rendered = std::mem::take(&mut prepaint.lines);
            for rendered_line in &rendered {
                rendered_line
                    .line
                    .paint(
                        point(bounds.left(), bounds.top() + rendered_line.top),
                        line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            self.input.update(cx, |input, _cx| {
                input.line_layouts = rendered
                    .into_iter()
                    .map(|r| LineLayout {
                        line: r.line,
                        byte_start: r.byte_start,
                        byte_len: r.byte_len,
                        top: r.top,
                    })
                    .collect();
                input.last_layout = None;
                input.last_bounds = Some(bounds);
            });
            return;
        }

        // Single-line.
        let line = prepaint.line.take().unwrap();
        line.paint(
            prepaint.text_origin,
            line_height,
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();

        if let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.line_layouts = Vec::new();
            input.last_bounds = Some(bounds);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{EditKind, TextInput, edit_menu_enabled};

    #[test]
    fn edit_menu_enabled_maps_state_to_flags() {
        // [cut, copy, paste, select_all]
        // 无选区 + 空内容 + 剪贴板空 → 全灰。
        assert_eq!(
            edit_menu_enabled(false, false, false),
            [false, false, false, false]
        );
        // 有选区(可剪切/拷贝)+ 有内容(可全选)+ 剪贴板有内容(可粘贴)→ 全亮。
        assert_eq!(edit_menu_enabled(true, true, true), [true, true, true, true]);
        // 有内容但无选区:cut/copy 灰,select_all 亮。
        assert_eq!(
            edit_menu_enabled(false, true, false),
            [false, false, false, true]
        );
        // 剪贴板有内容但输入框空:仅 paste 亮。
        assert_eq!(
            edit_menu_enabled(false, false, true),
            [false, false, true, false]
        );
    }

    // Maps a UTF-16 selection offset (relative to the inserted marked text) to a UTF-8 byte length.
    // This is the mapping whose absence caused an out-of-bounds `selected_range` and the IME panic.
    #[test]
    fn utf8_len_for_utf16_prefix_maps_offsets_across_multibyte_text() {
        // ASCII: 1 byte == 1 UTF-16 unit.
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("ni", 0), 0);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("ni", 1), 1);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("ni", 2), 2);

        // CJK: each char is 3 UTF-8 bytes / 1 UTF-16 unit.
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("你好", 0), 0);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("你好", 1), 3);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("你好", 2), 6);

        // Astral (emoji): 4 UTF-8 bytes / 2 UTF-16 units (surrogate pair).
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("😀", 0), 0);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("😀", 2), 4);

        // Over-long offsets saturate at the text length instead of panicking.
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("ni", 99), 2);
        assert_eq!(TextInput::utf8_len_for_utf16_prefix("", 3), 0);
    }

    #[test]
    fn line_starts_marks_each_line_beginning() {
        assert_eq!(TextInput::line_starts(""), vec![0]);
        assert_eq!(TextInput::line_starts("abc"), vec![0]);
        assert_eq!(TextInput::line_starts("ab\ncd"), vec![0, 3]);
        // Trailing newline yields a final empty line whose start is text.len().
        assert_eq!(TextInput::line_starts("a\n"), vec![0, 2]);
        assert_eq!(TextInput::line_starts("\n\n"), vec![0, 1, 2]);
        assert_eq!(TextInput::line_count("a\nb\nc"), 3);
        assert_eq!(TextInput::line_count(""), 1);
    }

    #[test]
    fn offset_and_line_col_round_trip() {
        // "ab\ncd": offsets 0,1,2 on row 0 (2 is line end, before '\n'); 3,4,5 on row 1.
        let text = "ab\ncd";
        assert_eq!(TextInput::offset_to_line_col(text, 0), (0, 0));
        assert_eq!(TextInput::offset_to_line_col(text, 2), (0, 2)); // end of line 0
        assert_eq!(TextInput::offset_to_line_col(text, 3), (1, 0)); // start of line 1
        assert_eq!(TextInput::offset_to_line_col(text, 5), (1, 2)); // end of text

        // Inverse composes exactly.
        for offset in 0..=text.len() {
            let (row, col) = TextInput::offset_to_line_col(text, offset);
            assert_eq!(TextInput::line_col_to_offset(text, row, col), offset);
        }
    }

    #[test]
    fn line_col_handles_cjk_and_emoji_columns() {
        // Columns are byte offsets, so multibyte chars advance col by their UTF-8 width.
        let text = "你好\n😀x";
        // Row 0: 你(3) 好(3) → end col = 6.
        assert_eq!(TextInput::offset_to_line_col(text, 6), (0, 6));
        // Row 1 starts after "你好\n" = byte 7.
        assert_eq!(TextInput::offset_to_line_col(text, 7), (1, 0));
        // 😀 is 4 bytes, then x at col 4.
        assert_eq!(TextInput::offset_to_line_col(text, 11), (1, 4));
        assert_eq!(TextInput::line_col_to_offset(text, 1, 4), 11);
    }

    #[test]
    fn trailing_newline_yields_navigable_empty_last_line() {
        let text = "a\n";
        assert_eq!(TextInput::line_count(text), 2);
        // The empty last line begins at the text end and has zero width.
        assert_eq!(TextInput::offset_to_line_col(text, 2), (1, 0));
        assert_eq!(TextInput::line_col_to_offset(text, 1, 0), 2);
        // Over-long col clamps to the (empty) line's end.
        assert_eq!(TextInput::line_col_to_offset(text, 1, 99), 2);
    }

    #[test]
    fn line_col_clamps_out_of_range_row_and_col() {
        let text = "ab\ncd";
        // Row past the end clamps to the last line.
        assert_eq!(TextInput::line_col_to_offset(text, 99, 0), 3);
        // Col past a line's content clamps to its end (before the '\n').
        assert_eq!(TextInput::line_col_to_offset(text, 0, 99), 2);
        // Offset past the end clamps to text length.
        assert_eq!(TextInput::offset_to_line_col(text, 999), (1, 2));
    }

    #[test]
    fn undo_coalescing_merges_same_kind_runs_only() {
        // Same-kind runs coalesce (typing a word = one undo step).
        assert!(TextInput::should_coalesce(
            EditKind::Insert,
            Some(EditKind::Insert)
        ));
        assert!(TextInput::should_coalesce(
            EditKind::Delete,
            Some(EditKind::Delete)
        ));
        // Different kinds never coalesce (type then delete = two steps).
        assert!(!TextInput::should_coalesce(
            EditKind::Insert,
            Some(EditKind::Delete)
        ));
        assert!(!TextInput::should_coalesce(
            EditKind::Delete,
            Some(EditKind::Insert)
        ));
        // `Other` (paste, replace-selection) never coalesces, in either direction.
        assert!(!TextInput::should_coalesce(
            EditKind::Other,
            Some(EditKind::Other)
        ));
        assert!(!TextInput::should_coalesce(
            EditKind::Insert,
            Some(EditKind::Other)
        ));
        // No prior edit (e.g. right after a caret move / undo) always starts a fresh step.
        assert!(!TextInput::should_coalesce(EditKind::Insert, None));
        assert!(!TextInput::should_coalesce(EditKind::Delete, None));
    }
}
