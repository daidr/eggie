
use super::*;
use std::fs;

#[test]
fn theme_by_name_resolves_across_dark_and_light_lists() {
    let catalog = theme_catalog();
    // A dark theme and a light theme both resolve through the unified lookup the hover preview
    // uses, and an unknown name yields None (so preview_theme becomes a no-op).
    assert_eq!(
        catalog
            .theme_by_name("Catppuccin Mocha")
            .map(|t| t.name.as_str()),
        Some("Catppuccin Mocha")
    );
    assert_eq!(
        catalog.theme_by_name("Ayu Light").map(|t| t.name.as_str()),
        Some("Ayu Light")
    );
    assert!(catalog.theme_by_name("No Such Theme").is_none());
}

/// A store with no preview active, for the preview tests.
fn store_with(config: AppSettings) -> SettingsStore {
    SettingsStore::from_config(config, std::path::PathBuf::from("unused"))
}

#[test]
fn a_preview_overrides_the_persisted_pair_regardless_of_the_appearance() {
    let catalog = theme_catalog();
    let mut store = store_with(AppSettings {
        theme_mode: ThemeMode::Dark,
        interface_dark_theme: "Builtin Dark".to_owned(),
        ..AppSettings::default()
    });

    // No preview active: resolves to the persisted pair.
    assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");

    let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
    store.theme_preview = Some((PreviewSlot::InterfaceDark, preview));
    assert_eq!(
        store.resolved_interface_theme(true).name,
        "Catppuccin Mocha"
    );

    // The preview is authoritative: it applies even when the system is light, so the dark-slot
    // preview stays visible under a light appearance. Only clearing it falls back to the persisted
    // pair, which then resolves light.
    store.config.theme_mode = ThemeMode::System;
    assert_eq!(
        store.resolved_interface_theme(false).name,
        "Catppuccin Mocha",
        "a preview applies regardless of the current appearance"
    );

    // Clearing restores the persisted pair, now resolving light.
    store.theme_preview = None;
    assert_eq!(
        store.resolved_interface_theme(false).name,
        AppSettings::default().interface_light_theme
    );
}

#[test]
fn a_light_slot_preview_applies_while_the_app_is_dark() {
    let catalog = theme_catalog();
    let mut store = store_with(AppSettings {
        theme_mode: ThemeMode::Dark,
        interface_dark_theme: "Builtin Dark".to_owned(),
        ..AppSettings::default()
    });

    // Hovering the light slot while the app is dark must preview the light slot. Before the fix,
    // the preview was filtered out because the light slot's half did not match the dark mode.
    let preview = catalog.theme_by_name("Catppuccin Latte").unwrap();
    store.theme_preview = Some((PreviewSlot::InterfaceLight, preview));
    assert_eq!(
        store.resolved_interface_theme(true).name,
        "Catppuccin Latte",
        "a light-slot preview applies regardless of the current dark appearance"
    );
}

#[test]
fn previewing_an_interface_theme_moves_the_terminal_only_when_it_follows() {
    let catalog = theme_catalog();
    let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
    let mut store = store_with(AppSettings {
        theme_mode: ThemeMode::Dark,
        interface_dark_theme: "Builtin Dark".to_owned(),
        ..AppSettings::default()
    });
    store.theme_preview = Some((PreviewSlot::InterfaceDark, preview));

    // Following the interface: the terminal rides along with the hover, exactly as it would once
    // the choice is committed.
    assert_eq!(
        store.config.terminal_theme_source,
        TerminalThemeSource::FollowInterface
    );
    assert_eq!(store.resolved_terminal_theme(true).name, "Catppuccin Mocha");

    // With its own pair, the terminal ignores an interface preview.
    store.config.use_custom_terminal_themes();
    store.config.terminal_dark_theme = "Builtin Dark".to_owned();
    assert_eq!(
        store.resolved_interface_theme(true).name,
        "Catppuccin Mocha"
    );
    assert_eq!(
        store.resolved_terminal_theme(true).name,
        "Builtin Dark",
        "a custom terminal pair must not follow an interface preview"
    );

    // And a terminal preview moves only the terminal, never the chrome.
    store.theme_preview = Some((PreviewSlot::TerminalDark, preview));
    assert_eq!(store.resolved_terminal_theme(true).name, "Catppuccin Mocha");
    assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");
}

#[test]
fn a_profile_preview_is_scoped_to_the_profile_being_edited() {
    let catalog = theme_catalog();
    let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
    let edited = uuid::Uuid::new_v4();
    let other = uuid::Uuid::new_v4();
    let mut store = store_with(AppSettings {
        theme_mode: ThemeMode::Dark,
        ..AppSettings::default()
    });
    store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(edited), preview));

    assert_eq!(
        store
            .profile_theme_preview(edited)
            .map(|theme| theme.name.as_str()),
        Some("Catppuccin Mocha")
    );
    assert!(
        store.profile_theme_preview(other).is_none(),
        "previewing one profile's theme must not repaint another profile's terminals"
    );
    // A profile preview is also not a global one.
    assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");
    assert_eq!(store.resolved_terminal_theme(true).name, "Builtin Dark");
}

