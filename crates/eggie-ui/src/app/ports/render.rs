use super::*;

pub(super) fn service_row(
    connection_id: &str,
    service: &eggie_protocol::RemoteService,
    colors: UiColors,
) -> Stateful<Div> {
    div()
        .id(format!(
            "ports-service-{connection_id}-{}-{}",
            service.address, service.port
        ))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .hover(move |row| row.bg(rgb(colors.hover)))
}

impl EggieApp {
    fn port_remember_indicator(&self, checked: bool) -> AnyElement {
        if checked {
            icon(IconName::CheckmarkCircle).into_any_element()
        } else {
            div()
                .size(px(14.))
                .flex_none()
                .rounded_full()
                .border_1()
                .border_color(rgb(self.colors.muted))
                .into_any_element()
        }
    }
    pub(super) fn port_text(&self, zh: &'static str, en: &'static str) -> &'static str {
        if self.language == Language::SimplifiedChinese {
            zh
        } else {
            en
        }
    }
    fn port_button(
        &self,
        id: String,
        label: &'static str,
        enabled: bool,
        primary: bool,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(26.))
            .px_2()
            .rounded_md()
            .text_size(px(11.))
            .bg(rgb(if primary {
                colors.accent
            } else {
                colors.panel_alt
            }))
            .text_color(rgb(if primary {
                colors.accent_foreground
            } else {
                colors.text
            }))
            .when(!enabled, |button| button.opacity(0.45))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(move |button| {
                        button.bg(rgb(if primary { colors.accent } else { colors.hover }))
                    })
                    .on_click(cx.listener(move |app, _, window, cx| action(app, window, cx)))
            })
            .child(label)
            .into_any_element()
    }
    pub(in crate::app) fn render_port_section(
        &self,
        connection: &PortMappingsSnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connection_id = connection.connection_id.clone();
        let create_busy = self
            .ports_ui
            .busy
            .contains(&format!("{connection_id}/create"));
        let id = connection_id.clone();
        let mut content = div().flex().flex_col().gap_2().min_w_0().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section_label(self.language.ports_label(), self.colors))
                .child(self.port_button(
                    "ports-add".into(),
                    self.port_text("添加端口", "Add port"),
                    connection.connected && !create_busy,
                    false,
                    move |app, window, cx| app.open_port_editor(id.clone(), window, cx),
                    cx,
                )),
        );
        if !connection.connected {
            content = content.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .child(self.port_text("SSH 连接已断开", "SSH disconnected")),
            );
        }
        if let Some((id, message)) = &self.ports_ui.notice
            && id == &connection_id
        {
            content = content.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.interface_theme.palette[1]))
                    .child(message.clone()),
            );
        }
        if let Some(editor) = &self.ports_ui.editor
            && editor.connection_id == connection_id
        {
            content = content.child(self.render_port_editor(editor, cx));
        }
        if !connection.mappings.is_empty() {
            content = content.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(self.colors.muted))
                    .child(self.port_text("映射", "Mappings")),
            );
            for mapping in &connection.mappings {
                content = content.child(self.render_mapping_card(connection, mapping, cx));
            }
        }
        let mut services: Vec<_> = connection
            .services
            .iter()
            .filter(|service| {
                !connection.mappings.iter().any(|m| {
                    m.request.kind == PortForwardKind::Local
                        && m.request.remote_port == Some(service.port)
                        && (m.request.target_host == service_target_host(&service.address)
                            || m.request.target_host == "localhost")
                })
            })
            .collect();
        services.sort_by_key(|service| (service.port, &service.address));
        let service_count = services.len();
        let expanded = self.ports_ui.services_expanded.contains(&connection_id);
        if !services.is_empty() {
            content = content.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(self.colors.muted))
                    .child(self.port_text("发现的服务", "Detected services")),
            );
            for service in services.into_iter().take(if expanded { usize::MAX } else { 3 }) {
                let target_host = service_target_host(&service.address).to_owned();
                let request = PortMappingRequest {
                    kind: PortForwardKind::Local,
                    local_port: None,
                    remote_port: Some(service.port),
                    target_host,
                    service: PortServiceKind::Auto,
                    remember: false,
                };
                let id = connection_id.clone();
                let port = service.port;
                let key = format!("service-{}-{port}", service.address);
                let busy = self
                    .ports_ui
                    .busy
                    .contains(&format!("{connection_id}/{key}"));
                content = content.child(
                    service_row(&connection_id, service, self.colors)
                        .child(div().id(format!("ports-service-label-{}-{port}", service.address))
                            .flex().items_center().flex_1().min_w_0().gap_2()
                            .tooltip(tooltip_text(format!("{} · {}:{}", service.process, service.address, service.port), self.colors))
                            .child(div().flex_none().child(format!("{port}")))
                            .child(div().truncate().text_size(px(11.)).text_color(rgb(self.colors.muted)).child(service.process.clone())))
                        .child(self.port_button(
                            format!("ports-connect-service-{}-{port}", service.address),
                            if busy {
                                self.port_text("转发中", "Forwarding")
                            } else {
                                self.port_text("转发", "Forward")
                            },
                            connection.connected && !busy,
                            false,
                            move |app, _, cx| {
                                app.run_port_operation(
                                    id.clone(),
                                    key.clone(),
                                    ClientRequest::CreatePortMapping {
                                        connection_id: id.clone(),
                                        request: request.clone(),
                                    },
                                    false,
                                    cx,
                                )
                            },
                            cx,
                        )),
                );
            }
            if service_count > 3 {
                let id = connection_id.clone();
                let label = if expanded { self.port_text("收起", "Show less").to_owned() }
                    else { format!("{} {}", self.port_text("更多服务", "More services"), service_count - 3) };
                content = content.child(div().id(format!("ports-more-services-{id}"))
                    .px_2().py_1().text_size(px(11.)).text_color(rgb(self.colors.muted)).cursor_pointer()
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if !app.ports_ui.services_expanded.remove(&id) { app.ports_ui.services_expanded.insert(id.clone()); }
                        cx.notify();
                    })).child(label));
            }
        } else if connection.mappings.is_empty() {
            content = content.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .child(if connection.discovery_error.is_some() {
                        self.port_text(
                            "自动发现不可用，可手动添加",
                            "Discovery unavailable · add a port manually",
                        )
                    } else if connection.connected {
                        self.port_text("未检测到服务", "No services detected")
                    } else {
                        self.port_text(
                            "SSH 就绪后可转发端口",
                            "Services are available once SSH connects",
                        )
                    }),
            );
        }
        content.into_any_element()
    }
    fn render_port_input(&self, label: &'static str, input: &Entity<TextInput>) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(self.colors.muted))
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .h(px(30.))
                    .px_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.background))
                    .child(input.clone()),
            )
            .into_any_element()
    }
    fn render_port_editor(&self, editor: &PortEditor, cx: &mut Context<Self>) -> AnyElement {
        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .track_focus(&editor.focus);
        if editor.advanced || editor.kind != PortForwardKind::Local {
            let mut modes = div().flex().flex_wrap().gap_1();
            for (kind, zh, en) in [
                (PortForwardKind::Local, "访问远端", "Remote service"),
                (PortForwardKind::Remote, "反向映射", "Reverse"),
                (PortForwardKind::Dynamic, "SOCKS 代理", "SOCKS"),
            ] {
                modes = modes.child(self.port_button(
                    format!("ports-kind-{kind:?}"),
                    self.port_text(zh, en),
                    !editor.submitting,
                    editor.kind == kind,
                    move |app, window, cx| {
                        if let Some(editor) = &mut app.ports_ui.editor {
                            editor.kind = kind;
                            editor.error = None;
                            let input = if kind == PortForwardKind::Dynamic {
                                &editor.local
                            } else {
                                &editor.remote
                            };
                            input.read(cx).focus_handle().focus(window, cx);
                        }
                        cx.notify();
                    },
                    cx,
                ));
            }
            card = card.child(modes);
        }
        if editor.kind != PortForwardKind::Dynamic {
            card = card.child(self.render_port_input(
                if editor.kind == PortForwardKind::Remote {
                    self.port_text("远端端口（自动）", "Remote port (auto)")
                } else {
                    self.port_text("远端端口", "Remote port")
                },
                &editor.remote,
            ));
        }
        if editor.advanced || editor.kind != PortForwardKind::Local {
            card = card.child(self.render_port_input(
                if editor.kind == PortForwardKind::Remote {
                    self.port_text("本机服务端口", "Local service port")
                } else {
                    self.port_text("本机端口", "Local port")
                },
                &editor.local,
            ));
        }
        if editor.advanced {
            if editor.kind != PortForwardKind::Dynamic {
                card = card.child(
                    self.render_port_input(self.port_text("目标地址", "Target host"), &editor.host),
                );
            }
            if editor.kind == PortForwardKind::Local {
                let mut services = div().flex().flex_wrap().gap_1();
                for (service, label) in [
                    (PortServiceKind::Auto, self.port_text("自动", "Auto")),
                    (PortServiceKind::Http, "HTTP"),
                    (PortServiceKind::Https, "HTTPS"),
                    (PortServiceKind::Tcp, "TCP"),
                ] {
                    services = services.child(self.port_button(
                        format!("ports-service-{service:?}"),
                        label,
                        !editor.submitting,
                        editor.service == service,
                        move |app, _, cx| {
                            if let Some(editor) = &mut app.ports_ui.editor {
                                editor.service = service;
                            }
                            cx.notify();
                        },
                        cx,
                    ));
                }
                card = card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(self.colors.muted))
                                .child(self.port_text("服务类型", "Service type")),
                        )
                        .child(services),
                );
            }
        }
        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .id("ports-editor-remember")
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(rgb(if editor.remember {
                            self.colors.accent
                        } else {
                            self.colors.muted
                        }))
                        .child(self.port_remember_indicator(editor.remember))
                        .child(self.port_text("记住映射", "Remember"))
                        .when(!editor.submitting, |row| {
                            row.on_click(cx.listener(|app, _, _, cx| {
                                if let Some(editor) = &mut app.ports_ui.editor {
                                    editor.remember = !editor.remember;
                                }
                                cx.notify();
                            }))
                        }),
                )
                .child(self.port_button(
                    "ports-editor-advanced".into(),
                    if editor.advanced {
                        self.port_text("收起", "Less")
                    } else {
                        self.port_text("高级设置", "Advanced")
                    },
                    !editor.submitting,
                    false,
                    |app, window, cx| {
                        if let Some(editor) = &mut app.ports_ui.editor {
                            editor.advanced = !editor.advanced;
                            let input = if editor.kind == PortForwardKind::Dynamic {
                                &editor.local
                            } else {
                                &editor.remote
                            };
                            input.read(cx).focus_handle().focus(window, cx);
                        }
                        cx.notify();
                    },
                    cx,
                )),
        );
        if let Some(error) = &editor.error {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.interface_theme.palette[1]))
                    .child(error.clone()),
            );
        }
        card.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(self.port_button(
                    "ports-editor-cancel".into(),
                    self.language.cancel(),
                    true,
                    false,
                    |app, window, cx| app.close_port_editor(window, cx),
                    cx,
                ))
                .child(self.port_button(
                    "ports-editor-connect".into(),
                    if editor.submitting {
                        self.port_text("转发中", "Forwarding")
                    } else {
                        self.port_text("转发", "Forward")
                    },
                    !editor.submitting,
                    true,
                    |app, window, cx| app.submit_port_editor(window, cx),
                    cx,
                )),
        )
        .into_any_element()
    }
    fn render_mapping_card(
        &self,
        connection: &PortMappingsSnapshot,
        mapping: &PortMapping,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let connection_id = connection.connection_id.clone();
        let id = mapping.id.clone();
        let busy = self
            .ports_ui
            .busy
            .contains(&format!("{connection_id}/{id}"));
        let label = match mapping.request.kind {
            PortForwardKind::Local => format!(
                "{}:{}",
                mapping.request.target_host,
                mapping.remote_port.unwrap_or_default()
            ),
            PortForwardKind::Remote => format!(
                "{} :{}",
                self.port_text("远端", "Remote"),
                mapping
                    .remote_port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| self.port_text("自动", "Auto").into())
            ),
            PortForwardKind::Dynamic => "SOCKS".to_owned(),
        };
        let (state, tint) = match mapping.state {
            PortMappingState::Connected => (
                self.port_text("转发中", "Forwarding"),
                self.interface_theme.palette[2],
            ),
            PortMappingState::Connecting => {
                (self.port_text("转发中", "Forwarding"), self.colors.accent)
            }
            PortMappingState::Stopping => (self.port_text("停止中", "Stopping"), self.colors.muted),
            PortMappingState::Stopped => (self.port_text("已停止", "Stopped"), self.colors.muted),
            PortMappingState::Disconnected => {
                (self.port_text("已断开", "Disconnected"), self.colors.muted)
            }
            PortMappingState::Failed => (
                self.port_text("失败", "Failed"),
                self.interface_theme.palette[1],
            ),
        };
        let address = if mapping.request.kind == PortForwardKind::Remote {
            format!(
                "{}:{}",
                mapping.request.target_host,
                mapping.local_port.unwrap_or_default()
            )
        } else {
            mapping
                .local_address()
                .unwrap_or_else(|| self.port_text("自动", "Auto").into())
        };
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(self.render_terminal_status_tag(state, tint)),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .child(format!("{} {}", self.port_text("本机", "Local"), address)),
            );
        if let Some(error) = &mapping.error {
            let target = id.clone();
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id(format!("ports-error-{id}"))
                            .text_size(px(11.))
                            .text_color(rgb(self.interface_theme.palette[1]))
                            .cursor_pointer()
                            .child(error_label(error.kind, self.language))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if !app.ports_ui.details.remove(&target) {
                                    app.ports_ui.details.insert(target.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(self.ports_ui.details.contains(&id), |box_| {
                        box_.child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(self.colors.muted))
                                .child(error.detail.clone()),
                        )
                    }),
            );
        }
        let mut actions = div().flex().flex_wrap().items_center().gap_1();
        if let Some(url) = mapping.browser_url() {
            actions = actions.child(self.port_button(
                format!("ports-open-{id}"),
                self.port_text("打开", "Open"),
                !busy,
                false,
                move |_, _, cx| cx.open_url(&url),
                cx,
            ));
        }
        if mapping.state == PortMappingState::Connected {
            let copy = if mapping.request.kind == PortForwardKind::Remote {
                format!("127.0.0.1:{}", mapping.remote_port.unwrap_or_default())
            } else {
                mapping.browser_url().unwrap_or(address)
            };
            actions = actions.child(self.port_button(
                format!("ports-copy-{id}"),
                self.port_text("复制", "Copy"),
                !busy,
                false,
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())),
                cx,
            ));
            let conn = connection_id.clone();
            let target = id.clone();
            actions = actions.child(self.port_button(
                format!("ports-stop-{id}"),
                self.port_text("停止", "Stop"),
                !busy,
                false,
                move |app, _, cx| {
                    app.run_port_operation(
                        conn.clone(),
                        target.clone(),
                        ClientRequest::StopPortMapping {
                            connection_id: conn.clone(),
                            id: target.clone(),
                            remove: false,
                        },
                        false,
                        cx,
                    )
                },
                cx,
            ));
        } else if !matches!(
            mapping.state,
            PortMappingState::Connecting | PortMappingState::Stopping
        ) {
            let conn = connection_id.clone();
            let target = id.clone();
            actions = actions.child(self.port_button(
                format!("ports-retry-{id}"),
                self.port_text("转发", "Forward"),
                connection.connected && !busy,
                true,
                move |app, _, cx| {
                    app.run_port_operation(
                        conn.clone(),
                        target.clone(),
                        ClientRequest::StartPortMapping {
                            connection_id: conn.clone(),
                            id: target.clone(),
                        },
                        false,
                        cx,
                    )
                },
                cx,
            ));
            let conn = connection_id.clone();
            let target = id.clone();
            actions = actions.child(self.port_button(
                format!("ports-remove-{id}"),
                self.port_text("移除", "Remove"),
                !busy,
                false,
                move |app, _, cx| {
                    app.run_port_operation(
                        conn.clone(),
                        target.clone(),
                        ClientRequest::StopPortMapping {
                            connection_id: conn.clone(),
                            id: target.clone(),
                            remove: true,
                        },
                        false,
                        cx,
                    )
                },
                cx,
            ));
        }
        let conn = connection_id;
        let target = id.clone();
        let remember = !mapping.request.remember;
        card.child(actions)
            .child(
                div()
                    .id(format!("ports-remember-{id}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(10.))
                    .text_color(rgb(if mapping.request.remember {
                        self.colors.accent
                    } else {
                        self.colors.muted
                    }))
                    .child(self.port_remember_indicator(mapping.request.remember))
                    .child(self.port_text("记住映射", "Remember"))
                    .when(!busy, |row| {
                        row.cursor_pointer()
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.run_port_operation(
                                    conn.clone(),
                                    target.clone(),
                                    ClientRequest::RememberPortMapping {
                                        connection_id: conn.clone(),
                                        id: target.clone(),
                                        remember,
                                    },
                                    false,
                                    cx,
                                )
                            }))
                    }),
            )
            .into_any_element()
    }
}
