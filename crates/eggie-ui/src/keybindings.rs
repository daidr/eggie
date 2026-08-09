//! Central registry for user-configurable keyboard shortcuts.
//!
//! GPUI's `actions!` macro only generates action *types* — it does not build a
//! runtime `name -> constructor` table. This module bridges that gap with a
//! `&'static` metadata slice ([`ACTION_SPECS`]) where each entry maps a stable
//! string id to everything needed to render, store, and rebuild a keybinding:
//! its default keystroke, its localized label, and a closure that constructs the
//! concrete GPUI `KeyBinding` (the closure fixes the concrete `Action` type, so
//! we never need to reconstruct a `Box<dyn Action>` from a string).
//!
//! The keymap is rebuilt at runtime from settings via [`rebuild_keymap`], so
//! edits in the settings window take effect without a restart.

use gpui::{App, DummyKeyboardMapper, KeyBinding, Keystroke};

use crate::settings::{AppSettings, Language};
use crate::settings_window::{
    ClearScreen, CloseTab, FontDecrease, FontIncrease, FontReset, JumpNextPrompt, JumpPrevPrompt,
    NewTab, NextTab, OpenSettings, PageDown, PageUp, PrevTab, Quit, ScrollBottom, ScrollTop,
    SplitDown, SplitRight, TerminalCopy, TerminalFind, TerminalPaste, TerminalSelectAll,
};

/// One user-configurable action. `ACTION_SPECS` is the single source of truth;
/// its order is the order rows render in the settings window.
#[derive(Clone, Copy)]
pub(crate) struct ActionSpec {
    /// Stable storage id (never changes across renames/translations), e.g. `"terminal_copy"`.
    pub id: &'static str,
    /// Default keystroke in canonical ascii form, e.g. `"cmd-c"`.
    pub default_keystroke: &'static str,
    /// Localized display label.
    pub label: fn(Language) -> &'static str,
    /// Builds a terminal `KeyBinding` (context = None) for the given keystroke.
    /// The closure fixes the concrete `Action` type. Returns `None` on parse
    /// failure so a bad override is skipped rather than panicking.
    pub build: fn(&str) -> Option<KeyBinding>,
}

/// Fallibly build a keybinding. Uses `KeyBinding::load` (not `::new`, which
/// panics on parse error) so malformed keystrokes are skipped.
fn load_binding<A: gpui::Action>(keystrokes: &str, action: A) -> Option<KeyBinding> {
    KeyBinding::load(
        keystrokes,
        Box::new(action),
        None,
        false,
        None,
        &DummyKeyboardMapper,
    )
    .ok()
}