#[test]
fn the_terminal_layer_equals_the_interface_layer_while_it_follows() {
    // The default (and pre-split) behaviour: one theme drives both layers, in every combination of
    // theme_mode and system appearance.
    let config = AppSettings {
        interface_dark_theme: "Catppuccin Mocha".to_owned(),
        interface_light_theme: "Ayu Light".to_owned(),
        ..AppSettings::default()
    };
    assert_eq!(
        config.terminal_theme_source,
        TerminalThemeSource::FollowInterface
    );
    for mode in ThemeMode::ALL {
        for system_is_dark in [true, false] {
            let config = AppSettings {
                theme_mode: mode,
                ..config.clone()
            };
            assert!(
                std::ptr::eq(
                    config.interface_theme(system_is_dark),
                    config.terminal_theme(system_is_dark)
                ),
                "{mode:?} / system_is_dark={system_is_dark}: the terminal must follow the interface"
            );
        }
    }
}

#[test]
fn a_custom_terminal_pair_is_independent_of_the_interface_pair() {
    // The point of the split: a dark interface with a light terminal, and vice versa.
    let config = AppSettings {
        theme_mode: ThemeMode::Dark,
        interface_dark_theme: "Catppuccin Mocha".to_owned(),
        terminal_theme_source: TerminalThemeSource::Custom,
        terminal_dark_theme: "Builtin Dark".to_owned(),
        ..AppSettings::default()
    };
    assert_eq!(config.interface_theme(true).name, "Catppuccin Mocha");
    assert_eq!(config.terminal_theme(true).name, "Builtin Dark");

    // Each layer picks its own half with the same dark/light decision.
    let config = AppSettings {
        theme_mode: ThemeMode::Light,
        interface_light_theme: "Ayu Light".to_owned(),
        terminal_light_theme: "Builtin Light".to_owned(),
        ..config
    };
    assert_eq!(config.interface_theme(true).name, "Ayu Light");
    assert_eq!(config.terminal_theme(true).name, "Builtin Light");
}

#[test]
fn opting_into_a_custom_terminal_pair_changes_nothing_on_screen() {
    // Seeding from the interface pair is what keeps the toggle from reading as a bug: without it,
    // switching to Custom would jump the terminal to the built-in defaults.
    let mut config = AppSettings {
        interface_dark_theme: "Catppuccin Mocha".to_owned(),
        interface_light_theme: "Ayu Light".to_owned(),
        ..AppSettings::default()
    };
    let before = [config.terminal_theme(true), config.terminal_theme(false)];
    config.use_custom_terminal_themes();
    assert_eq!(config.terminal_theme_source, TerminalThemeSource::Custom);
    for (index, system_is_dark) in [true, false].into_iter().enumerate() {
        assert!(
            std::ptr::eq(config.terminal_theme(system_is_dark), before[index]),
            "system_is_dark={system_is_dark}: the toggle must be visually inert"
        );
    }
}

#[test]
fn a_light_theme_cannot_be_persisted_into_a_dark_slot() {
    // Each slot is validated against its own half of the catalog, which is what lets each picker
    // offer one half rather than all of it.
    let mut config = AppSettings {
        interface_dark_theme: "Ayu Light".to_owned(),
        terminal_dark_theme: "Ayu Light".to_owned(),
        interface_light_theme: "Catppuccin Mocha".to_owned(),
        terminal_light_theme: "Catppuccin Mocha".to_owned(),
        ..AppSettings::default()
    };
    config.normalize();
    assert_eq!(config.interface_dark_theme, DEFAULT_DARK_THEME);
    assert_eq!(config.terminal_dark_theme, DEFAULT_DARK_THEME);
    assert_eq!(config.interface_light_theme, DEFAULT_LIGHT_THEME);
    assert_eq!(config.terminal_light_theme, DEFAULT_LIGHT_THEME);
}

#[test]
fn the_global_palette_alone_cannot_detect_an_appearance_flip() {
    // This is why the main window's reconcile gate carries `use_dark` as an input of its own.
    //
    // Pin the global terminal theme so a system flip leaves it byte-identical (a light/dark pair
    // naming the same theme does this; so does `theme_mode` pinned to Dark or Light). A profile with
    // a per-half override still has to re-resolve, so a gate keyed only on the global palette would
    // skip the reconcile and leave that terminal painting the wrong half.
    let config = AppSettings {
        terminal_theme_source: TerminalThemeSource::Custom,
        terminal_dark_theme: DEFAULT_DARK_THEME.to_owned(),
        terminal_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
        theme_mode: ThemeMode::Dark,
        ..AppSettings::default()
    };
    // `theme_mode: Dark` pins the global layer: both system appearances resolve to the same theme.
    let pinned = config.terminal_theme(true);
    assert!(
        std::ptr::eq(pinned, config.terminal_theme(false)),
        "the global palette must be unchanged across the flip for this test to mean anything"
    );

    let dark_name = theme_catalog().dark_names().into_iter().next().unwrap();
    let light_name = theme_catalog().light_names().into_iter().next().unwrap();
    let appearance = eggie_domain::ProfileAppearance {
        terminal_dark_theme: Some(dark_name.clone()),
        terminal_light_theme: Some(light_name.clone()),
        ..Default::default()
    };

    // Same global layer, same profile, different half in effect → different resolved palette.
    let resolved_dark = resolve_appearance(
        &config,
        Some(&appearance),
        GlobalTerminalLayer {
            theme: pinned,
            use_dark: true,
        },
        None,
    );
    let resolved_light = resolve_appearance(
        &config,
        Some(&appearance),
        GlobalTerminalLayer {
            theme: pinned,
            use_dark: false,
        },
        None,
    );
    assert_eq!(resolved_dark.theme.name, dark_name);
    assert_eq!(resolved_light.theme.name, light_name);
    assert_ne!(
        resolved_dark.theme.appearance(),
        resolved_light.theme.appearance(),
        "the flip changes what the daemon must be told, while the global palette says nothing"
    );
}

