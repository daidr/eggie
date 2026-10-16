use super::*;

/// Which theme picker a hover preview came from. Recorded alongside the previewed theme so the
/// preview resolves through the *same* layering as the real setting: previewing an interface theme
/// moves the terminal too when it follows the interface, and leaves it alone when it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewSlot {
    InterfaceDark,
    InterfaceLight,
    TerminalDark,
    TerminalLight,
    /// A profile's terminal override, carrying the profile being edited. The identity travels with the
    /// slot because the main window — which has to decide whether a given terminal is affected — cannot
    /// see the settings window's selection.
    ProfileTerminalDark(eggie_domain::ProfileId),
    ProfileTerminalLight(eggie_domain::ProfileId),
}

pub(crate) struct SettingsStore {
    preferences: AppSettings,
    config: AppSettings,
    lua_config: eggie_config::ConfigSnapshot,
    daemon: Option<eggie_ipc::DaemonClient>,
    preference_revision: u64,
    path: PathBuf,
    /// Transient theme override driven by the settings window's hover preview, tagged with the picker
    /// it came from. Never persisted and never serialized — it only overrides what the `resolved_*`
    /// methods return while the pointer rests on a theme option. Cleared when the pointer leaves, the
    /// dropdown closes, or the window closes.
    theme_preview: Option<(PreviewSlot, &'static TerminalTheme)>,
}

impl SettingsStore {
    pub(crate) fn daemon_client(&self) -> Option<eggie_ipc::DaemonClient> {
        self.daemon.clone()
    }
    pub(crate) fn load() -> Self {
        Self::load_from(eggie_storage::data_file_path("preferences.json"))
    }

    pub(crate) fn load_with_daemon(
        snapshot: eggie_config::ConfigSnapshot,
        preferences: eggie_config::PreferencesSnapshot,
        client: eggie_ipc::DaemonClient,
    ) -> Self {
        let mut store = Self::from_config(
            AppSettings::default(),
            eggie_storage::data_file_path("preferences.json"),
        );
        store.preferences = serde_json::from_value(preferences.value).unwrap_or_else(|error| {
            eprintln!("invalid daemon preferences: {error}");
            AppSettings::default()
        });
        store.preferences.normalize();
        store.preference_revision = preferences.revision;
        store.daemon = Some(client);
        store.apply_lua_snapshot(snapshot);
        store
    }

    fn load_from(path: PathBuf) -> Self {
        let mut config = eggie_storage::load_json::<AppSettings>(&path).unwrap_or_default();
        config.normalize();
        Self::from_config(config, path)
    }

    fn from_config(config: AppSettings, path: PathBuf) -> Self {
        let lua_config = eggie_config::ConfigSnapshot {
            revision: 0,
            status_revision: 0,
            path: eggie_storage::config_path(),
            exists: false,
            error: None,
            registered: Default::default(),
        };
        Self {
            preferences: config.clone(),
            config,
            lua_config,
            daemon: None,
            preference_revision: 0,
            path,
            theme_preview: None,
        }
    }

    pub(crate) fn config(&self) -> &AppSettings {
        &self.config
    }

    pub(crate) fn lua_config(&self) -> &eggie_config::ConfigSnapshot {
        &self.lua_config
    }

    pub(crate) fn set_lua_snapshot(
        &mut self,
        snapshot: eggie_config::ConfigSnapshot,
        cx: &mut Context<Self>,
    ) {
        if snapshot.status_revision <= self.lua_config.status_revision {
            return;
        }
        self.apply_lua_snapshot(snapshot);
        cx.notify();
    }

    fn apply_lua_snapshot(&mut self, snapshot: eggie_config::ConfigSnapshot) {
        self.lua_config = snapshot;
        self.config = self.preferences.clone();
        let registered = &self.lua_config.registered;
        if let Some(name) = &registered.ui_light_theme {
            self.config.interface_light_theme = name.clone();
        }
        if let Some(name) = &registered.ui_dark_theme {
            self.config.interface_dark_theme = name.clone();
        }
        if let Some(name) = &registered.terminal_light_theme {
            self.config.terminal_light_theme = name.clone();
            self.config.terminal_theme_source = TerminalThemeSource::Custom;
        }
        if let Some(name) = &registered.terminal_dark_theme {
            self.config.terminal_dark_theme = name.clone();
            self.config.terminal_theme_source = TerminalThemeSource::Custom;
        }
        if let Some(font) = &registered.terminal_font {
            self.config.font_family = font.family.clone();
            self.config.font_size = font.size;
        }
        self.config.normalize();
    }

    /// The interface theme actually in effect: the live hover preview if one is active on either
    /// interface slot, otherwise the persisted pair. Drives the app's chrome.
    pub(crate) fn resolved_interface_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        // A hover preview is authoritative: it applies regardless of which half is in effect, so
        // hovering the light slot while the app is dark previews the light slot.
        if let Some((slot, theme)) = self.theme_preview
            && matches!(
                slot,
                PreviewSlot::InterfaceDark | PreviewSlot::InterfaceLight
            )
        {
            return theme;
        }
        self.config.interface_theme(system_is_dark)
    }

