use super::*;
use eggie_protocol::{
    PortForwardKind, PortMapping, PortMappingErrorKind, PortMappingRequest, PortMappingState,
    PortMappingsSnapshot, PortServiceKind,
};
mod render;
#[cfg(test)]
mod tests;

fn service_target_host(address: &str) -> &str {
    match address {
        "*" | "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        address => address,
    }
}

// A local tab must never inherit a connection from another terminal or window.
fn port_snapshot_for_session<'a>(
    session: SessionId,
    inspections: &'a HashMap<SessionId, SessionInspection>,
    connections: &'a [PortMappingsSnapshot],
) -> Option<&'a PortMappingsSnapshot> {
    let own = inspections.get(&session)?.port_mappings.as_ref()?;
    Some(
        connections
            .iter()
            .find(|connection| {
                connection.connection_id == own.connection_id && connection.revision >= own.revision
            })
            .unwrap_or(own),
    )
}

#[derive(Default)]
pub(super) struct PortsUi {
    editor: Option<PortEditor>,
    busy: HashSet<String>,
    details: HashSet<String>,
    services_expanded: HashSet<String>,
    notice: Option<(String, String)>,
}
struct PortEditor {
    appearance: (UiColors, Language),
    id: ItemId,
    connection_id: String,
    remote: Entity<TextInput>,
    local: Entity<TextInput>,
    host: Entity<TextInput>,
    focus: FocusHandle,
    kind: PortForwardKind,
    service: PortServiceKind,
    advanced: bool,
    remember: bool,
    submitting: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}