#[test]
fn a_preview_key_distinguishes_slots_and_themes() {
    // The gate's other new input. A profile preview changes no global palette, so without a key the
    // reconcile would skip the repaint the preview exists to show.
    let catalog = theme_catalog();
    let first = catalog.theme_by_name("Catppuccin Mocha").unwrap();
    let second = catalog.theme_by_name("Builtin Dark").unwrap();
    let profile = uuid::Uuid::new_v4();
    let mut store = store_with(AppSettings::default());
    assert!(store.theme_preview_key().is_none());

    store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(profile), first));
    let armed = store.theme_preview_key();
    assert!(armed.is_some());

    // Moving to another option on the same slot changes the key.
    store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(profile), second));
    assert_ne!(armed, store.theme_preview_key());

    // So does the same theme on a different slot.
    store.theme_preview = Some((PreviewSlot::TerminalDark, second));
    let terminal_slot = store.theme_preview_key();
    store.theme_preview = Some((PreviewSlot::InterfaceDark, second));
    assert_ne!(terminal_slot, store.theme_preview_key());

    // And clearing it returns to "nothing armed", so the gate notices the restore.
    store.theme_preview = None;
    assert!(store.theme_preview_key().is_none());
}

#[test]
fn an_existing_settings_file_keeps_its_chosen_themes() {
    // The rename would otherwise silently reset both themes to the built-in defaults on first launch,
    // because serde ignores unknown keys. The aliases are what make that a non-event.
    let config: AppSettings = serde_json::from_str(
        r#"{"dark_theme":"Atom One Dark","light_theme":"iTerm2 Solarized Light"}"#,
    )
    .unwrap();
    assert_eq!(config.interface_dark_theme, "Atom One Dark");
    assert_eq!(config.interface_light_theme, "iTerm2 Solarized Light");
    // An old file says nothing about the terminal layer, so it follows the interface — which is
    // exactly the behaviour that file was written under.
    assert_eq!(
        config.terminal_theme_source,
        TerminalThemeSource::FollowInterface
    );
}

#[test]
fn settings_round_trip_and_normalize_font_size() {
    let directory = std::env::temp_dir().join(format!("eggie-settings-{}", uuid::Uuid::new_v4()));
    let path = directory.join("settings.json");
    let mut config = AppSettings {
        language: Language::English,
        theme_mode: ThemeMode::Light,
        interface_dark_theme: "Catppuccin Mocha".to_owned(),
        interface_light_theme: "Ayu Light".to_owned(),
        terminal_theme_source: TerminalThemeSource::default(),
        terminal_dark_theme: DEFAULT_DARK_THEME.to_owned(),
        terminal_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
        font_family: "Menlo".to_owned(),
        font_family_bold: String::new(),
        font_family_italic: String::new(),
        font_family_bold_italic: String::new(),
        font_synthetic_style: FontSyntheticStyle::default(),
        ligatures: true,
        font_features: Vec::new(),
        font_shaping_break_cursor: true,
        font_variations: Vec::new(),
        font_thicken: false,
        font_thicken_strength: 255,
        font_codepoint_map: Vec::new(),
        font_size: 100.,
        terminal_padding_x: -20.,
        terminal_padding_y: f32::NAN,
        minimum_contrast: 99.,
        progress_complete_timeout_secs: 0,
        progress_stale_timeout_secs: u32::MAX,
        allow_osc_clipboard_read: true,
        detect_urls: false,
        copy_on_select: false,
        auto_check_updates: true,
        update_channel: UpdateChannel::Beta,
        cursor_shape: CursorShapeSetting::Bar,
        cursor_blink: CursorBlink::On,
        bell_mode: BellMode::Sound,
        open_directory_with: OpenWith::VsCode,
        font_metrics: FontMetricAdjustments::default(),
        keybindings: std::collections::BTreeMap::new(),
        scrollback_lines: MAX_SCROLLBACK_LINES + 1,
        shell_program: "  /bin/fish  ".to_owned(),
        shell_args: vec!["-l".to_owned(), "  ".to_owned()],
        shell_integration_path: false,
        remote_install_consent: std::collections::BTreeMap::from([(
            "deploy@prod.example.com".to_owned(),
            true,
        )]),
    };
    config.normalize();
    fs::create_dir_all(&directory).unwrap();
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();

    let loaded = SettingsStore::load_from(path);
    assert_eq!(loaded.config.font_size, MAX_FONT_SIZE);
    // Remembered remote-install answers survive a round trip: being re-asked on every launch
    // would defeat the point of recording the decision.
    assert_eq!(
        loaded
            .config
            .remote_install_consent
            .get("deploy@prod.example.com"),
        Some(&true)
    );
    assert_eq!(loaded.config.terminal_padding_x, MIN_TERMINAL_PADDING);
    assert_eq!(loaded.config.terminal_padding_y, DEFAULT_TERMINAL_PADDING_Y);
    assert_eq!(loaded.config.minimum_contrast, MAX_MINIMUM_CONTRAST);
    assert_eq!(loaded.config.scrollback_lines, MAX_SCROLLBACK_LINES);
    assert_eq!(loaded.config.shell_program, "/bin/fish");
    assert_eq!(loaded.config.shell_args, vec!["-l".to_owned()]);
    // A non-default (false) value serializes and survives the round trip.
    assert!(!loaded.config.shell_integration_path);
    // A non-default opener persists across the round trip.
    assert_eq!(loaded.config.open_directory_with, OpenWith::VsCode);
    assert_eq!(
        loaded.config.progress_complete_timeout_secs,
        MIN_PROGRESS_TIMEOUT_SECS
    );
    assert_eq!(
        loaded.config.progress_stale_timeout_secs,
        MAX_PROGRESS_TIMEOUT_SECS
    );
    assert_eq!(loaded.config.interface_light_theme, "Ayu Light");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn shell_features_assemble_into_the_env_string() {
    assert_eq!(ShellFeatures { path: true }.to_env_string(), "path");
    assert_eq!(ShellFeatures { path: false }.to_env_string(), "");
    // The path feature ships on, so the value migrated into the Default profile carries it. (The
    // assembled env string now comes from the profile — see `resolve_spawn_config`.)
    assert!(
        AppSettings::default()
            .legacy_shell_settings()
            .shell_integration_path
    );
}

/// The global theme a terminal without an override inherits, for the merge tests.
fn global_theme() -> &'static TerminalTheme {
    AppSettings::default().terminal_theme(true)
}

