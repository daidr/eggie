use super::*;
use eggie_protocol::RemoteSessionPhase;

fn status_surface(session_id: SessionId, colors: UiColors, top: f32) -> Stateful<Div> {
    terminal_overlay_surface(format!("ssh-status-{session_id}"), colors)
        .absolute()
        .top(px(top))
        .right(px(12.))
        .w(px(340.))
        .max_w(relative(0.9))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
}

fn status_icon(
    session_id: SessionId,
    phase: RemoteSessionPhase,
    tint: u32,
    colors: UiColors,
) -> AnyElement {
    let glyph = if phase == RemoteSessionPhase::Connecting {
        div()
            .size(px(18.))
            .with_animation(
                format!("ssh-connecting-{session_id}"),
                Animation::new(Duration::from_millis(1200)).repeat(),
                move |element, phase| {
                    element.child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                paint_indicator_ring(
                                    bounds,
                                    TerminalProgressState::Indeterminate,
                                    None,
                                    phase,
                                    tint,
                                    colors.border,
                                    window,
                                );
                            },
                        )
                        .size_full(),
                    )
                },
            )
            .into_any_element()
    } else {
        icon_sized(
            if phase == RemoteSessionPhase::Exited {
                IconName::CheckmarkCircle
            } else {
                IconName::AlertCircle
            },
            18.,
        )
        .into_any_element()
    };
    div()
        .flex_none()
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .bg(rgba((tint << 8) | 0x14))
        .text_color(rgb(tint))
        .child(glyph)
        .into_any_element()
}

pub(super) fn inherited_remote_directory(
    profile: &Profile,
    source: Option<&SessionSummary>,
) -> Option<String> {
    let target = profile.remote.as_ref()?;
    let source = source?;
    (source.remote.as_ref()?.target == *target)
        .then(|| source.remote_directory().map(str::to_owned))
        .flatten()
}

pub(super) fn phase_label(phase: RemoteSessionPhase, language: Language) -> &'static str {
    match (phase, language == Language::SimplifiedChinese) {
        (RemoteSessionPhase::Connecting, true) => "连接中",
        (RemoteSessionPhase::Connected, true) => "已连接",
        (RemoteSessionPhase::Disconnected, true) => "连接已断开",
        (RemoteSessionPhase::Failed, true) => "连接失败",
        (RemoteSessionPhase::Exited, true) => "已结束",
        (RemoteSessionPhase::Connecting, false) => "Connecting",
        (RemoteSessionPhase::Connected, false) => "Connected",
        (RemoteSessionPhase::Disconnected, false) => "Disconnected",
        (RemoteSessionPhase::Failed, false) => "Connection failed",
        (RemoteSessionPhase::Exited, false) => "Exited",
    }
}

fn failure_label(kind: eggie_protocol::SshFailureKind, language: Language) -> &'static str {
    use eggie_protocol::SshFailureKind::*;
    match (kind, language == Language::SimplifiedChinese) {
        (Authentication, true) => "身份验证失败",
        (Authentication, false) => "Authentication failed",
        (HostKey, true) => "主机密钥验证失败",
        (HostKey, false) => "Host key verification failed",
        (NameResolution, true) => "无法解析主机名",
        (NameResolution, false) => "Host name could not be resolved",
        (Timeout, true) => "连接超时",
        (Timeout, false) => "Connection timed out",
        (Refused, true) => "连接被拒绝",
        (Refused, false) => "Connection refused",
        (Network, true) => "网络连接中断",
        (Network, false) => "Network connection interrupted",
    }
}

fn failure_hint(kind: eggie_protocol::SshFailureKind, language: Language) -> &'static str {
    use eggie_protocol::SshFailureKind::*;
    match (kind, language == Language::SimplifiedChinese) {
        (Authentication, true) => "检查用户名、密钥或 SSH Agent",
        (Authentication, false) => "Check the user, identity key or SSH agent",
        (HostKey, true) => "核对主机指纹和 known_hosts",
        (HostKey, false) => "Verify the host fingerprint and known_hosts",
        (NameResolution, true) => "检查 Host 别名或主机地址",
        (NameResolution, false) => "Check the Host alias or address",
        (Timeout, true) => "检查网络、VPN 和跳板机",
        (Timeout, false) => "Check the network, VPN and jump host",
        (Refused, true) => "检查端口和远端 SSH 服务",
        (Refused, false) => "Check the port and remote SSH service",
        (Network, true) => "恢复网络后重新连接",
        (Network, false) => "Reconnect when the network is available",
    }
}

impl EggieApp {
    pub(super) fn source_session(
        &self,
        group: GroupId,
        item: Option<ItemId>,
    ) -> Option<&SessionSummary> {
        let group = self.workspace_state.workspace().layout.find_group(group)?;
        let item = item
            .and_then(|id| group.items.iter().find(|item| item.id == id))
            .or_else(|| group.active_item())?;
        self.session_feed
            .sessions
            .iter()
            .find(|session| Some(session.id) == item.session_id)
    }

