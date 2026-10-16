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
    fn inspect_ssh_config(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.settings.read(cx).daemon_client() else {
            return;
        };
        let id = self.selected_profile_id(cx);
        let Some(target) = self
            .profile_store
            .read(cx)
            .profile(id)
            .and_then(|p| p.remote.clone())
        else {
            return;
        };
        if !target.is_usable() {
            return;
        }
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
        let mut content = div().flex().flex_col().gap_2();
        let busy = self.ssh_config_task.is_some();
        content = content.child(
            div()
                .id("inspect-ssh-config")
                .rounded_md()
                .px_3()
                .py_2()
                .bg(rgb(self.colors.panel_alt))
                .when(!busy, |button| {
                    button
                        .cursor_pointer()
                        .hover(|button| button.bg(rgb(self.colors.border)))
                        .on_click(cx.listener(|settings, _, _, cx| settings.inspect_ssh_config(cx)))
                })
                .child(if busy {
                    if zh { "读取中" } else { "Reading" }
                } else if zh {
                    "查看连接配置"
                } else {
                    "Inspect connection config"
                }),
        );
        if let Some((id, target, result)) = &self.ssh_config_result
            && *id == profile.id
            && profile.remote.as_ref() == Some(target)
        {
            match result {
                Ok(config) => {
                    let mut details = vec![format!(
                        "{}@{}:{}",
                        config.user, config.hostname, config.port
                    )];
                    if let Some(jump) = &config.proxy_jump {
                        details.push(format!(
                            "{}: {jump}",
                            if zh { "跳板机" } else { "Jump host" }
                        ));
                    }
                    if config.uses_proxy_command {
                        details.push(
                            if zh {
                                "使用 ProxyCommand"
                            } else {
                                "Uses ProxyCommand"
                            }
                            .into(),
                        );
                    }
                    for identity in &config.identity_files {
                        details.push(identity.clone());
                    }
                    content = content.child(
                        div()
                            .max_w(px(300.))
                            .text_size(px(11.))
                            .text_color(rgb(self.colors.muted))
                            .child(details.join("\n")),
                    );
                }
                Err(error) => {
                    content = content.child(
                        div()
                            .max_w(px(300.))
                            .text_size(px(11.))
                            .text_color(rgb(WARNING_TEXT))
                            .child(error.clone()),
                    )
                }
            }
        }
        self.setting_row(
            if zh { "SSH 配置" } else { "SSH config" },
            None,
            content.into_any_element(),
            self.colors,
        )
    }
}