/// The global terminal layer for the merge tests, with the dark half in effect.
fn global_layer() -> GlobalTerminalLayer {
    GlobalTerminalLayer {
        theme: global_theme(),
        use_dark: true,
    }
}

/// A profile appearance overriding both terminal halves with the same name — the "looks the same in
/// either appearance" shape the single `theme` slot used to express.
fn both_halves(name: &str) -> eggie_domain::ProfileAppearance {
    eggie_domain::ProfileAppearance {
        terminal_dark_theme: Some(name.to_owned()),
        terminal_light_theme: Some(name.to_owned()),
        ..Default::default()
    }
}

#[test]
fn appearance_without_overrides_is_the_global_layer() {
    let config = AppSettings::default();
    let resolved = resolve_appearance(&config, None, global_layer(), None);
    assert_eq!(resolved.theme.name, global_theme().name);
    assert_eq!(resolved.font_family.as_ref(), config.font_family.as_str());
    assert_eq!(resolved.font_size, config.font_size);

    // An all-inherit override behaves identically to no profile at all.
    let empty = eggie_domain::ProfileAppearance::default();
    assert_eq!(
        resolve_appearance(&config, Some(&empty), global_layer(), None),
        resolved
    );
}

#[test]
fn profile_overrides_win_field_by_field() {
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    config.font_size = 14.;

    // Only the size is overridden: family and theme still come from the global layer.
    let appearance = eggie_domain::ProfileAppearance {
        font_size: Some(18.),
        ..Default::default()
    };
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(resolved.font_size, 18.);
    assert_eq!(resolved.font_family.as_ref(), "Menlo");
    assert_eq!(resolved.theme.name, global_theme().name);

    // Filling both halves with one name is how a "prod is red" profile keeps its marking when the
    // system flips — the single-slot behaviour, now stated explicitly rather than implied.
    let named = theme_catalog().dark_names().into_iter().next().unwrap();
    // Guard the test itself: if the catalog ever ships a theme that sorts first AND is the global
    // default, the assertions below would degenerate into `global == global` and pass vacuously.
    assert_ne!(
        named,
        global_theme().name,
        "the override must name a theme that differs from the global one"
    );
    let appearance = both_halves(&named);
    for use_dark in [true, false] {
        let global = GlobalTerminalLayer {
            theme: config.terminal_theme(use_dark),
            use_dark,
        };
        let resolved = resolve_appearance(&config, Some(&appearance), global, None);
        assert_eq!(
            resolved.theme.name, named,
            "with both halves set, the override must hold in both appearances"
        );
    }
}

#[test]
fn a_profile_override_follows_the_appearance_half_by_half() {
    // What the split buys: a profile can mark itself differently (or not at all) per appearance.
    let config = AppSettings::default();
    let dark_name = theme_catalog().dark_names().into_iter().next().unwrap();
    let appearance = eggie_domain::ProfileAppearance {
        terminal_dark_theme: Some(dark_name.clone()),
        terminal_light_theme: None,
        ..Default::default()
    };

    // Dark half: the override applies.
    let resolved = resolve_appearance(
        &config,
        Some(&appearance),
        GlobalTerminalLayer {
            theme: config.terminal_theme(true),
            use_dark: true,
        },
        None,
    );
    assert_eq!(resolved.theme.name, dark_name);

    // Light half: only the dark slot is set, so this one inherits the global terminal theme. This is
    // the case the old single-slot model could not express.
    let light_global = config.terminal_theme(false);
    let resolved = resolve_appearance(
        &config,
        Some(&appearance),
        GlobalTerminalLayer {
            theme: light_global,
            use_dark: false,
        },
        None,
    );
    assert_eq!(resolved.theme.name, light_global.name);
}