fn parse_port(text: &str, required: bool) -> Result<Option<u16>, &'static str> {
    if text.trim().is_empty() {
        return if required {
            Err("missing_target_port")
        } else {
            Ok(None)
        };
    }
    text.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .map(Some)
        .ok_or("invalid_port")
}
fn error_label(kind: PortMappingErrorKind, language: Language) -> &'static str {
    match (kind, language == Language::SimplifiedChinese) {
        (PortMappingErrorKind::PortInUse, true) => "端口已被占用",
        (PortMappingErrorKind::PortInUse, false) => "Port is already in use",
        (PortMappingErrorKind::ConnectionUnavailable, true) => "SSH 连接已断开",
        (PortMappingErrorKind::ConnectionUnavailable, false) => "SSH disconnected",
        (PortMappingErrorKind::AccessDenied, true) => "没有权限使用此端口",
        (PortMappingErrorKind::AccessDenied, false) => "Port access denied",
        (PortMappingErrorKind::PersistenceFailed, true) => "映射设置未保存",
        (PortMappingErrorKind::PersistenceFailed, false) => "Mapping settings were not saved",
        (_, true) => "连接失败",
        (_, false) => "Connection failed",
    }
}
fn validation_label(error: &str, language: Language) -> &'static str {
    match (error, language == Language::SimplifiedChinese) {
        ("missing_target_port", true) => "请输入服务端口",
        ("missing_target_port", false) => "Enter the service port",
        ("invalid_target_host", true) => "请输入有效的目标地址",
        ("invalid_target_host", false) => "Enter a valid target address",
        (_, true) => "端口需在 1–65535 之间",
        (_, false) => "Port must be between 1 and 65535",
    }
}
impl EggieApp {
    pub(super) fn sync_port_editor_appearance(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.ports_ui.editor else {
            return;
        };
        if editor.appearance == (self.colors, self.language) {
            return;
        }
        editor.appearance = (self.colors, self.language);
        let style = TextInputStyle {
            text_color: (self.colors.text << 8) | 0xff,
            placeholder_color: (self.colors.muted << 8) | 0xff,
            cursor_color: (self.colors.accent << 8) | 0xff,
            selection_color: (self.colors.accent << 8) | 0x55,
        };
        for input in [&editor.remote, &editor.local, &editor.host] {
            input.update(cx, |input, cx| {
                input.set_style(style, cx);
                input.set_language(self.language);
            });
        }
        editor.local.update(cx, |input, _| {
            input.set_placeholder(if self.language == Language::SimplifiedChinese {
                "自动"
            } else {
                "Auto"
            });
        });
    }
    pub(super) fn active_port_connection(&self, cx: &App) -> Option<PortMappingsSnapshot> {
        port_snapshot_for_session(
            self.active_session_id()?,
            &self.session_feed.session_inspections,
            &self.window_manager.read(cx).port_connections,
        )
        .cloned()
    }
    pub(super) fn sync_port_editor_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.ports_ui.editor else {
            return;
        };
        let visible = self.right_tab == RightTab::Info
            && !self.right_sidebar_collapsed
            && self
                .active_port_connection(cx)
                .is_some_and(|connection| connection.connection_id == editor.connection_id);
        if !visible {
            self.close_port_editor(window, cx);
        }
    }
    fn open_port_editor(
        &mut self,
        connection_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .ports_ui
            .busy
            .contains(&format!("{connection_id}/create"))
        {
            return;
        }
        self.dismiss_completion();
        let colors = self.colors;
        let language = self.language;
        let mut make_input = |placeholder: &'static str, initial: &str| {
            cx.new(|cx| {
                let mut input = TextInput::new(
                    window,
                    cx,
                    TextInputStyle {
                        text_color: (colors.text << 8) | 0xff,
                        placeholder_color: (colors.muted << 8) | 0xff,
                        cursor_color: (colors.accent << 8) | 0xff,
                        selection_color: (colors.accent << 8) | 0x55,
                    },
                );
                input.set_placeholder(placeholder);
                input.set_language(language);
                input.set_content(initial.to_owned(), cx);
                input
            })
        };
        let remote = make_input("3000", "");
        let local = make_input(
            if language == Language::SimplifiedChinese {
                "自动"
            } else {
                "Auto"
            },
            "",
        );
        let host = make_input("127.0.0.1", "127.0.0.1");
        let subscriptions = [&remote, &local, &host]
            .into_iter()
            .map(|input| cx.subscribe_in(input, window, Self::port_input_event))
            .collect();
        remote.read(cx).focus_handle().focus(window, cx);
        self.ports_ui.editor = Some(PortEditor {
            appearance: (colors, language),
            id: ItemId::new_v4(),
            connection_id,
            remote,
            local,
            host,
            focus: cx.focus_handle(),
            kind: PortForwardKind::Local,
            service: PortServiceKind::Auto,
            advanced: false,
            remember: false,
            submitting: false,
            error: None,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }
    fn port_input_event(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Confirm => self.submit_port_editor(window, cx),
            TextInputEvent::Cancel => self.close_port_editor(window, cx),
            TextInputEvent::Changed => {
                if let Some(editor) = &mut self.ports_ui.editor {
                    editor.error = None;
                }
                cx.notify();
            }
            TextInputEvent::ConfirmReverse => {}
        }
    }
    pub(super) fn close_port_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ports_ui.editor.take().is_some() {
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }
    fn submit_port_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.ports_ui.editor else {
            return;
        };
        if editor.submitting {
            return;
        }
        let result = (|| {
            let request = PortMappingRequest {
                kind: editor.kind,
                local_port: parse_port(
                    editor.local.read(cx).content(),
                    editor.kind == PortForwardKind::Remote,
                )?,
                remote_port: if editor.kind == PortForwardKind::Dynamic {
                    None
                } else {
                    parse_port(
                        editor.remote.read(cx).content(),
                        editor.kind == PortForwardKind::Local,
                    )?
                },
                target_host: editor.host.read(cx).content().trim().to_owned(),
                service: editor.service,
                remember: editor.remember,
            };
            request.validate()?;
            Ok::<_, &'static str>(request)
        })();
        match result {
            Err(error) => {
                editor.error = Some(validation_label(error, self.language).into());
                cx.notify();
            }
            Ok(request) => {
                editor.submitting = true;
                editor.focus.focus(window, cx);
                let connection_id = editor.connection_id.clone();
                self.run_port_operation(
                    connection_id.clone(),
                    "create".into(),
                    ClientRequest::CreatePortMapping {
                        connection_id,
                        request,
                    },
                    true,
                    cx,
                );
            }
        }
    }
    fn run_port_operation(
        &mut self,
        connection_id: String,
        key: String,
        request: ClientRequest,
        creating: bool,
        cx: &mut Context<Self>,
    ) {
        let busy_key = format!("{connection_id}/{key}");
        if !self.ports_ui.busy.insert(busy_key.clone()) {
            return;
        }
        let editor_id = creating
            .then(|| self.ports_ui.editor.as_ref().map(|e| e.id))
            .flatten();
        self.ports_ui.notice = None;
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = executor.spawn(async move { client.request(request) }).await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.ports_ui.busy.remove(&busy_key);
                let error = match result {
                    Ok(DaemonResponse::PortMapping { mapping, snapshot }) => {
                        let current = snapshot
                            .mappings
                            .iter()
                            .find(|m| m.id == mapping.id)
                            .unwrap_or(&mapping);
                        let error = current
                            .error
                            .as_ref()
                            .map(|e| error_label(e.kind, app.language).to_owned());
                        app.window_manager.update(cx, |manager, cx| {
                            manager.merge_port_snapshot(snapshot);
                            cx.notify();
                        });
                        error
                    }
                    Err(_) | Ok(_) => Some(
                        if app.language == Language::SimplifiedChinese {
                            "SSH 连接不可用"
                        } else {
                            "SSH connection unavailable"
                        }
                        .into(),
                    ),
                };
                if creating
                    && app
                        .ports_ui
                        .editor
                        .as_ref()
                        .is_some_and(|e| Some(e.id) == editor_id)
                {
                    if let Some(error) = error {
                        let editor = app.ports_ui.editor.as_mut().unwrap();
                        editor.submitting = false;
                        editor.error = Some(error);
                        let input = if editor.kind == PortForwardKind::Dynamic {
                            &editor.local
                        } else {
                            &editor.remote
                        };
                        input.read(cx).focus_handle().focus(window, cx);
                    } else {
                        app.close_port_editor(window, cx);
                    }
                } else if let Some(error) = error {
                    app.ports_ui.notice = Some((connection_id, error));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn port_editor_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.right_tab != RightTab::Info {
            return false;
        }
        let Some(editor) = &self.ports_ui.editor else {
            return false;
        };
        let inputs = if editor.kind == PortForwardKind::Dynamic {
            vec![&editor.local]
        } else if editor.advanced {
            vec![&editor.remote, &editor.local, &editor.host]
        } else if editor.kind == PortForwardKind::Remote {
            vec![&editor.remote, &editor.local]
        } else {
            vec![&editor.remote]
        };
        let focused = inputs
            .iter()
            .position(|input| input.read(cx).is_focused(window));
        if focused.is_none() && !editor.focus.is_focused(window) {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_port_editor(window, cx),
            "tab" if !editor.submitting => {
                let current = focused.unwrap_or(0);
                let next = if event.keystroke.modifiers.shift {
                    (current + inputs.len() - 1) % inputs.len()
                } else {
                    (current + 1) % inputs.len()
                };
                inputs[next].read(cx).focus_handle().focus(window, cx);
            }
            _ => {}
        }
        true
    }
}