/// The single source of truth for configurable actions.
pub(crate) const ACTION_SPECS: &[ActionSpec] = &[
    ActionSpec {
        id: "open_settings",
        default_keystroke: "cmd-,",
        label: Language::action_open_settings,
        build: |ks| load_binding(ks, OpenSettings),
    },
    ActionSpec {
        id: "quit",
        default_keystroke: "cmd-q",
        label: Language::action_quit,
        build: |ks| load_binding(ks, Quit),
    },
    ActionSpec {
        id: "terminal_copy",
        default_keystroke: "cmd-c",
        label: Language::action_terminal_copy,
        build: |ks| load_binding(ks, TerminalCopy),
    },
    ActionSpec {
        id: "terminal_paste",
        default_keystroke: "cmd-v",
        label: Language::action_terminal_paste,
        build: |ks| load_binding(ks, TerminalPaste),
    },
    ActionSpec {
        id: "terminal_select_all",
        default_keystroke: "cmd-a",
        label: Language::action_terminal_select_all,
        build: |ks| load_binding(ks, TerminalSelectAll),
    },
    ActionSpec {
        id: "terminal_find",
        default_keystroke: "cmd-f",
        label: Language::action_terminal_find,
        build: |ks| load_binding(ks, TerminalFind),
    },
    ActionSpec {
        id: "new_tab",
        default_keystroke: "cmd-t",
        label: Language::action_new_tab,
        build: |ks| load_binding(ks, NewTab),
    },
    ActionSpec {
        id: "close_tab",
        default_keystroke: "cmd-w",
        label: Language::action_close_tab,
        build: |ks| load_binding(ks, CloseTab),
    },
    ActionSpec {
        id: "next_tab",
        default_keystroke: "cmd-shift-]",
        label: Language::action_next_tab,
        build: |ks| load_binding(ks, NextTab),
    },
    ActionSpec {
        id: "prev_tab",
        default_keystroke: "cmd-shift-[",
        label: Language::action_prev_tab,
        build: |ks| load_binding(ks, PrevTab),
    },
    ActionSpec {
        id: "split_right",
        default_keystroke: "cmd-d",
        label: Language::action_split_right,
        build: |ks| load_binding(ks, SplitRight),
    },
    ActionSpec {
        id: "split_down",
        default_keystroke: "cmd-shift-d",
        label: Language::action_split_down,
        build: |ks| load_binding(ks, SplitDown),
    },
    ActionSpec {
        id: "clear_screen",
        default_keystroke: "cmd-k",
        label: Language::action_clear_screen,
        build: |ks| load_binding(ks, ClearScreen),
    },
    ActionSpec {
        id: "scroll_top",
        default_keystroke: "cmd-home",
        label: Language::action_scroll_top,
        build: |ks| load_binding(ks, ScrollTop),
    },
    ActionSpec {
        id: "scroll_bottom",
        default_keystroke: "cmd-end",
        label: Language::action_scroll_bottom,
        build: |ks| load_binding(ks, ScrollBottom),
    },
    ActionSpec {
        id: "page_up",
        default_keystroke: "cmd-pageup",
        label: Language::action_page_up,
        build: |ks| load_binding(ks, PageUp),
    },
    ActionSpec {
        id: "page_down",
        default_keystroke: "cmd-pagedown",
        label: Language::action_page_down,
        build: |ks| load_binding(ks, PageDown),
    },
    ActionSpec {
        id: "font_increase",
        default_keystroke: "cmd-=",
        label: Language::action_font_increase,
        build: |ks| load_binding(ks, FontIncrease),
    },
    ActionSpec {
        id: "font_decrease",
        default_keystroke: "cmd--",
        label: Language::action_font_decrease,
        build: |ks| load_binding(ks, FontDecrease),
    },
    ActionSpec {
        id: "font_reset",
        default_keystroke: "cmd-0",
        label: Language::action_font_reset,
        build: |ks| load_binding(ks, FontReset),
    },
    ActionSpec {
        id: "jump_prev_prompt",
        default_keystroke: "cmd-up",
        label: Language::action_jump_prev_prompt,
        build: |ks| load_binding(ks, JumpPrevPrompt),
    },
    ActionSpec {
        id: "jump_next_prompt",
        default_keystroke: "cmd-down",
        label: Language::action_jump_next_prompt,
        build: |ks| load_binding(ks, JumpNextPrompt),
    },
];

/// Look up a spec by its stable id.
pub(crate) fn spec_by_id(id: &str) -> Option<&'static ActionSpec> {
    ACTION_SPECS.iter().find(|spec| spec.id == id)
}

/// The default keystroke for an action id, if the id is known.
pub(crate) fn default_keystroke(id: &str) -> Option<&'static str> {
    spec_by_id(id).map(|spec| spec.default_keystroke)
}

/// The keystroke currently in effect for an action: the user override if present,
/// otherwise the default.
pub(crate) fn effective_keystroke<'a>(config: &'a AppSettings, id: &str) -> Option<&'a str> {
    let spec = spec_by_id(id)?;
    Some(
        config
            .keybindings
            .get(id)
            .map(String::as_str)
            .unwrap_or(spec.default_keystroke),
    )
}

/// Find an action (other than `exclude_id`) whose effective keystroke equals
/// `candidate`. All configurable bindings share `context = None`, so equal
/// canonical strings mean a real conflict. Returns the conflicting action's id.
pub(crate) fn find_conflict(
    config: &AppSettings,
    candidate: &str,
    exclude_id: &str,
) -> Option<&'static str> {
    ACTION_SPECS
        .iter()
        .filter(|spec| spec.id != exclude_id)
        .find(|spec| {
            config
                .keybindings
                .get(spec.id)
                .map(String::as_str)
                .unwrap_or(spec.default_keystroke)
                == candidate
        })
        .map(|spec| spec.id)
}

/// True when `key` is a bare modifier name (produced by GPUI when only a
/// modifier is held). Used to ignore the intermediate state while recording,
/// before the user presses a real key.
pub(crate) fn is_bare_modifier(key: &str) -> bool {
    matches!(
        key,
        "shift" | "control" | "alt" | "platform" | "function" | "cmd" | ""
    )
}

