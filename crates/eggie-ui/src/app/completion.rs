use super::*;
use eggie_completion::{
    CompletionContext, CompletionItem, CompletionKind, PromptInput, rank_indices,
};
use gpui::{ScrollStrategy, UniformListScrollHandle, uniform_list};

const LOCAL_DELAY: Duration = Duration::from_millis(16);
const NATIVE_DELAY: Duration = Duration::from_millis(120);
const NATIVE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct CompletionUi {
    generation: u64,
    task: Option<gpui::Task<()>>,
    native_task: Option<gpui::Task<()>>,
    pub(super) panel: Option<CompletionPanel>,
    pub(super) wants: bool,
    explicit: bool,
    scroll: UniformListScrollHandle,
    last_input: Option<(SessionId, PromptInput)>,
    native_pending: bool,
    local_empty: bool,
}
pub(super) struct CompletionPanel {
    pub(super) session: SessionId,
    input: PromptInput,
    source_input: PromptInput,
    candidates: Vec<CompletionItem>,
    matches: Vec<usize>,
    selected: usize,
    navigated: bool,
    anchor: Option<(u16, u16)>,
}
impl CompletionPanel {
    fn new(session: SessionId, input: &PromptInput, items: Vec<CompletionItem>) -> Self {
        Self {
            session,
            input: input.clone(),
            source_input: input.clone(),
            matches: (0..items.len()).collect(),
            candidates: items,
            selected: 0,
            navigated: false,
            anchor: None,
        }
    }
    fn item(&self, index: usize) -> Option<&CompletionItem> {
        self.matches
            .get(index)
            .and_then(|&i| self.candidates.get(i))
    }
    fn ready_for(&self, input: Option<&PromptInput>) -> bool {
        input.is_some_and(|input| input.same_edit(&self.input))
    }
    fn update(&mut self, input: &PromptInput, items: Vec<CompletionItem>) -> bool {
        self.update_anchor(input);
        let selected = if self.navigated {
            self.item(self.selected)
                .and_then(|old| {
                    items
                        .iter()
                        .position(|new| new.insert_text == old.insert_text)
                })
                .unwrap_or(0)
        } else {
            0
        };
        let changed = selected != self.selected
            || self.matches.len() != items.len()
            || self.matches.iter().enumerate().any(|(i, _)| {
                self.item(i).is_none_or(|old| {
                    old.label != items[i].label
                        || old.description != items[i].description
                        || old.kind != items[i].kind
                })
            });
        self.selected = selected;
        self.input = input.clone();
        self.source_input = input.clone();
        self.matches = (0..items.len()).collect();
        self.candidates = items;
        changed
    }
    fn refilter(&mut self, input: &PromptInput) -> bool {
        let Some(ctx) = CompletionContext::parse(input) else {
            return false;
        };
        let Some(previous) = CompletionContext::parse(&self.source_input) else {
            return false;
        };
        let domain = |query: &str| {
            (
                query.starts_with('-'),
                query.rsplit_once('/').map(|(dir, _)| dir.to_owned()),
                query
                    .rsplit('/')
                    .next()
                    .is_some_and(|part| part.starts_with('.')),
            )
        };
        if self.source_input.cwd != input.cwd
            || self.source_input.shell != input.shell
            || self.source_input.home != input.home
            || self.source_input.search_path != input.search_path
            || self.source_input.text[..previous.start] != input.text[..ctx.start]
            || self.source_input.text[previous.end..] != input.text[ctx.end..]
            || previous.quote != ctx.quote
            || previous.expand_variable != ctx.expand_variable
            || domain(&previous.query) != domain(&ctx.query)
        {
            return false;
        }
        let matches = rank_indices(&self.candidates, &ctx.query);
        if matches.is_empty() {
            return false;
        }
        self.matches = matches;
        self.input = input.clone();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        true
    }
    fn update_anchor(&mut self, input: &PromptInput) {
        let same_token = CompletionContext::parse(&self.input)
            .zip(CompletionContext::parse(input))
            .is_some_and(|(old, new)| self.input.text[..old.start] == input.text[..new.start]);
        if !same_token {
            self.anchor = None;
        }
    }
    fn anchor(&mut self, line: u16, column: u16, columns: u16) -> (u16, u16) {
        *self.anchor.get_or_insert_with(|| {
            let prefix_width = CompletionContext::parse(&self.input)
                .map(|ctx| {
                    unicode_width::UnicodeWidthStr::width(
                        &self.input.text[ctx.start..self.input.cursor],
                    )
                })
                .unwrap_or(0);
            let columns = usize::from(columns.max(1));
            let start =
                (usize::from(line) * columns + usize::from(column)).saturating_sub(prefix_width);
            ((start / columns) as u16, (start % columns) as u16)
        })
    }
}
impl CompletionUi {
    fn begin_request(
        &mut self,
        session: SessionId,
        input: &PromptInput,
        native_refresh: bool,
    ) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.last_input = Some((session, input.clone()));
        self.native_pending = input.native && !native_refresh;
        self.local_empty = false;
        self.generation
    }
    fn cancel_requests(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.task = None;
        self.native_task = None;
        self.native_pending = false;
        self.local_empty = false;
    }
    fn dismiss(&mut self) {
        self.cancel_requests();
        self.panel = None;
        self.wants = false;
        self.explicit = false;
        self.last_input = None;
    }
    fn observe_input(
        &mut self,
        session: SessionId,
        editable: bool,
        input: Option<PromptInput>,
    ) -> Option<(PromptInput, bool)> {
        if !editable {
            self.dismiss();
            return None;
        }
        let Some(input) = input else {
            // Missing input at an editable prompt means redraw, not dismissal.
            self.cancel_requests();
            self.last_input = None;
            return None;
        };
        if self
            .last_input
            .as_ref()
            .is_some_and(|(s, i)| *s == session && i == &input)
        {
            return None;
        }
        let native_refresh = self.last_input.as_ref().is_some_and(|(s, previous)| {
            *s == session
                && previous.same_edit(&input)
                && previous.native_revision != input.native_revision
        });
        self.last_input = Some((session, input.clone()));
        if input.text.is_empty() && !self.explicit {
            self.dismiss();
            return None;
        }
        if !self.wants {
            return None;
        }
        if let Some(panel) = self.panel.as_mut() {
            if panel.session != session {
                self.panel = None;
            } else if panel.refilter(&input) && !native_refresh {
                self.scroll
                    .scroll_to_item(panel.selected, ScrollStrategy::Nearest);
            }
        }
        Some((input, native_refresh))
    }
    fn apply_results(
        &mut self,
        session: SessionId,
        input: &PromptInput,
        items: Vec<CompletionItem>,
    ) -> bool {
        self.local_empty = items.is_empty();
        if items.is_empty() {
            // A native provider can still supply candidates when the local provider has none.
            return !self.native_pending && self.panel.take().is_some();
        }
        if let Some(panel) = self.panel.as_mut() {
            panel.update(input, items)
        } else {
            self.scroll = UniformListScrollHandle::new();
            self.panel = Some(CompletionPanel::new(session, input, items));
            true
        }
    }
    fn finish_native(&mut self) -> bool {
        self.native_pending = false;
        self.local_empty && self.panel.take().is_some()
    }
}
impl EggieApp {
    pub(super) fn dismiss_completion(&mut self) {
        self.completion.dismiss();
    }
    fn completion_editable(&self, session: SessionId) -> bool {
        self.command_palette.is_none()
            && self.terminal_search.is_none()
            && !self.terminal_ime_states.contains_key(&session)
            && self
                .session_feed
                .snapshots
                .get(&session)
                .is_some_and(|frame| {
                    frame.prompt_editable
                        && !frame.input_modes.alternate_screen
                        && frame.display_offset == 0
                })
    }
    fn completion_input(&self, session: SessionId) -> Option<PromptInput> {
        if !self.completion_editable(session) {
            return None;
        }
        let frame = self.session_feed.snapshots.get(&session)?;
        frame.prompt_input.clone().filter(PromptInput::valid)
    }
    pub(super) fn trigger_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.settings.read(cx).config().terminal_suggestions {
            return false;
        }
        let Some(session) = self.active_session_id() else {
            return false;
        };
        if !self.completion_editable(session) {
            return false;
        }
        self.completion.wants = true;
        self.completion.explicit = true;
        if let Some(input) = self.completion_input(session) {
            self.request_completion(session, input, false, cx);
        }
        true
    }
    pub(super) fn completion_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session_id() else {
            return false;
        };
        if !self.completion_editable(session)
            || !self.settings.read(cx).config().terminal_suggestions
        {
            self.dismiss_completion();
            return false;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            self.dismiss_completion();
            return false;
        }
        let visible = self.completion.panel.as_ref().is_some_and(|p| {
            p.session == session
                && !p.matches.is_empty()
                && p.ready_for(self.completion_input(session).as_ref())
        });
        if visible {
            match key {
                "escape" => {
                    self.dismiss_completion();
                    cx.notify();
                    return true;
                }
                "down" | "up"
                    if key == "down"
                        || self.completion.panel.as_ref().is_some_and(|p| p.navigated) =>
                {
                    let p = self.completion.panel.as_mut().unwrap();
                    let count = p.matches.len();
                    p.selected = if key == "down" {
                        if !p.navigated {
                            0
                        } else {
                            (p.selected + 1).min(count - 1)
                        }
                    } else {
                        p.selected.saturating_sub(1)
                    };
                    p.navigated = true;
                    self.completion
                        .scroll
                        .scroll_to_item(p.selected, ScrollStrategy::Nearest);
                    cx.notify();
                    return true;
                }
                "tab" if modifiers.shift => {
                    let p = self.completion.panel.as_mut().unwrap();
                    p.selected = p.selected.saturating_sub(1);
                    p.navigated = true;
                    self.completion
                        .scroll
                        .scroll_to_item(p.selected, ScrollStrategy::Nearest);
                    cx.notify();
                    return true;
                }
                "pageup" | "pagedown" => {
                    let p = self.completion.panel.as_mut().unwrap();
                    p.selected = if key == "pageup" {
                        p.selected.saturating_sub(8)
                    } else {
                        (p.selected + 8).min(p.matches.len() - 1)
                    };
                    p.navigated = true;
                    self.completion
                        .scroll
                        .scroll_to_item(p.selected, ScrollStrategy::Nearest);
                    cx.notify();
                    return true;
                }
                "tab" | "enter"
                    if key == "tab" && !modifiers.shift
                        || self.completion.panel.as_ref().is_some_and(|p| p.navigated) =>
                {
                    return self.accept_completion(None, cx);
                }
                _ => {}
            }
        }
        if key == "tab" && !modifiers.shift {
            return self.trigger_completion(cx);
        }
        if matches!(key, "enter" | "escape" | "up" | "down") {
            self.dismiss_completion();
            cx.notify();
            return false;
        }
        self.completion.wants = event
            .keystroke
            .key_char
            .as_ref()
            .is_some_and(|s| !s.is_empty())
            || matches!(key, "backspace" | "delete" | "left" | "right");
        self.completion.explicit = false;
        if let Some(p) = self.completion.panel.as_mut() {
            p.navigated = false;
        }
        false
    }
    pub(super) fn sync_completion(&mut self, session: SessionId, cx: &mut Context<Self>) {
        if self.active_session_id() != Some(session) {
            return;
        }
        let editable = self.completion_editable(session)
            && self.settings.read(cx).config().terminal_suggestions;
        if let Some((input, native_refresh)) =
            self.completion
                .observe_input(session, editable, self.completion_input(session))
        {
            self.request_completion(session, input, native_refresh, cx);
        }
    }
    fn request_completion(
        &mut self,
        session: SessionId,
        input: PromptInput,
        native_refresh: bool,
        cx: &mut Context<Self>,
    ) {
        let generation = self
            .completion
            .begin_request(session, &input, native_refresh);
        let explicit = self.completion.explicit;
        let native_client = self.client.clone();
        let native_executor = cx.background_executor().clone();
        let native_input = input.clone();
        self.completion.native_task = (input.native && !native_refresh).then(|| {
            cx.spawn(async move |this, cx| {
                if !explicit {
                    native_executor.timer(NATIVE_DELAY).await;
                }
                let valid = this
                    .update(cx, |app, _| {
                        app.completion.generation == generation
                            && app.active_session_id() == Some(session)
                            && app
                                .completion_input(session)
                                .is_some_and(|p| p.same_edit(&native_input))
                    })
                    .unwrap_or(false);
                if valid {
                    let deadline = native_executor.timer(NATIVE_TIMEOUT);
                    let dispatch = native_executor.spawn(async move {
                        native_client.request(ClientRequest::TerminalCompletions {
                            session_id: session,
                            input: native_input,
                            native: true,
                        })
                    });
                    if let futures::future::Either::Left((_, deadline)) =
                        futures::future::select(dispatch, deadline).await
                    {
                        deadline.await;
                    }
                    let _ = this.update(cx, |app, cx| {
                        if app.completion.generation == generation && app.completion.finish_native()
                        {
                            cx.notify();
                        }
                    });
                }
            })
        });
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        self.completion.task = Some(cx.spawn(async move |this, cx| {
            if !explicit && !native_refresh {
                executor.timer(LOCAL_DELAY).await;
            }
            let valid = this
                .update(cx, |app, _| {
                    app.completion.generation == generation
                        && app.active_session_id() == Some(session)
                        && app
                            .completion_input(session)
                            .is_some_and(|p| p.same_edit(&input))
                })
                .unwrap_or(false);
            if !valid {
                return;
            }
            let request_client = client.clone();
            let request_input = input.clone();
            let result = executor
                .spawn(async move {
                    request_client.request(ClientRequest::TerminalCompletions {
                        session_id: session,
                        input: request_input,
                        native: false,
                    })
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.receive_completions(generation, session, &input, result, cx)
            });
        }));
    }
    fn receive_completions(
        &mut self,
        generation: u64,
        session: SessionId,
        input: &PromptInput,
        result: anyhow::Result<DaemonResponse>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.completion.generation != generation
            || self.active_session_id() != Some(session)
            || self
                .completion_input(session)
                .is_none_or(|p| !p.same_edit(input))
        {
            return false;
        }
        let items = match result {
            Ok(DaemonResponse::TerminalCompletions { items, .. }) => items,
            _ => Vec::new(),
        };
        let changed = self.completion.apply_results(session, input, items);
        if changed {
            if let Some(panel) = self.completion.panel.as_ref() {
                self.completion
                    .scroll
                    .scroll_to_item(panel.selected, ScrollStrategy::Nearest);
            }
            cx.notify();
        }
        true
    }
    fn accept_completion(&mut self, index: Option<usize>, cx: &mut Context<Self>) -> bool {
        let Some(panel) = self.completion.panel.as_ref() else {
            return false;
        };
        let Some(current) = self
            .completion_input(panel.session)
            .filter(|p| panel.ready_for(Some(p)))
        else {
            return false;
        };
        let Some(item) = panel.item(index.unwrap_or(panel.selected)) else {
            return false;
        };
        let Some(ctx) = CompletionContext::parse(&current) else {
            return false;
        };
        let item = if item.kind == CompletionKind::History {
            let mut item = item.clone();
            item.replace_end = current.text.len();
            item
        } else {
            ctx.item(&item.label, &item.description, item.kind)
        };
        let Some(bytes) = item.replacement(&current) else {
            return false;
        };
        let session = panel.session;
        let directory = item.kind == eggie_completion::CompletionKind::Directory;
        let enqueued = self.enqueue_terminal_input(session, bytes);
        self.dismiss_completion();
        self.completion.wants = directory;
        cx.notify();
        enqueued
    }
    pub(super) fn render_completion(
        &mut self,
        session: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !window.is_window_active()
            || self.active_session_id() != Some(session)
            || !self.completion_editable(session)
            || !self.settings.read(cx).config().terminal_suggestions
        {
            return None;
        }
        let panel = self
            .completion
            .panel
            .as_mut()
            .filter(|p| p.session == session && !p.matches.is_empty())?;
        let viewport = self
            .terminal_viewports
            .values()
            .find(|v| v.session_id == session)?;
        let snapshot = self.session_feed.snapshots.get(&session)?;
        let size = window.viewport_size();
        let margin = 8.;
        let width = 420_f32.min(f32::from(size.width) - margin * 2.).max(0.);
        let row_height = 30.;
        let count = panel.matches.len();
        let height = (count.min(8) as f32) * row_height + 32.;
        let (line, column) = panel.anchor(
            snapshot.cursor_line,
            snapshot.cursor_column,
            snapshot.size.columns,
        );
        let x = f32::from(viewport.bounds.origin.x) + column as f32 * viewport.cell_width;
        let y = f32::from(viewport.bounds.origin.y) + line as f32 * viewport.line_height;
        let below = y + viewport.line_height + 4.;
        let top = if below + height <= f32::from(size.height) - margin {
            below
        } else {
            (y - height - 4.).max(margin)
        };
        let position = point(
            px(x.min(f32::from(size.width) - width - margin).max(margin)),
            px(top),
        );
        let colors = self.colors;
        let list = uniform_list(
            "terminal-completions",
            count,
            cx.processor(|app, range: std::ops::Range<usize>, _, cx| {
                let Some(p) = app.completion.panel.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|i| {
                        p.item(i).map(|item| {
                            div()
                                .id(("completion", i))
                                .w_full()
                                .h(px(30.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .bg(rgb(if i == p.selected {
                                    app.colors.hover
                                } else {
                                    app.colors.panel
                                }))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |app, _, _, cx| {
                                        app.accept_completion(Some(i), cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(item.label.clone()),
                                )
                                .child(
                                    div()
                                        .max_w(px(190.))
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(rgb(app.colors.muted))
                                        .child(item.description.clone()),
                                )
                        })
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.completion.scroll)
        .w_full()
        .h(px(count.min(8) as f32 * row_height));
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(position)
                    .snap_to_window_with_margin(px(margin))
                    .child(
                        div()
                            .id("completion-panel")
                            .w(px(width))
                            .p(px(4.))
                            .rounded(px(14.))
                            .border_1()
                            .border_color(rgb(colors.border))
                            .bg(rgb(colors.panel))
                            .shadow_lg()
                            .text_color(rgb(colors.text))
                            .text_size(px(13.))
                            .occlude()
                            .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                                app.dismiss_completion();
                                cx.notify();
                            }))
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                            .child(list)
                            .child(
                                div()
                                    .h(px(24.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors.muted))
                                    .child("↑ ↓")
                                    .child("Tab ↹     Esc"),
                            ),
                    ),
            )
            .priority(5)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(s: &str) -> CompletionItem {
        CompletionContext::parse(&PromptInput::default())
            .unwrap()
            .item(s, "", eggie_completion::CompletionKind::Command)
    }
    #[test]
    fn async_native_candidates_preserve_a_users_keyboard_selection() {
        let mut panel = CompletionPanel::new(
            SessionId::nil(),
            &PromptInput::default(),
            vec![item("git"), item("gist")],
        );
        panel.selected = 1;
        panel.navigated = true;
        panel.update(
            &PromptInput::default(),
            vec![item("gist"), item("git"), item("gitk")],
        );
        assert_eq!(panel.item(panel.selected).unwrap().label, "gist");
        panel.navigated = false;
        panel.update(&PromptInput::default(), vec![item("git"), item("gist")]);
        assert_eq!(panel.selected, 0);
        panel.navigated = true;
        panel.selected = 1;
        panel.update(&PromptInput::default(), vec![item("git")]);
        assert_eq!(panel.selected, 0);
    }
    fn input(text: &str) -> PromptInput {
        PromptInput {
            text: text.into(),
            cursor: text.len(),
            ..Default::default()
        }
    }
    #[test]
    fn cached_candidates_survive_narrowing_and_backspace_without_rebuilding_edits() {
        let input = input("cat ");
        let context = CompletionContext::parse(&input).unwrap();
        let mut panel = CompletionPanel::new(
            SessionId::nil(),
            &input,
            vec![
                context.item("a file.txt", "", CompletionKind::File),
                context.item("another.txt", "", CompletionKind::File),
                context.item("中文.txt", "", CompletionKind::File),
            ],
        );
        let text = |s| PromptInput {
            text: s,
            ..input.clone()
        };
        let narrow = PromptInput {
            cursor: 8,
            ..text("cat a\\ f".into())
        };
        assert!(panel.refilter(&narrow));
        assert_eq!(panel.matches.len(), 1);
        let broad = PromptInput {
            cursor: 5,
            ..text("cat a".into())
        };
        assert!(panel.refilter(&broad));
        assert_eq!(panel.matches.len(), 2);
        let candidate = panel.item(0).unwrap();
        let edited = CompletionContext::parse(&broad).unwrap().item(
            &candidate.label,
            &candidate.description,
            candidate.kind,
        );
        assert_eq!(edited.replace_start, 4);
        assert_eq!(edited.replace_end, 5);
        assert_eq!(edited.insert_text, "a\\ file.txt");
        assert!(!panel.refilter(&PromptInput {
            text: "cat folder/a".into(),
            cursor: 12,
            ..input.clone()
        }));
        assert!(!panel.refilter(&PromptInput {
            text: "cat --a".into(),
            cursor: 7,
            ..input.clone()
        }));
        assert!(!panel.refilter(&PromptInput {
            text: "cat .a".into(),
            cursor: 6,
            ..input.clone()
        }));
        assert!(!panel.refilter(&PromptInput {
            text: "cat 'a".into(),
            cursor: 6,
            ..input.clone()
        }));
        assert!(!panel.refilter(&PromptInput {
            text: "echo a".into(),
            cursor: 6,
            ..input.clone()
        }));
    }
    #[test]
    fn unchanged_candidates_do_not_redraw_or_lose_selection() {
        let input = input("gi");
        let mut panel =
            CompletionPanel::new(SessionId::nil(), &input, vec![item("git"), item("gist")]);
        panel.navigated = true;
        panel.selected = 1;
        let mut revised = input.clone();
        revised.native_revision = 1;
        assert!(!panel.update(&revised, vec![item("git"), item("gist")]));
        assert_eq!(panel.selected, 1);
        assert!(panel.update(&revised, vec![item("gist"), item("git")]));
        assert_eq!(panel.selected, 0);
    }
    #[test]
    fn continuous_typing_keeps_the_panel_through_redraws_and_context_changes() {
        let session = SessionId::new_v4();
        let mut ui = CompletionUi {
            wants: true,
            ..Default::default()
        };
        let first = input("g");
        ui.begin_request(session, &first, false);
        ui.apply_results(session, &first, vec![item("git"), item("grep")]);
        for text in [
            "gi",
            "git",
            "git ",
            "git c",
            "git ch",
            "git checkout ",
            "git checkout test-",
        ] {
            let generation = ui.generation;
            assert!(ui.observe_input(session, true, None).is_none());
            assert!(ui.wants && ui.panel.is_some(), "redraw hid {text:?}");
            assert!(
                ui.generation > generation,
                "obsolete work survived a redraw"
            );
            assert!(!ui.panel.as_ref().unwrap().ready_for(None));
            let next = input(text);
            let (request, refresh) = ui.observe_input(session, true, Some(next.clone())).unwrap();
            assert_eq!(request, next);
            ui.begin_request(session, &request, refresh);
            assert!(ui.panel.is_some(), "context transition hid {text:?}");
            ui.apply_results(session, &request, vec![item("test-feature")]);
            assert!(ui.panel.as_ref().unwrap().ready_for(Some(&next)));
        }
        ui.observe_input(session, false, None);
        assert!(
            ui.panel.is_none() && !ui.wants,
            "command output left suggestions open"
        );
    }
    #[test]
    fn native_only_candidates_do_not_close_and_reopen_the_panel() {
        let session = SessionId::nil();
        let mut ui = CompletionUi {
            wants: true,
            ..Default::default()
        };
        let first = input("gi");
        ui.apply_results(session, &first, vec![item("git")]);
        let next = PromptInput {
            native: true,
            ..input("eggie_test_")
        };
        ui.observe_input(session, true, Some(next.clone()));
        ui.begin_request(session, &next, false);
        assert!(!ui.apply_results(session, &next, vec![]));
        assert!(ui.panel.is_some());
        assert!(
            !ui.panel.as_ref().unwrap().ready_for(Some(&next)),
            "old candidates were actionable"
        );
        let completed = PromptInput {
            native_revision: 1,
            ..next
        };
        let (request, refresh) = ui
            .observe_input(session, true, Some(completed.clone()))
            .unwrap();
        assert!(refresh);
        ui.begin_request(session, &request, refresh);
        ui.apply_results(session, &request, vec![item("eggie_test_alias")]);
        assert!(ui.panel.as_ref().unwrap().ready_for(Some(&completed)));
        assert_eq!(
            ui.panel.as_ref().unwrap().item(0).unwrap().label,
            "eggie_test_alias"
        );
    }
    #[test]
    fn narrowing_to_no_cached_match_waits_for_the_actual_provider() {
        let session = SessionId::nil();
        let first = input("a");
        let mut ui = CompletionUi {
            wants: true,
            ..Default::default()
        };
        ui.apply_results(session, &first, vec![item("abc")]);
        let next = input("axy");
        ui.observe_input(session, true, Some(next.clone()));
        assert_eq!(ui.panel.as_ref().unwrap().item(0).unwrap().label, "abc");
        assert!(!ui.panel.as_ref().unwrap().ready_for(Some(&next)));
        ui.begin_request(session, &next, false);
        assert!(ui.apply_results(session, &next, vec![]));
        assert!(ui.panel.is_none(), "confirmed empty results were kept");
    }
    #[test]
    fn native_empty_results_and_timeouts_retire_only_unmatched_panels() {
        for timed_out in [false, true] {
            let session = SessionId::nil();
            let first = input("g");
            let mut ui = CompletionUi {
                wants: true,
                ..Default::default()
            };
            ui.apply_results(session, &first, vec![item("git")]);
            let next = PromptInput {
                native: true,
                ..input("unknown-command ")
            };
            ui.begin_request(session, &next, false);
            assert!(!ui.apply_results(session, &next, vec![]));
            if timed_out {
                assert!(ui.finish_native());
            } else {
                let completed = PromptInput {
                    native_revision: 1,
                    ..next
                };
                let (request, refresh) = ui.observe_input(session, true, Some(completed)).unwrap();
                assert!(refresh);
                ui.begin_request(session, &request, refresh);
                assert!(ui.apply_results(session, &request, vec![]));
            }
            assert!(ui.panel.is_none());
            let local = PromptInput {
                native: true,
                ..input("g")
            };
            ui.begin_request(session, &local, false);
            ui.apply_results(session, &local, vec![item("git")]);
            assert!(!ui.finish_native());
            assert!(
                ui.panel.is_some(),
                "native timeout discarded local candidates"
            );
        }
    }
    #[test]
    fn empty_input_and_explicit_dismissal_end_the_visible_lifecycle() {
        let session = SessionId::nil();
        let first = input("g");
        let mut ui = CompletionUi {
            wants: true,
            ..Default::default()
        };
        ui.apply_results(session, &first, vec![item("git")]);
        ui.observe_input(session, true, Some(input("")));
        assert!(ui.panel.is_none() && !ui.wants);
        ui.wants = true;
        ui.explicit = true;
        assert!(ui.observe_input(session, true, Some(input(""))).is_some());
        ui.apply_results(session, &input(""), vec![item("git")]);
        ui.dismiss();
        assert!(ui.panel.is_none() && !ui.wants);
        assert!(ui.observe_input(session, true, Some(first)).is_none());
    }
    #[test]
    fn the_panel_anchor_stays_at_the_token_start_until_the_next_argument() {
        let session = SessionId::nil();
        let mut panel = CompletionPanel::new(session, &input("git ch"), vec![item("checkout")]);
        assert_eq!(panel.anchor(2, 12, 80), (2, 10));
        assert!(panel.refilter(&input("git che")));
        assert_eq!(panel.anchor(2, 13, 80), (2, 10));
        panel.update(&input("git chec"), vec![item("checkout")]);
        assert_eq!(panel.anchor(2, 14, 80), (2, 10));
        panel.update(&input("git checkout te"), vec![item("test-feature")]);
        assert_eq!(panel.anchor(2, 20, 80), (2, 18));
        let mut unicode = CompletionPanel::new(session, &input("cat 中"), vec![item("中文.txt")]);
        assert_eq!(
            unicode.anchor(1, 1, 10),
            (0, 9),
            "wide text crossing a line break"
        );
    }
}
