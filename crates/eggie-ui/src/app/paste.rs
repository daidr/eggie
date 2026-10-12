use super::*;
use crate::settings::PastePreviewMode;
use eggie_protocol::{
    TerminalPasteContext, TerminalPastePhase, TerminalPasteStatus, paste_has_controls,
    sanitize_paste,
};
use std::sync::atomic::AtomicBool;

pub(super) struct PasteTransferUi {
    id: u64,
    started: Instant,
    status: TerminalPasteStatus,
    cancelled: Arc<AtomicBool>,
}

pub(super) fn context(snapshot: &TerminalSnapshot) -> TerminalPasteContext {
    TerminalPasteContext {
        bracketed: snapshot.input_modes.bracketed_paste,
        alternate_screen: snapshot.input_modes.alternate_screen,
        prompt_editable: snapshot.prompt_editable,
        paste_events: snapshot.input_modes.paste_events,
    }
}

fn needs_preview(text: &str, context: TerminalPasteContext, mode: PastePreviewMode) -> bool {
    if mode == PastePreviewMode::Never {
        return false;
    }
    if paste_has_controls(text) {
        return true;
    }
    if context.alternate_screen || context.paste_events || !text.contains(['\r', '\n']) {
        return false;
    }
    match mode {
        PastePreviewMode::WhenUnsafe => !context.bracketed,
        PastePreviewMode::Multiline => context.prompt_editable || !context.bracketed,
        PastePreviewMode::Never => false,
    }
}

fn byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}