/// Canonical ascii keystroke for storage and conflict comparison. Fixed modifier
/// order `fn-ctrl-alt-cmd-shift-<key>` — identical to `gpui::Keystroke::unparse`
/// and round-trippable through `Keystroke::parse`.
pub(crate) fn canonical_keystroke(keystroke: &Keystroke) -> String {
    let modifiers = &keystroke.modifiers;
    let mut result = String::new();
    if modifiers.function {
        result.push_str("fn-");
    }
    if modifiers.control {
        result.push_str("ctrl-");
    }
    if modifiers.alt {
        result.push_str("alt-");
    }
    if modifiers.platform {
        result.push_str("cmd-");
    }
    if modifiers.shift {
        result.push_str("shift-");
    }
    result.push_str(&keystroke.key);
    result
}

/// Human-friendly keystroke for display (macOS symbols: ⌘⇧⌥⌃ + key). Named
/// keys that GPUI renders verbatim get nicer symbols here.
pub(crate) fn display_keystroke(canonical: &str) -> String {
    let Ok(keystroke) = Keystroke::parse(canonical) else {
        return canonical.to_string();
    };
    let modifiers = &keystroke.modifiers;
    let mut result = String::new();
    if modifiers.control {
        result.push('⌃');
    }
    if modifiers.alt {
        result.push('⌥');
    }
    if modifiers.shift {
        result.push('⇧');
    }
    if modifiers.platform {
        result.push('⌘');
    }
    result.push_str(&display_key(&keystroke.key));
    result
}

fn display_key(key: &str) -> String {
    match key {
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "tab" => "⇥".to_string(),
        "enter" => "⏎".to_string(),
        "escape" => "⎋".to_string(),
        "backspace" => "⌫".to_string(),
        "delete" => "⌦".to_string(),
        "space" => "␣".to_string(),
        "home" => "↖".to_string(),
        "end" => "↘".to_string(),
        "pageup" => "⇞".to_string(),
        "pagedown" => "⇟".to_string(),
        key if key.chars().count() == 1 => key.to_uppercase(),
        key => key.to_string(),
    }
}

/// Rebuild the entire app keymap from the current settings. Clears all existing
/// bindings, re-adds the terminal bindings (default or user override) first,
/// then the text-input bindings — this order preserves the binding-index
/// tiebreak so `EggieTextInput`-context bindings win when a text field is focused.
///
/// This only touches the keymap (keystroke -> action). Action *handlers*
/// (`on_action`) are registered exactly once elsewhere and must never be
/// re-registered here.
pub(crate) fn rebuild_keymap(config: &AppSettings, cx: &mut App) {
    cx.clear_key_bindings();
    let mut bindings = Vec::with_capacity(ACTION_SPECS.len());
    for spec in ACTION_SPECS {
        let keystroke = config
            .keybindings
            .get(spec.id)
            .map(String::as_str)
            .unwrap_or(spec.default_keystroke);
        if let Some(binding) =
            (spec.build)(keystroke).or_else(|| (spec.build)(spec.default_keystroke))
        {
            bindings.push(binding);
        }
    }
    cx.bind_keys(bindings);
    crate::text_input::install_keybindings(cx);
}