#[test]
fn a_profile_preview_overrides_the_stored_slot() {
    // The hover preview has to win over the profile's own stored value, or hovering an option on a
    // profile that already has one would show nothing.
    let config = AppSettings::default();
    let stored = theme_catalog().dark_names().into_iter().next().unwrap();
    let preview = theme_catalog().theme_by_name("Catppuccin Mocha").unwrap();
    assert_ne!(
        stored, preview.name,
        "the preview must differ from the stored value"
    );
    let appearance = both_halves(&stored);
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), Some(preview));
    assert_eq!(resolved.theme.name, preview.name);
}

#[test]
fn two_terminals_in_one_window_resolve_independently() {
    // The central premise: one window, one global layer, two terminals that must not agree.
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    config.font_size = 14.;
    let global = global_theme();

    let themed = eggie_domain::ProfileAppearance {
        // A light theme in the dark half is fine here: the profile layer resolves by name across the
        // whole catalog, and the point of this test is only that the two terminals disagree.
        terminal_dark_theme: theme_catalog().light_names().into_iter().next(),
        font_family: Some("SF Mono".to_owned()),
        font_size: Some(20.),
        ..Default::default()
    };
    let overridden = resolve_appearance(&config, Some(&themed), global_layer(), None);
    // A terminal with no profile (adopted/restored, or a profile with no overrides) follows global.
    let inherited = resolve_appearance(&config, None, global_layer(), None);

    assert_ne!(
        overridden.theme.name, inherited.theme.name,
        "the themed terminal must not share the other's palette"
    );
    assert_eq!(inherited.theme.name, global.name);
    assert_eq!(overridden.font_size, 20.);
    assert_eq!(inherited.font_size, 14.);
    assert_eq!(overridden.font_family.as_ref(), "SF Mono");
    assert_eq!(inherited.font_family.as_ref(), "Menlo");
}

#[test]
fn an_override_matching_the_global_theme_resolves_to_the_same_palette() {
    // Worth pinning because the daemon push is diffed on the resolved palette: a profile that names
    // the theme already in effect must produce no change, hence no redundant SetAppearance.
    let config = AppSettings::default();
    let global = global_theme();
    let appearance = both_halves(&global.name);
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(resolved.theme.appearance(), global.appearance());
}

#[test]
fn an_unknown_theme_name_falls_back_to_the_global_theme() {
    let config = AppSettings::default();
    let appearance = both_halves("No Such Theme");
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(
        resolved.theme.name,
        global_theme().name,
        "a typo must degrade to no override, not to an arbitrary theme"
    );
}

#[test]
fn a_profile_font_family_moves_the_styles_that_inherited_it() {
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    // Italic has its own family; bold and bold-italic inherit the regular one.
    config.font_family_italic = "Courier New".to_owned();

    let appearance = eggie_domain::ProfileAppearance {
        font_family: Some("SF Mono".to_owned()),
        ..Default::default()
    };
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(resolved.font_family.as_ref(), "SF Mono");
    assert_eq!(resolved.families.regular.as_ref(), "SF Mono");
    assert_eq!(
        resolved.families.bold.as_ref(),
        "SF Mono",
        "a slot that inherited the regular family must follow the override"
    );
    assert_eq!(resolved.families.bold_italic.as_ref(), "SF Mono");
    assert_eq!(
        resolved.families.italic.as_ref(),
        "Courier New",
        "a slot with its own explicit family keeps it"
    );
}

#[test]
fn a_style_pinned_to_the_regular_family_is_still_treated_as_explicit() {
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    // Deliberately pinning bold to the same name as regular: `resolved_font_families` collapses
    // this into the same string as "unset", so inheritance must be read from the source config.
    config.font_family_bold = "Menlo".to_owned();

    let appearance = eggie_domain::ProfileAppearance {
        font_family: Some("SF Mono".to_owned()),
        ..Default::default()
    };
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(resolved.families.regular.as_ref(), "SF Mono");
    assert_eq!(
        resolved.families.bold.as_ref(),
        "Menlo",
        "an explicitly pinned style must not follow the profile's family override"
    );
    // The genuinely-unset styles do follow it.
    assert_eq!(resolved.families.italic.as_ref(), "SF Mono");
    assert_eq!(resolved.families.bold_italic.as_ref(), "SF Mono");
}

#[test]
fn a_profile_font_size_is_clamped_and_sanitized() {
    let config = AppSettings::default();
    let resolve = |size: f32| {
        let appearance = eggie_domain::ProfileAppearance {
            font_size: Some(size),
            ..Default::default()
        };
        resolve_appearance(&config, Some(&appearance), global_layer(), None).font_size
    };
    assert_eq!(resolve(1_000.), MAX_FONT_SIZE);
    assert_eq!(resolve(1.), MIN_FONT_SIZE);
    assert_eq!(
        resolve(f32::NAN),
        config.font_size,
        "a non-finite override is ignored"
    );
}

#[test]
fn a_blank_profile_font_family_is_not_an_override() {
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    let appearance = eggie_domain::ProfileAppearance {
        font_family: Some("   ".to_owned()),
        ..Default::default()
    };
    let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
    assert_eq!(
        resolved.font_family.as_ref(),
        "Menlo",
        "whitespace must mean inherit, matching how the global per-style families behave"
    );
}

