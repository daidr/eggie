//! Data model for the command palette (⌘⇧P).
//!
//! The palette lists every command the user can invoke and dispatches the chosen one. Its entries
//! come from two sources merged in display order:
//! - [`crate::keybindings::ACTION_SPECS`] — the configurable commands (each carries a live keystroke
//!   shown on the right), reusing the same `make_action` the keymap builds from.
//! - [`MENU_ONLY_COMMANDS`] — commands that live only in the menu bar (no configurable shortcut),
//!   so the palette is a superset of both surfaces.
//!
//! This module is pure data + filtering (no GPUI view state), so it can be unit-tested in isolation.
//! The palette's UI state, rendering, and event handling live in `app.rs` alongside the other
//! floating overlays.

use gpui::Action;

use crate::keybindings::{ACTION_SPECS, display_keystroke, effective_keystroke};
use crate::settings::{AppSettings, Language};
use crate::settings_window::{
    About, CheckForUpdates, CloseAllWindows, HelpDocs, Hide, HideOthers, ShowAll, SplitLeft,
    SplitUp, ToggleSecureKeyboardEntry, ZoomWindow,
};

/// A command that exists only in the menu bar (no configurable keybinding), surfaced in the palette
/// so every menu action is reachable by search too.
struct MenuCommand {
    /// Localized display label (reuses the menu's own i18n string).
    label: fn(Language) -> &'static str,
    /// Constructs the action to dispatch when chosen.
    make_action: fn() -> Box<dyn Action>,
}

/// Menu-only commands, in the order they appear across the menu bar (Eggie → File → Window → Help).
const MENU_ONLY_COMMANDS: &[MenuCommand] = &[
    MenuCommand {
        label: Language::about_eggie,
        make_action: || Box::new(About),
    },
    MenuCommand {
        label: Language::secure_keyboard_entry,
        make_action: || Box::new(ToggleSecureKeyboardEntry),
    },
    MenuCommand {
        label: Language::action_split_left,
        make_action: || Box::new(SplitLeft),
    },
    MenuCommand {
        label: Language::action_split_up,
        make_action: || Box::new(SplitUp),
    },
    MenuCommand {
        label: Language::close_all_windows_menu_item,
        make_action: || Box::new(CloseAllWindows),
    },
    MenuCommand {
        label: Language::zoom_menu_item,
        make_action: || Box::new(ZoomWindow),
    },
    MenuCommand {
        label: Language::hide_eggie,
        make_action: || Box::new(Hide),
    },
    MenuCommand {
        label: Language::hide_others,
        make_action: || Box::new(HideOthers),
    },
    MenuCommand {
        label: Language::show_all,
        make_action: || Box::new(ShowAll),
    },
    MenuCommand {
        label: Language::help_docs_menu_item,
        make_action: || Box::new(HelpDocs),
    },
    MenuCommand {
        label: Language::check_for_updates_menu_item,
        make_action: || Box::new(CheckForUpdates),
    },
];

/// One row in the command palette: a label, its shortcut (if any), and the action to dispatch.
pub(crate) struct PaletteEntry {
    pub(crate) label: &'static str,
    /// The command's effective shortcut in display form (e.g. "⌘T"), or `None` for menu-only
    /// commands with no configurable keybinding.
    pub(crate) keystroke: Option<String>,
    pub(crate) action: Box<dyn Action>,
}

/// Build the full palette command list for the current language and keybindings. Configurable
/// commands come first (each with its effective shortcut), then the menu-only commands. The palette
/// itself (`command_palette`) is excluded so it never lists "open the command palette".
pub(crate) fn palette_entries(config: &AppSettings) -> Vec<PaletteEntry> {
    let language = config.language;
    let mut entries = Vec::with_capacity(ACTION_SPECS.len() + MENU_ONLY_COMMANDS.len());
    for spec in ACTION_SPECS {
        if spec.id == "command_palette" {
            continue;
        }
        entries.push(PaletteEntry {
            label: (spec.label)(language),
            keystroke: effective_keystroke(config, spec.id).map(display_keystroke),
            action: (spec.make_action)(),
        });
    }
    for command in MENU_ONLY_COMMANDS {
        entries.push(PaletteEntry {
            label: (command.label)(language),
            keystroke: None,
            action: (command.make_action)(),
        });
    }
    entries
}

/// Whether `label` matches `query`: an empty query matches everything; otherwise a case-insensitive
/// substring match (same semantics as the settings selector filter).
pub(crate) fn command_matches(label: &str, query: &str) -> bool {
    query.is_empty() || label.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_matches_is_case_insensitive_substring() {
        assert!(command_matches("New Tab", "tab"));
        assert!(command_matches("New Tab", "NEW"));
        assert!(command_matches("New Tab", ""));
        assert!(!command_matches("New Tab", "split"));
    }

    #[test]
    fn palette_lists_configurable_and_menu_only_commands() {
        let config = AppSettings::default();
        let entries = palette_entries(&config);

        // 27 configurable commands (28 ACTION_SPECS minus command_palette) + the menu-only set.
        assert_eq!(
            entries.len(),
            ACTION_SPECS.len() - 1 + MENU_ONLY_COMMANDS.len()
        );

        // The palette never lists a "command palette" entry that would reopen itself.
        let palette_label = Language::English.action_command_palette();
        assert!(entries.iter().all(|entry| entry.label != palette_label));

        // A configurable command shows its shortcut; a menu-only command does not.
        let new_tab = entries
            .iter()
            .find(|entry| entry.label == Language::English.action_new_tab())
            .expect("new_tab should be present");
        assert_eq!(new_tab.keystroke.as_deref(), Some("⌘T"));

        let about = entries
            .iter()
            .find(|entry| entry.label == Language::English.about_eggie())
            .expect("about should be present");
        assert_eq!(about.keystroke, None);
    }
}