/// Convenience for tests / callers that need an override map keyed by id.
#[cfg(test)]
pub(crate) fn make_overrides(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(id, ks)| ((*id).to_string(), (*ks).to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;
    use std::collections::BTreeMap;

    fn config_with(overrides: BTreeMap<String, String>) -> AppSettings {
        AppSettings {
            keybindings: overrides,
            ..AppSettings::default()
        }
    }

    #[test]
    fn action_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in ACTION_SPECS {
            assert!(seen.insert(spec.id), "duplicate action id: {}", spec.id);
        }
    }

    #[test]
    fn every_default_keystroke_parses_and_builds() {
        for spec in ACTION_SPECS {
            assert!(
                Keystroke::parse(spec.default_keystroke).is_ok(),
                "default keystroke does not parse for {}: {:?}",
                spec.id,
                spec.default_keystroke
            );
            assert!(
                (spec.build)(spec.default_keystroke).is_some(),
                "default keystroke does not build for {}: {:?}",
                spec.id,
                spec.default_keystroke
            );
        }
    }

    #[test]
    fn default_keymap_has_no_internal_conflicts() {
        let config = AppSettings::default();
        for spec in ACTION_SPECS {
            assert_eq!(
                find_conflict(&config, spec.default_keystroke, spec.id),
                None,
                "default keystroke {:?} for {} conflicts with another action",
                spec.default_keystroke,
                spec.id
            );
        }
    }

    #[test]
    fn canonical_roundtrips_through_parse() {
        for source in ["cmd-c", "cmd-shift-d", "ctrl-alt-cmd-shift-k", "cmd-home"] {
            let parsed = Keystroke::parse(source).unwrap();
            let canonical = canonical_keystroke(&parsed);
            let reparsed = Keystroke::parse(&canonical).unwrap();
            assert_eq!(parsed.modifiers, reparsed.modifiers, "modifiers for {source}");
            assert_eq!(parsed.key, reparsed.key, "key for {source}");
        }
    }

    #[test]
    fn canonical_uses_fixed_modifier_order() {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                control: true,
                alt: true,
                shift: true,
                platform: true,
                function: false,
            },
            key: "k".to_string(),
            key_char: None,
        };
        assert_eq!(canonical_keystroke(&keystroke), "ctrl-alt-cmd-shift-k");
    }

    #[test]
    fn canonical_matches_default_table_for_shifted_key() {
        // ⌘⇧D -> a real event is shift + lowercase d + platform.
        let keystroke = Keystroke {
            modifiers: Modifiers {
                control: false,
                alt: false,
                shift: true,
                platform: true,
                function: false,
            },
            key: "d".to_string(),
            key_char: None,
        };
        assert_eq!(canonical_keystroke(&keystroke), "cmd-shift-d");
        assert_eq!(default_keystroke("split_down"), Some("cmd-shift-d"));
    }

    #[test]
    fn display_uses_macos_symbols() {
        assert_eq!(display_keystroke("cmd-shift-t"), "⇧⌘T");
        assert_eq!(display_keystroke("cmd-home"), "⌘↖");
        assert_eq!(display_keystroke("cmd-pageup"), "⌘⇞");
        assert_eq!(display_keystroke("cmd-,"), "⌘,");
        // With shift preserved for symbol keys, ⌘⇧, displays naturally.
        assert_eq!(display_keystroke("cmd-shift-,"), "⇧⌘,");
        assert_eq!(display_keystroke("cmd-shift-]"), "⇧⌘]");
    }

    #[test]
    fn tab_switch_defaults_match_live_unfolded_events() {
        // After the event-parser fix, a live ⌘⇧] / ⌘⇧[ arrives unfolded: shift preserved,
        // key is the base symbol. The natural default keystroke must match it.
        for (default, base_key) in [("cmd-shift-]", "]"), ("cmd-shift-[", "[")] {
            let live = Keystroke {
                modifiers: Modifiers {
                    control: false,
                    alt: false,
                    shift: true,
                    platform: true,
                    function: false,
                },
                key: base_key.to_string(),
                key_char: None,
            };
            let binding = KeyBinding::new(default, crate::settings_window::NextTab, None);
            assert_eq!(
                binding.match_keystrokes(&[live]),
                Some(false),
                "default {default} should match the live unfolded event for key {base_key}"
            );
        }
    }

    #[test]
    fn bare_modifier_detection() {
        assert!(is_bare_modifier("cmd"));
        assert!(is_bare_modifier("shift"));
        assert!(is_bare_modifier("platform"));
        assert!(is_bare_modifier(""));
        assert!(!is_bare_modifier("k"));
        assert!(!is_bare_modifier("home"));
    }

    #[test]
    fn conflict_detected_against_default() {
        let config = AppSettings::default();
        // cmd-c is terminal_copy's default; recording it for new_tab conflicts.
        assert_eq!(find_conflict(&config, "cmd-c", "new_tab"), Some("terminal_copy"));
    }

    #[test]
    fn conflict_excludes_self() {
        let config = AppSettings::default();
        assert_eq!(find_conflict(&config, "cmd-c", "terminal_copy"), None);
    }

    #[test]
    fn no_conflict_for_unused_keystroke() {
        let config = AppSettings::default();
        assert_eq!(find_conflict(&config, "cmd-shift-y", "new_tab"), None);
    }

    #[test]
    fn override_shadows_default_in_conflict_check() {
        // Rebind terminal_copy to cmd-shift-y; now cmd-c is free, cmd-shift-y taken.
        let config = config_with(make_overrides(&[("terminal_copy", "cmd-shift-y")]));
        assert_eq!(find_conflict(&config, "cmd-c", "new_tab"), None);
        assert_eq!(
            find_conflict(&config, "cmd-shift-y", "new_tab"),
            Some("terminal_copy")
        );
    }

    #[test]
    fn effective_keystroke_prefers_override() {
        let config = config_with(make_overrides(&[("terminal_copy", "cmd-shift-y")]));
        assert_eq!(effective_keystroke(&config, "terminal_copy"), Some("cmd-shift-y"));
        assert_eq!(effective_keystroke(&config, "new_tab"), Some("cmd-t"));
        assert_eq!(effective_keystroke(&config, "unknown"), None);
    }
}