#[test]
fn legacy_settings_use_ghostty_terminal_padding_defaults() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "dark_theme": "Builtin Dark",
                "light_theme": "Builtin Light",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();

    assert_eq!(config.terminal_padding_x, DEFAULT_TERMINAL_PADDING_X);
    assert_eq!(config.terminal_padding_y, DEFAULT_TERMINAL_PADDING_Y);
    assert_eq!(config.minimum_contrast, DEFAULT_MINIMUM_CONTRAST);
    assert_eq!(
        config.progress_complete_timeout_secs,
        DEFAULT_PROGRESS_COMPLETE_TIMEOUT_SECS
    );
    assert_eq!(
        config.progress_stale_timeout_secs,
        DEFAULT_PROGRESS_STALE_TIMEOUT_SECS
    );
}

#[test]
fn legacy_settings_without_keybindings_field_load_empty() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert!(config.keybindings.is_empty());
}

#[test]
fn bell_mode_defaults_to_flash() {
    assert_eq!(BellMode::default(), BellMode::Flash);
    assert_eq!(AppSettings::default().bell_mode, BellMode::Flash);
}

#[test]
fn copy_on_select_defaults_to_enabled() {
    assert!(AppSettings::default().copy_on_select);
}

#[test]
fn legacy_settings_without_copy_on_select_default_to_enabled() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert!(config.copy_on_select);
}

#[test]
fn cursor_shape_defaults_to_block() {
    assert_eq!(CursorShapeSetting::default(), CursorShapeSetting::Block);
    assert_eq!(
        AppSettings::default().cursor_shape,
        CursorShapeSetting::Block
    );
}

#[test]
fn legacy_settings_without_cursor_shape_default_to_block() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert_eq!(config.cursor_shape, CursorShapeSetting::Block);
}

#[test]
fn cursor_blink_defaults_to_program() {
    assert_eq!(CursorBlink::default(), CursorBlink::Program);
    assert_eq!(AppSettings::default().cursor_blink, CursorBlink::Program);
}

#[test]
fn cursor_blink_resolve_follows_the_three_states() {
    // Program follows the program-reported bit.
    assert!(CursorBlink::Program.resolve(true));
    assert!(!CursorBlink::Program.resolve(false));
    // On/Off force the outcome regardless of the program.
    assert!(CursorBlink::On.resolve(false));
    assert!(!CursorBlink::Off.resolve(true));
}

#[test]
fn legacy_settings_without_cursor_blink_default_to_program() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert_eq!(config.cursor_blink, CursorBlink::Program);
}

#[test]
fn legacy_settings_without_bell_mode_default_to_flash() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert_eq!(config.bell_mode, BellMode::Flash);
}

#[test]
fn bell_mode_sound_and_flash_predicates() {
    assert!(!BellMode::Silent.plays_sound() && !BellMode::Silent.flashes());
    assert!(BellMode::Flash.flashes() && !BellMode::Flash.plays_sound());
    assert!(BellMode::Sound.plays_sound() && !BellMode::Sound.flashes());
    assert!(BellMode::FlashAndSound.plays_sound() && BellMode::FlashAndSound.flashes());
}

#[test]
fn normalize_drops_unknown_action_ids() {
    let mut config = AppSettings::default();
    config
        .keybindings
        .insert("not_a_real_action".to_owned(), "cmd-shift-z".to_owned());
    config.normalize();
    assert!(config.keybindings.is_empty());
}

#[test]
fn normalize_drops_invalid_keystrokes() {
    let mut config = AppSettings::default();
    // Two non-modifier keys is not a valid single keystroke.
    config
        .keybindings
        .insert("terminal_copy".to_owned(), "cmd-a-b".to_owned());
    config.normalize();
    assert!(config.keybindings.is_empty());
}

#[test]
fn normalize_drops_overrides_equal_to_default() {
    let mut config = AppSettings::default();
    // terminal_copy's default is cmd-c; storing it as an override is redundant.
    config
        .keybindings
        .insert("terminal_copy".to_owned(), "cmd-c".to_owned());
    config.normalize();
    assert!(config.keybindings.is_empty());
}

#[test]
fn normalize_keeps_valid_overrides() {
    let mut config = AppSettings::default();
    config
        .keybindings
        .insert("terminal_copy".to_owned(), "cmd-shift-c".to_owned());
    config.normalize();
    assert_eq!(
        config.keybindings.get("terminal_copy").map(String::as_str),
        Some("cmd-shift-c")
    );
}

#[test]
fn keybindings_only_serialize_when_present() {
    let config = AppSettings::default();
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("keybindings"),
        "empty keybindings should be skipped: {json}"
    );

    let mut with_override = AppSettings::default();
    with_override
        .keybindings
        .insert("terminal_copy".to_owned(), "cmd-shift-c".to_owned());
    let json = serde_json::to_string(&with_override).unwrap();
    assert!(json.contains("keybindings"));
    assert!(json.contains("terminal_copy"));
}

#[test]
fn minimum_contrast_matches_ghostty_black_or_white_fallback() {
    assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.), 0x222222);
    assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.1), 0xffffff);
    assert_eq!(minimum_contrast_rgb(0xeeeeee, 0xeeeeee, 1.1), 0x000000);
    assert_eq!(minimum_contrast_rgb(0xffffff, 0x000000, 21.), 0xffffff);
}