impl EggieApp {
    pub(super) fn paste_into_active_terminal(
        &mut self,
        force_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.paste_preview_open {
            return true;
        }
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let Some(snapshot) = self.session_feed.snapshots.get(&session_id) else {
            return false;
        };
        let target = context(snapshot);
        self.dismiss_completion();
        if target.paste_events && !force_preview {
            let sent = self.enqueue_terminal_clipboard_paste(
                session_id,
                TerminalClipboardSelection::Clipboard,
                clipboard_contents_from_item(item, &["*/*".to_owned()]),
            );
            if sent {
                let _ = self.input_sender.send_selection_clear(session_id);
                self.terminal_ime_states.remove(&session_id);
                cx.notify();
            }
            return sent;
        }
        let Some(text) = item.text() else {
            return false;
        };
        let preview = force_preview
            || needs_preview(&text, target, self.settings.read(cx).config().paste_preview);
        if !preview {
            return self.start_terminal_paste(session_id, text, Some(target), cx);
        }
        let cleaned = sanitize_paste(&text);
        let zh = self.language == Language::SimplifiedChinese;
        let mut summary = String::new();
        if paste_has_controls(&text) {
            summary.push_str(if zh {
                "\n已移除控制字符"
            } else {
                "\nControl characters removed"
            });
        }
        if !target.bracketed && !target.alternate_screen && cleaned.contains('\n') {
            summary.push_str(if zh {
                "\n换行可能执行命令"
            } else {
                "\nLine breaks may execute commands"
            });
        }
        let result = native_paste::preview(window, &cleaned, summary.trim(), self.language);
        self.paste_preview_open = true;
        cx.spawn(async move |this, cx| {
            let result = result.await.ok().flatten();
            this.update(cx, |app, cx| {
                app.paste_preview_open = false;
                if let Some(text) = result {
                    app.start_terminal_paste(session_id, text, Some(target), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    fn start_terminal_paste(
        &mut self,
        session_id: SessionId,
        text: String,
        expected: Option<TerminalPasteContext>,
        cx: &mut Context<Self>,
    ) -> bool {
        if text.is_empty() {
            return true;
        }
        let id = Self::next_input_sequence();
        let total_bytes = text.len();
        let Ok(receiver) = self
            .input_sender
            .send_paste_text(session_id, text, id, expected)
        else {
            return false;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        self.paste_transfers.insert(
            session_id,
            PasteTransferUi {
                id,
                started: Instant::now(),
                cancelled: cancelled.clone(),
                status: TerminalPasteStatus {
                    id,
                    total_bytes,
                    written_bytes: 0,
                    phase: TerminalPastePhase::Sending,
                    error: None,
                },
            },
        );
        let _ = self.input_sender.send_selection_clear(session_id);
        self.terminal_ime_states.remove(&session_id);
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let initial = executor
                .spawn(async move { receiver.recv().map_err(|e| e.to_string()).and_then(|s| s) })
                .await;
            let mut status = initial;
            let mut cancel_sent = false;
            loop {
                if let Ok(s) = &status
                    && s.phase == TerminalPastePhase::Failed
                {
                    status = Err(s.error.clone().unwrap_or_default());
                }
                let finished = status
                    .as_ref()
                    .is_ok_and(|s| s.phase != TerminalPastePhase::Sending)
                    || status.is_err();
                let result = this.update_in(cx, |app, window, cx| {
                    if let Some(transfer) = app
                        .paste_transfers
                        .get_mut(&session_id)
                        .filter(|transfer| transfer.id == id)
                    {
                        if let Ok(status) = &status {
                            transfer.status = status.clone();
                        }
                        if finished {
                            app.paste_transfers.remove(&session_id);
                            if let Err(error) = &status {
                                let message = if error.contains("paste target changed") {
                                    if app.language == Language::SimplifiedChinese {
                                        "终端状态已变化，请重新粘贴".to_owned()
                                    } else {
                                        "Terminal changed. Paste again".to_owned()
                                    }
                                } else if error.contains("terminal process exited") {
                                    if app.language == Language::SimplifiedChinese {
                                        "进程已结束".to_owned()
                                    } else {
                                        "Process exited".to_owned()
                                    }
                                } else if error.contains("wire message exceeds") {
                                    if app.language == Language::SimplifiedChinese {
                                        "粘贴内容过大".to_owned()
                                    } else {
                                        "Paste is too large".to_owned()
                                    }
                                } else {
                                    if app.language == Language::SimplifiedChinese {
                                        "终端连接已中断".to_owned()
                                    } else {
                                        "Terminal disconnected".to_owned()
                                    }
                                };
                                let prompt = window.prompt(
                                    PromptLevel::Warning,
                                    &message,
                                    None,
                                    &[if app.language == Language::SimplifiedChinese {
                                        "确定"
                                    } else {
                                        "OK"
                                    }],
                                    cx,
                                );
                                window
                                    .spawn(cx, async move |_| {
                                        let _ = prompt.await;
                                    })
                                    .detach();
                            }
                        }
                        cx.notify();
                    }
                });
                if result.is_err() {
                    let client = client.clone();
                    executor
                        .spawn(async move {
                            let _ = client.request(ClientRequest::CancelPaste { session_id, id });
                        })
                        .await;
                    break;
                }
                if finished {
                    break;
                }
                executor.timer(Duration::from_millis(32)).await;
                let request = if cancelled.load(Ordering::Acquire) && !cancel_sent {
                    cancel_sent = true;
                    ClientRequest::CancelPaste { session_id, id }
                } else {
                    ClientRequest::PasteStatus { session_id, id }
                };
                let client = client.clone();
                status = executor
                    .spawn(async move {
                        match client.request(request) {
                            Ok(DaemonResponse::PasteStatus { status }) => Ok(status),
                            Ok(_) => Err("Unexpected paste response".to_owned()),
                            Err(error) => Err(error.to_string()),
                        }
                    })
                    .await;
            }
        })
        .detach();
        cx.notify();
        true
    }

    pub(super) fn cancel_active_paste(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(transfer) = self
            .active_session_id()
            .and_then(|id| self.paste_transfers.get(&id))
        else {
            return false;
        };
        transfer.cancelled.store(true, Ordering::Release);
        cx.notify();
        true
    }

    pub(super) fn render_paste_progress(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let transfer = self.paste_transfers.get(&session_id)?;
        if transfer.started.elapsed() < Duration::from_millis(150) {
            return None;
        }
        let colors = self.colors;
        let fraction = (transfer.status.written_bytes as f32
            / transfer.status.total_bytes.max(1) as f32)
            .clamp(0., 1.);
        let zh = self.language == Language::SimplifiedChinese;
        let text = format!(
            "{}  {} / {}",
            if zh { "粘贴中" } else { "Pasting" },
            byte_size(transfer.status.written_bytes),
            byte_size(transfer.status.total_bytes)
        );
        Some(
            div()
                .id(SharedString::from(format!(
                    "terminal-paste-progress-{session_id}"
                )))
                .absolute()
                .bottom(px(16.))
                .left(px(16.))
                .w(px(280.))
                .max_w(relative(0.9))
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_xl()
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.panel))
                .shadow_md()
                .occlude()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().text_xs().child(text))
                        .child(
                            div()
                                .id("cancel-terminal-paste")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|el| el.bg(rgb(colors.panel_alt)))
                                .child(if zh { "取消" } else { "Cancel" })
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if let Some(transfer) = app.paste_transfers.get(&session_id) {
                                        transfer.cancelled.store(true, Ordering::Release);
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .h(px(3.))
                        .w_full()
                        .rounded_full()
                        .bg(rgb(colors.panel_alt))
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_full()
                                .bg(rgb(colors.accent)),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_respects_application_modes_and_never_interrupts_normal_typing() {
        let mut target = TerminalPasteContext::default();
        assert!(!needs_preview(
            "echo hello",
            target,
            PastePreviewMode::WhenUnsafe
        ));
        assert!(needs_preview(
            "echo hello\n",
            target,
            PastePreviewMode::WhenUnsafe
        ));
        assert!(needs_preview(
            "echo a\necho b",
            target,
            PastePreviewMode::WhenUnsafe
        ));
        target.bracketed = true;
        assert!(!needs_preview("a\nb", target, PastePreviewMode::WhenUnsafe));
        target.prompt_editable = true;
        assert!(needs_preview("a\nb", target, PastePreviewMode::Multiline));
        target.alternate_screen = true;
        assert!(!needs_preview("a\nb", target, PastePreviewMode::Multiline));
        assert!(needs_preview(
            "a\u{3}b",
            target,
            PastePreviewMode::WhenUnsafe
        ));
        assert!(!needs_preview("a\u{3}b", target, PastePreviewMode::Never));
        target.alternate_screen = false;
        target.bracketed = false;
        target.paste_events = true;
        assert!(!needs_preview("a\nb", target, PastePreviewMode::Multiline));
    }
}
