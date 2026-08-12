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
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Copy,
        Cut,
        Confirm,
        ConfirmReverse,
        Cancel,
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

pub(crate) struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    style: TextInputStyle,
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
            is_selecting: false,
            style,
            language: Language::English,
            focused: false,
            blink_epoch: Instant::now(),
            blink_scheduled: false,
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
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
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
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
            // Single-line field: collapse any newlines to spaces.
            self.replace_text_in_range(None, &text.replace(['\r', '\n'], " "), window, cx);
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

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Confirm);
    }

    fn confirm_reverse(&mut self, _: &ConfirmReverse, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::ConfirmReverse);
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

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
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

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
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
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
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
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::confirm_reverse))
            .on_action(cx.listener(Self::cancel))
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
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("shift-enter", ConfirmReverse, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
    ]);
}

/// The custom element that shapes and paints the single line of text, caret, and selection.
struct TextElement {
    input: Entity<TextInput>,
}

struct TextElementPrepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    /// Baseline origin for the shaped line, vertically centered within the paint bounds. Painting
    /// the line here (instead of `bounds.origin`) keeps text centered no matter how tall the flex
    /// container stretches the element.
    text_origin: Point<Pixels>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
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
        // Fill the parent's height (so text can be centered within it), but never collapse below one
        // line when the parent doesn't constrain height.
        style.size.height = relative(1.).into();
        style.min_size.height = window.line_height().into();
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
            cursor: cursor_quad,
            selection,
            text_origin,
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
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().unwrap();
        line.paint(
            prepaint.text_origin,
            window.line_height(),
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
            input.last_bounds = Some(bounds);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{TextInput, edit_menu_enabled};

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
}