#[test]
fn resolved_adjust_honors_floor_and_absence() {
    assert_eq!(ResolvedMetricAdjustments::adjust(None, 10., 1.), 10.);
    assert!(
        (ResolvedMetricAdjustments::adjust(Some(MetricModifier::Absolute(2)), 10., 1.) - 12.).abs()
            < 1e-6
    );
    // A large negative delta is clamped to the floor.
    assert_eq!(
        ResolvedMetricAdjustments::adjust(Some(MetricModifier::Absolute(-100)), 3., 1.),
        1.
    );
}

#[test]
fn normalize_drops_invalid_metric_adjustments_and_keeps_valid_ones() {
    let mut config = AppSettings::default();
    config.font_metrics.cell_height = Some("20%".to_owned());
    config.font_metrics.underline_thickness = Some("bogus".to_owned());
    config.font_metrics.cursor_thickness = Some("".to_owned());
    config.font_metrics.icon_height = Some("-15%".to_owned());
    config.normalize();
    assert_eq!(config.font_metrics.cell_height.as_deref(), Some("20%"));
    assert_eq!(config.font_metrics.underline_thickness, None);
    assert_eq!(config.font_metrics.cursor_thickness, None);
    assert_eq!(config.font_metrics.icon_height.as_deref(), Some("-15%"));
    assert!(matches!(
        config.font_metrics.resolve().icon_height,
        Some(MetricModifier::Percent(m)) if (m - 0.85).abs() < 1e-9
    ));
}

#[test]
fn font_metrics_only_serialize_when_present() {
    let config = AppSettings::default();
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("font_metrics"),
        "empty font_metrics should be skipped: {json}"
    );

    let mut with_adjust = AppSettings::default();
    with_adjust.font_metrics.cell_height = Some("2".to_owned());
    let json = serde_json::to_string(&with_adjust).unwrap();
    assert!(json.contains("font_metrics"));
    assert!(json.contains("cell_height"));
}

#[test]
fn legacy_settings_without_font_metrics_default_to_empty() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert_eq!(config.font_metrics, FontMetricAdjustments::default());
}

#[test]
fn scrollback_and_shell_default_and_serialize_only_when_non_default() {
    let config = AppSettings::default();
    assert_eq!(config.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
    assert!(config.shell_program.is_empty());
    assert!(config.shell_args.is_empty());
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("scrollback_lines"),
        "default skipped: {json}"
    );
    assert!(!json.contains("shell_program"));
    assert!(!json.contains("shell_args"));

    let mut custom = AppSettings::default();
    custom.scrollback_lines = 500;
    custom.shell_program = "/opt/homebrew/bin/fish".to_owned();
    custom.shell_args = vec!["-l".to_owned()];
    let json = serde_json::to_string(&custom).unwrap();
    let round_trip: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(round_trip.scrollback_lines, 500);
    assert_eq!(round_trip.shell_program, "/opt/homebrew/bin/fish");
    assert_eq!(round_trip.shell_args, vec!["-l".to_owned()]);
}

#[test]
fn legacy_settings_without_scrollback_or_shell_use_defaults() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert_eq!(config.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
    assert!(config.shell_program.is_empty());
    assert!(config.shell_args.is_empty());
}

#[test]
fn normalize_clamps_scrollback_and_cleans_shell_fields() {
    let mut config = AppSettings::default();
    config.scrollback_lines = MAX_SCROLLBACK_LINES + 5_000;
    config.shell_program = "  /bin/fish  ".to_owned();
    config.shell_args = vec![" -l ".to_owned(), "  ".to_owned(), String::new()];
    config.normalize();
    assert_eq!(config.scrollback_lines, MAX_SCROLLBACK_LINES);
    assert_eq!(config.shell_program, "/bin/fish");
    // Blank/whitespace-only args dropped; the meaningful one is kept (untrimmed value preserved
    // beyond the emptiness check — argv semantics are the daemon's concern).
    assert_eq!(config.shell_args, vec![" -l ".to_owned()]);

    // Whitespace-only shell path normalizes to empty (=> $SHELL fallback).
    let mut blank = AppSettings::default();
    blank.shell_program = "   ".to_owned();
    blank.normalize();
    assert!(blank.shell_program.is_empty());
}

#[test]
fn per_style_families_default_to_empty_and_fall_back_to_regular() {
    let config = AppSettings::default();
    assert!(config.font_family_bold.is_empty());
    assert!(config.font_family_italic.is_empty());
    assert!(config.font_family_bold_italic.is_empty());
    let families = config.resolved_font_families();
    // Every style resolves to the regular family when no dedicated family is set.
    assert_eq!(families.family(false, false).as_ref(), config.font_family);
    assert_eq!(families.family(true, false).as_ref(), config.font_family);
    assert_eq!(families.family(false, true).as_ref(), config.font_family);
    assert_eq!(families.family(true, true).as_ref(), config.font_family);
    assert!(!families.has_dedicated_family(true, false));
}

