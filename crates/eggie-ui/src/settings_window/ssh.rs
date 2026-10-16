use super::*;
use eggie_protocol::{ClientRequest, DaemonResponse};
use std::time::Duration;

impl SettingsWindow {
    pub(super) fn load_ssh_hosts(&mut self, cx: &mut Context<Self>) {
        if self.ssh_hosts_task.is_some() {
            return;
        }
        let Some(client) = self.settings.read(cx).daemon_client() else {
            return;
        };
        self.ssh_hosts_error = None;
        let executor = cx.background_executor().clone();
        self.ssh_hosts_task = Some(cx.spawn(async move |this, cx| {
            let result = executor
                .spawn(async move {
                    client.request_with_timeout(ClientRequest::ListSshHosts, Duration::from_secs(5))
                })
                .await;
            let _ = this.update(cx, |settings, cx| {
                settings.ssh_hosts_task = None;
                match result {
                    Ok(DaemonResponse::SshHosts { hosts }) => settings.ssh_host_names = hosts,
                    Err(error) => settings.ssh_hosts_error = Some(error.to_string()),
                    _ => settings.ssh_hosts_error = Some("SSH config is unavailable".into()),
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn handle_ssh_host_key(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.key != "down"
            || key.modifiers.platform
            || key.modifiers.control
            || key.modifiers.shift
            || !self
                .profile_inputs
                .remote_host
                .read(cx)
                .focus_handle()
                .is_focused(window)
        {
            return false;
        }
        self.toggle_selector(SelectorKind::SshHost, window, cx);
        true
    }

    pub(super) fn reset_ssh_config(&mut self) {
        self.ssh_config_expanded = false;
        self.ssh_config_task = None;
        self.ssh_config_result = None;
    }

    fn ssh_config_target(
        &self,
        cx: &App,
    ) -> Option<(eggie_domain::ProfileId, eggie_domain::RemoteTarget)> {
        let id = self.selected_profile_id(cx);
        let store = self.profile_store.read(cx);
        if store.launch_error(id).is_some() {
            return None;
        }
        let target = store.profile(id)?.remote.clone()?;
        target.is_usable().then_some((id, target))
    }

    fn toggle_ssh_config(&mut self, cx: &mut Context<Self>) {
        if self.ssh_config_expanded {
            self.reset_ssh_config();
        } else if self.ssh_config_target(cx).is_some() {
            self.ssh_config_expanded = true;
            self.inspect_ssh_config(cx);
        }
        cx.notify();
    }

    fn inspect_ssh_config(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.settings.read(cx).daemon_client() else {
            return;
        };
        let Some((id, target)) = self.ssh_config_target(cx) else {
            return;
        };
        let request = ClientRequest::ResolveSshConfig {
            target: target.clone(),
        };
        self.ssh_config_result = None;
        let executor = cx.background_executor().clone();
        self.ssh_config_task = Some(cx.spawn(async move |this, cx| {
            let result = executor
                .spawn(async move { client.request_with_timeout(request, Duration::from_secs(5)) })
                .await;
            let _ = this.update(cx, |settings, cx| {
                // A closed disclosure or an edited target cannot publish an obsolete result.
                if !settings.ssh_config_expanded
                    || settings.ssh_config_target(cx).as_ref() != Some(&(id, target.clone()))
                {
                    return;
                }
                settings.ssh_config_task = None;
                let result = match result {
                    Ok(DaemonResponse::SshEffectiveConfig { config }) => Ok(config),
                    Err(error) => Err(error.to_string()),
                    _ => Err("SSH config is unavailable".into()),
                };
                settings.ssh_config_result = Some((id, target, result));
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_ssh_config(
        &self,
        language: Language,
        profile: &eggie_domain::Profile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zh = language == Language::SimplifiedChinese;
        let available = self.ssh_config_target(cx).is_some()
            && self.settings.read(cx).daemon_client().is_some();
        let expanded = available && self.ssh_config_expanded;
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .min_h(px(48.))
            .px_4()
            .child(
                div()
                    .id("ssh-connection-details")
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .py_3()
                    .text_color(rgb(if available {
                        self.colors.text
                    } else {
                        self.colors.muted
                    }))
                    .when(available, |button| {
                        button
                            .cursor_pointer()
                            .focusable()
                            .tab_index(0)
                            .hover(|button| button.text_color(rgb(self.colors.accent)))
                            .focus(|button| button.text_color(rgb(self.colors.accent)))
                            .on_click(
                                cx.listener(|settings, _, _, cx| settings.toggle_ssh_config(cx)),
                            )
                            .on_key_down(cx.listener(
                                |settings, event: &gpui::KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        settings.toggle_ssh_config(cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                    })
                    .child(icon(if expanded {
                        IconName::ArrowDown
                    } else {
                        IconName::ArrowRight
                    }))
                    .child(if zh {
                        "连接详情"
                    } else {
                        "Connection details"
                    }),
            )
            .when(expanded, |header| {
                header.child(
                    div()
                        .id("ssh-config-refresh")
                        .flex_none()
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .text_size(px(11.))
                        .text_color(rgb(self.colors.muted))
                        .when(self.ssh_config_task.is_none(), |button| {
                            button
                                .cursor_pointer()
                                .hover(|button| button.text_color(rgb(self.colors.accent)))
                                .on_click(
                                    cx.listener(|settings, _, _, cx| {
                                        settings.inspect_ssh_config(cx)
                                    }),
                                )
                        })
                        .child(if self.ssh_config_task.is_some() {
                            if zh { "读取中" } else { "Loading" }
                        } else if zh {
                            "刷新"
                        } else {
                            "Refresh"
                        }),
                )
            });
        let mut content = div().flex().flex_col().w_full().child(header);
        if expanded {
            let mut details = div()
                .flex()
                .flex_col()
                .gap_3()
                .mx_4()
                .mb_4()
                .p_3()
                .rounded_lg()
                .bg(rgb(self.colors.panel_alt))
                .text_size(px(12.));
            if let Some((id, target, result)) = &self.ssh_config_result
                && *id == profile.id
                && profile.remote.as_ref() == Some(target)
            {
                match result {
                    Ok(config) => {
                        let rows = ssh_detail_rows(config, language);
                        let mut identity = div().flex().items_start().gap_4();
                        for (index, (label, value)) in rows.iter().take(3).enumerate() {
                            identity = identity.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .min_w_0()
                                    .when(index < 2, |field| field.flex_1())
                                    .when(index == 2, |field| field.w(px(64.)).flex_none())
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(self.colors.muted))
                                            .child(*label),
                                    )
                                    .child(value.clone()),
                            );
                        }
                        details = details.child(identity);
                        if rows.len() > 3 {
                            let mut routes = div()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .border_t_1()
                                .border_color(rgb(self.colors.border))
                                .pt_3();
                            for (label, value) in rows.into_iter().skip(3) {
                                routes = routes.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(self.colors.muted))
                                                .child(label),
                                        )
                                        .child(value),
                                );
                            }
                            details = details.child(routes);
                        }
                    }
                    Err(error) => {
                        details =
                            details.child(div().text_color(rgb(WARNING_TEXT)).child(error.clone()));
                    }
                }
            } else {
                details = details.child(div().text_color(rgb(self.colors.muted)).child(if zh {
                    "正在读取 SSH 配置"
                } else {
                    "Reading SSH config"
                }));
            }
            content = content.child(details);
        }
        content.into_any_element()
    }
}

fn ssh_detail_rows(
    config: &eggie_protocol::SshEffectiveConfig,
    language: Language,
) -> Vec<(&'static str, String)> {
    let zh = language == Language::SimplifiedChinese;
    let mut rows = vec![
        (if zh { "主机" } else { "Host" }, config.hostname.clone()),
        (if zh { "用户" } else { "User" }, config.user.clone()),
        (if zh { "端口" } else { "Port" }, config.port.to_string()),
    ];
    if !config.identity_files.is_empty() {
        rows.push((
            if zh { "密钥" } else { "Identity files" },
            config.identity_files.join("\n"),
        ));
    }
    if let Some(jump) = &config.proxy_jump {
        rows.push((if zh { "跳板机" } else { "Jump host" }, jump.clone()));
    }
    if config.uses_proxy_command {
        rows.push((
            if zh { "代理命令" } else { "Proxy command" },
            if zh { "已配置" } else { "Configured" }.into(),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_details_label_fields_and_omit_unused_routes() {
        let mut config = eggie_protocol::SshEffectiveConfig {
            hostname: "dev.example".into(),
            user: "dev".into(),
            port: 2222,
            identity_files: vec![],
            proxy_jump: None,
            uses_proxy_command: false,
        };
        assert_eq!(
            ssh_detail_rows(&config, Language::SimplifiedChinese),
            [
                ("主机", "dev.example".into()),
                ("用户", "dev".into()),
                ("端口", "2222".into())
            ]
        );
        config.identity_files = vec!["~/.ssh/first".into(), "~/.ssh/second".into()];
        config.proxy_jump = Some("bastion".into());
        config.uses_proxy_command = true;
        let rows = ssh_detail_rows(&config, Language::English);
        assert_eq!(
            rows[3],
            ("Identity files", "~/.ssh/first\n~/.ssh/second".into())
        );
        assert_eq!(rows[4], ("Jump host", "bastion".into()));
        assert_eq!(rows[5], ("Proxy command", "Configured".into()));
    }
}