    pub(super) fn show_ssh_error(
        &self,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = if self.language == Language::SimplifiedChinese {
            "无法打开终端"
        } else {
            "Cannot open terminal"
        };
        let prompt = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&message),
            &[if self.language == Language::SimplifiedChinese {
                "好"
            } else {
                "OK"
            }],
            cx,
        );
        cx.spawn(async move |_, _| {
            let _ = prompt.await;
        })
        .detach();
    }

    pub(super) fn render_ssh_status(
        &self,
        group: &TabGroup,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let item = group.active_item()?;
        let session = self
            .session_feed
            .global_sessions
            .iter()
            .find(|s| Some(s.id) == item.session_id)?;
        let remote = session.remote.as_ref()?;
        if remote.phase == RemoteSessionPhase::Connected {
            return None;
        }
        let session_id = session.id;
        let connecting = remote.phase == RemoteSessionPhase::Connecting;
        let colors = self.colors;
        let tint = match remote.phase {
            RemoteSessionPhase::Connecting => colors.accent,
            RemoteSessionPhase::Failed => self.interface_theme.palette[1],
            _ => colors.muted,
        };
        let busy = self
            .window_manager
            .read(cx)
            .pending_termination
            .contains(&session_id);
        let action = if busy {
            if connecting { "取消中" } else { "连接中" }
        } else if connecting {
            "取消"
        } else {
            "重新连接"
        };
        let action = if self.language == Language::SimplifiedChinese {
            action
        } else if busy {
            if connecting {
                "Cancelling"
            } else {
                "Connecting"
            }
        } else if connecting {
            "Cancel"
        } else {
            "Reconnect"
        };
        let error = self
            .window_manager
            .read(cx)
            .reconnect_errors
            .get(&session_id)
            .cloned();
        let destination = remote.target.consent_key();
        let top = if self
            .terminal_search
            .as_ref()
            .is_some_and(|search| search.session_id == session_id)
        {
            54.
        } else {
            12.
        };
        let row = div()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(12.))
            .child(status_icon(session_id, remote.phase, tint, colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div().font_weight(gpui::FontWeight::MEDIUM).child(
                            remote
                                .failure
                                .map(|kind| failure_label(kind, self.language))
                                .unwrap_or_else(|| phase_label(remote.phase, self.language)),
                        ),
                    )
                    .child(
                        div()
                            .id(format!("ssh-status-destination-{session_id}"))
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .tooltip(tooltip_text(destination.clone(), colors))
                            .child(destination),
                    ),
            )
            .child(
                div()
                    .id(format!("ssh-reconnect-{session_id}"))
                    .flex_none()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .px_2()
                    .bg(rgb(colors.panel_alt))
                    .text_color(rgb(colors.accent))
                    .when(!busy, |button| {
                        button
                            .cursor_pointer()
                            .hover(move |button| button.bg(rgb(colors.hover)))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let client = app.client.clone();
                                app.window_manager.update(cx, |manager, cx| {
                                    if connecting {
                                        manager.terminate(session_id, client, cx);
                                    } else {
                                        manager.reconnect(session_id, client, cx);
                                    }
                                });
                            }))
                    })
                    .when(busy, |button| button.opacity(0.5))
                    .child(action),
            );
        Some(
            status_surface(session_id, colors, top)
                .child(row)
                .children(remote.failure.map(|kind| {
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .child(failure_hint(kind, self.language))
                }))
                .children(error.map(|error| {
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .child(error)
                }))
                .with_animation(
                    format!("ssh-status-enter-{session_id}"),
                    Animation::new(Duration::from_millis(180)).with_easing(gpui::ease_out_quint()),
                    move |surface, progress| {
                        surface
                            .opacity(progress)
                            .top(px(top - 4. * (1. - progress)))
                    },
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_ssh_identity(&self, session: &SessionSummary) -> Option<AnyElement> {
        let remote = session.remote.as_ref()?;
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section_label("SSH", self.colors))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(remote.target.consent_key()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(self.colors.muted))
                                .child(phase_label(remote.phase, self.language)),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn floating_status_preserves_content_size_and_blocks_terminal_mouse_input(
        cx: &mut gpui::TestAppContext,
    ) {
        struct StatusView {
            terminal_downs: Rc<Cell<usize>>,
            actions: Rc<Cell<usize>>,
            content_size: Rc<Cell<gpui::Size<Pixels>>>,
        }
        impl Render for StatusView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let terminal_downs = self.terminal_downs.clone();
                let actions = self.actions.clone();
                let content_size = self.content_size.clone();
                let colors = UiColors {
                    background: 0xffffff,
                    panel: 0xffffff,
                    panel_alt: 0xeeeeee,
                    hover: 0xdddddd,
                    border: 0xcccccc,
                    text: 0,
                    muted: 0x888888,
                    accent: 0x3366ff,
                    accent_foreground: 0xffffff,
                    accent_hover: 0x2244ff,
                };
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .w(px(400.))
                    .h(px(240.))
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        terminal_downs.set(terminal_downs.get() + 1);
                    })
                    .on_children_prepainted(move |children, _, _| {
                        content_size.set(children[0].size);
                    })
                    .child(div().flex_1().w_full())
                    .child(
                        status_surface(SessionId::nil(), colors, 12.)
                            .h(px(64.))
                            .child(
                                div()
                                    .id("status-action")
                                    .w(px(80.))
                                    .h(px(28.))
                                    .on_click(move |_, _, _| actions.set(actions.get() + 1)),
                            ),
                    )
            }
        }
        let terminal_downs = Rc::new(Cell::new(0));
        let actions = Rc::new(Cell::new(0));
        let content_size = Rc::new(Cell::new(size(px(0.), px(0.))));
        let window = cx.add_window(|_, _| StatusView {
            terminal_downs: terminal_downs.clone(),
            actions: actions.clone(),
            content_size: content_size.clone(),
        });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(content_size.get(), size(px(400.), px(240.)));
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_click(point(px(60.), px(16.)), gpui::Modifiers::default());
        assert_eq!(
            terminal_downs.get(),
            0,
            "card padding must not reach the terminal"
        );
        visual.simulate_click(point(px(85.), px(40.)), gpui::Modifiers::default());
        assert_eq!(actions.get(), 1, "the action must still receive clicks");
        assert_eq!(terminal_downs.get(), 0);
        visual.simulate_click(point(px(5.), px(200.)), gpui::Modifiers::default());
        assert_eq!(
            terminal_downs.get(),
            1,
            "terminal input outside the card remains available"
        );
    }
}