#[test]
fn dedicated_bold_family_is_selected_for_bold_style() {
    let mut config = AppSettings::default();
    config.font_family = "Menlo".to_owned();
    config.font_family_bold = "Menlo Bold".to_owned();
    let families = config.resolved_font_families();
    assert_eq!(families.family(true, false).as_ref(), "Menlo Bold");
    assert!(families.has_dedicated_family(true, false));
    // Italic has no dedicated family, so it still falls back to regular.
    assert_eq!(families.family(false, true).as_ref(), "Menlo");
    assert!(!families.has_dedicated_family(false, true));
}

#[test]
fn font_synthetic_style_defaults_to_all_enabled_and_gates_synthesis() {
    let config = AppSettings::default();
    assert_eq!(config.font_synthetic_style, FontSyntheticStyle::default());
    let families = config.resolved_font_families();
    assert!(families.allows_synthesis(true, false));
    assert!(families.allows_synthesis(false, true));

    let mut disabled = AppSettings::default();
    disabled.font_synthetic_style.italic = false;
    let families = disabled.resolved_font_families();
    assert!(!families.allows_synthesis(false, true));
    assert!(families.allows_synthesis(true, false));
}

#[test]
fn legacy_settings_without_font_family_styles_default_to_empty_and_synthetic_on() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert!(config.font_family_bold.is_empty());
    assert!(config.font_family_italic.is_empty());
    assert!(config.font_family_bold_italic.is_empty());
    assert_eq!(config.font_synthetic_style, FontSyntheticStyle::default());
}

#[test]
fn normalize_clears_whitespace_only_style_families() {
    let mut config = AppSettings::default();
    config.font_family_bold = "   ".to_owned();
    config.normalize();
    assert!(config.font_family_bold.is_empty());
}

#[test]
fn ligatures_default_on_and_resolve_to_no_features() {
    let config = AppSettings::default();
    assert!(config.ligatures);
    assert!(config.font_shaping_break_cursor);
    assert!(config.resolved_font_features().is_empty());
}

#[test]
fn ligatures_off_disables_liga_calt_dlig() {
    let mut config = AppSettings::default();
    config.ligatures = false;
    let features = config.resolved_font_features();
    let disabled: Vec<_> = features
        .iter()
        .filter(|f| f.value == 0)
        .map(|f| f.tag)
        .collect();
    assert!(disabled.contains(b"liga"));
    assert!(disabled.contains(b"calt"));
    assert!(disabled.contains(b"dlig"));
}

#[test]
fn user_font_features_append_after_master_toggle() {
    let mut config = AppSettings::default();
    config.ligatures = false;
    config.font_features = vec!["+liga".to_owned(), "ss01".to_owned(), "bogus".to_owned()];
    let features = config.resolved_font_features();
    // The last `liga` entry (user +liga) wins under HarfBuzz "later wins" ordering.
    let last_liga = features.iter().rev().find(|f| &f.tag == b"liga").unwrap();
    assert_eq!(last_liga.value, 1);
    assert!(features.iter().any(|f| &f.tag == b"ss01" && f.value == 1));
    // Invalid entries are dropped.
    assert!(
        features.len() == 3 /* liga,calt,dlig */ + 2 /* +liga, ss01 */
    );
}

#[test]
fn legacy_settings_without_ligature_fields_default_on() {
    let config: AppSettings = serde_json::from_str(
        r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
    )
    .unwrap();
    assert!(config.ligatures);
    assert!(config.font_shaping_break_cursor);
    assert!(config.font_features.is_empty());
}

#[test]
fn default_ligature_fields_are_not_serialized() {
    let config = AppSettings::default();
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        !json.contains("ligatures"),
        "default ligatures skipped: {json}"
    );
    assert!(!json.contains("font_shaping_break_cursor"));
    assert!(!json.contains("font_features"));
}

#[test]
fn resolved_font_variations_drops_invalid_entries() {
    let mut config = AppSettings::default();
    config.font_variations = vec!["wght=300".to_owned(), "bad".to_owned()];
    let variations = config.resolved_font_variations();
    assert_eq!(variations.len(), 1);
    assert_eq!(&variations[0].tag, b"wght");
}

#[test]
fn thicken_defaults_off_at_full_strength_and_is_not_serialized() {
    let config = AppSettings::default();
    assert!(!config.font_thicken);
    assert_eq!(config.font_thicken_strength, 255);
    let json = serde_json::to_string(&config).unwrap();
    assert!(!json.contains("font_thicken"), "defaults skipped: {json}");
    assert!(!json.contains("font_variations"));
}

#[test]
fn legacy_settings_without_variation_or_thicken_use_defaults() {
    let config: AppSettings =
        serde_json::from_str(r#"{ "font_family": "Menlo", "font_size": 14 }"#).unwrap();
    assert!(config.font_variations.is_empty());
    assert!(!config.font_thicken);
    assert_eq!(config.font_thicken_strength, 255);
}

#[test]
fn resolved_codepoint_map_drops_invalid_entries() {
    let mut config = AppSettings::default();
    config.font_codepoint_map = vec!["U+E000-U+E0FF=Nerd".to_owned(), "garbage".to_owned()];
    let map = config.resolved_codepoint_map();
    assert_eq!(map.len(), 1);
    assert_eq!(map[0].family, "Nerd");
    // Default is empty and not serialized.
    assert!(AppSettings::default().resolved_codepoint_map().is_empty());
    let json = serde_json::to_string(&AppSettings::default()).unwrap();
    assert!(!json.contains("font_codepoint_map"));
}