    /// The global terminal theme actually in effect. Under
    /// [`TerminalThemeSource::FollowInterface`] this delegates to [`Self::resolved_interface_theme`] —
    /// that one line is what makes hovering an interface theme move the terminals along with the
    /// chrome, exactly as committing it would.
    pub(crate) fn resolved_terminal_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        match self.config.terminal_theme_source {
            TerminalThemeSource::FollowInterface => self.resolved_interface_theme(system_is_dark),
            TerminalThemeSource::Custom => {
                if let Some((slot, theme)) = self.theme_preview
                    && matches!(slot, PreviewSlot::TerminalDark | PreviewSlot::TerminalLight)
                {
                    return theme;
                }
                self.config.terminal_theme(system_is_dark)
            }
        }
    }

    /// The hover preview for `profile_id`'s terminal override, when one is active on the half currently
    /// in effect. Returns `None` for any other profile, so previewing one profile's theme never repaints
    /// terminals belonging to another.
    pub(crate) fn profile_theme_preview(
        &self,
        profile_id: eggie_domain::ProfileId,
    ) -> Option<&'static TerminalTheme> {
        // A hover preview applies regardless of which half is in effect, but only for the profile
        // being edited — previewing one profile's theme never repaints another's terminals.
        if let Some((slot, theme)) = self.theme_preview
            && match slot {
                PreviewSlot::ProfileTerminalDark(id) | PreviewSlot::ProfileTerminalLight(id) => {
                    id == profile_id
                }
                _ => false,
            }
        {
            return Some(theme);
        }
        None
    }

    /// Whether any hover preview is active, and on which slot. Feeds the main window's reconcile gate:
    /// a profile preview changes no global palette, so without this the reconcile would skip it.
    pub(crate) fn theme_preview_key(&self) -> Option<(PreviewSlot, usize)> {
        self.theme_preview
            .map(|(slot, theme)| (slot, theme as *const TerminalTheme as usize))
    }

    /// Set the transient hover preview to `theme` on `slot`. No-op (and no notify) if it is already the
    /// active preview, so a stream of repeated hover events over the same option doesn't churn renders.
    pub(crate) fn set_theme_preview(
        &mut self,
        slot: PreviewSlot,
        theme: &'static TerminalTheme,
        cx: &mut Context<Self>,
    ) {
        if self
            .theme_preview
            .is_some_and(|(active, current)| active == slot && std::ptr::eq(current, theme))
        {
            return;
        }
        self.theme_preview = Some((slot, theme));
        cx.notify();
    }

    /// Drop any active hover preview, restoring the persisted theme. No-op if none is active.
    pub(crate) fn clear_theme_preview(&mut self, cx: &mut Context<Self>) {
        if self.theme_preview.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn update(&mut self, update: impl FnOnce(&mut AppSettings), cx: &mut Context<Self>) {
        let previous = self.preferences.clone();
        update(&mut self.preferences);
        let managed = &self.lua_config.registered;
        if managed.ui_light_theme.is_some() {
            self.preferences.interface_light_theme = previous.interface_light_theme;
        }
        if managed.ui_dark_theme.is_some() {
            self.preferences.interface_dark_theme = previous.interface_dark_theme;
        }
        if managed.terminal_light_theme.is_some() {
            self.preferences.terminal_light_theme = previous.terminal_light_theme;
            self.preferences.terminal_theme_source = previous.terminal_theme_source;
        }
        if managed.terminal_dark_theme.is_some() {
            self.preferences.terminal_dark_theme = previous.terminal_dark_theme;
            self.preferences.terminal_theme_source = previous.terminal_theme_source;
        }
        if managed.terminal_font.is_some() {
            self.preferences.font_family = previous.font_family;
            self.preferences.font_size = previous.font_size;
        }
        self.preferences.normalize();
        self.apply_lua_snapshot(self.lua_config.clone());
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie settings: {error}");
        }
        cx.notify();
    }

    fn save(&mut self) -> anyhow::Result<()> {
        if let Some(client) = &self.daemon {
            let response = client.request(eggie_protocol::ClientRequest::SetPreferences {
                base_revision: self.preference_revision,
                value: serde_json::to_value(&self.preferences)?,
            })?;
            match response {
                eggie_protocol::DaemonResponse::Preferences { snapshot } => {
                    self.preference_revision = snapshot.revision;
                    Ok(())
                }
                eggie_protocol::DaemonResponse::Error { message } => anyhow::bail!("{message}"),
                response => anyhow::bail!("unexpected preferences response: {response:?}"),
            }
        } else {
            Ok(eggie_storage::save_json_atomic(
                &self.path,
                &self.preferences,
            )?)
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
