use super::*;

/// The global terminal layer a profile's overrides merge over, together with the dark/light decision
/// the profile layer has to repeat.
///
/// These two always travel together — resolving a profile override needs both the fallback theme and
/// which half of the profile's own pair to read — so they are one value rather than two parameters
/// threaded side by side through the spawn paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlobalTerminalLayer {
    pub(crate) theme: &'static TerminalTheme,
    pub(crate) use_dark: bool,
}

impl GlobalTerminalLayer {
    pub(crate) fn new(config: &AppSettings, system_is_dark: bool) -> Self {
        Self {
            theme: config.terminal_theme(system_is_dark),
            use_dark: config.use_dark(system_is_dark),
        }
    }
}

/// A terminal's appearance after merging its profile's overrides over the global settings.
///
/// One of these is resolved per terminal on every render pass, so a change to either layer moves the
/// terminals it applies to and leaves the rest alone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedAppearance {
    pub(crate) theme: &'static TerminalTheme,
    pub(crate) font_family: std::sync::Arc<str>,
    pub(crate) font_size: f32,
    /// Per-style families, with the profile's family (when it overrides one) as the regular slot.
    pub(crate) families: ResolvedFontFamilies,
}

/// Merge a profile's appearance overrides over the global settings: profile → global → built-in
/// default, one field at a time.
///
/// `global` is the already-resolved global *terminal* layer plus the dark/light decision (see
/// [`GlobalTerminalLayer`] and [`AppSettings::terminal_theme`] — a profile never overrides the
/// interface, because chrome is window-level while one window can show tabs from several profiles at
/// once).
///
/// The dark/light decision cannot be folded into the resolved theme the way it once was: the profile
/// layer has to make the same choice again, so a system flip moves a profile-themed terminal even when
/// the global palette is unchanged. An unknown theme name falls back to `global.theme` rather than to
/// an arbitrary catalog entry, so a typo degrades to "no override".
///
/// `profile_theme_preview` is the settings window's live hover preview when it applies to *this*
/// profile. The caller decides applicability, because it knows which profile the session belongs to and
/// this module deliberately stays profile-id agnostic.
///
/// `font_size` is clamped to the app's supported range, matching what `AppSettings::normalize` does to
/// the global value; a non-finite override is ignored.
pub(crate) fn resolve_appearance(
    config: &AppSettings,
    appearance: Option<&eggie_domain::ProfileAppearance>,
    global: GlobalTerminalLayer,
    profile_theme_preview: Option<&'static TerminalTheme>,
) -> ResolvedAppearance {
    let theme = profile_theme_preview
        .or_else(|| {
            appearance
                .and_then(|appearance| appearance.terminal_theme(global.use_dark))
                .and_then(|name| theme_catalog().theme_by_name(name))
        })
        .unwrap_or(global.theme);

    let mut families = config.resolved_font_families();
    let font_family: std::sync::Arc<str> = match appearance
        .and_then(|appearance| appearance.font_family.as_deref())
        .map(str::trim)
        .filter(|family| !family.is_empty())
    {
        Some(family) => {
            // Overriding the regular family must also move every per-style slot that merely inherited
            // it, or a profile's font would apply to regular text and leave bold/italic on the global
            // family. Slots with their own explicit family keep it.
            //
            // Inheritance is read from the *source* config, not from the resolved strings:
            // `resolved_font_families` already collapsed "unset" and "explicitly set to the regular
            // family" into the same value, so comparing resolved names would move a slot the user had
            // deliberately pinned.
            let overridden: std::sync::Arc<str> = std::sync::Arc::from(family);
            let inherits = |explicit: &str| explicit.trim().is_empty();
            if inherits(&config.font_family_bold) {
                families.bold = std::sync::Arc::clone(&overridden);
            }
            if inherits(&config.font_family_italic) {
                families.italic = std::sync::Arc::clone(&overridden);
            }
            if inherits(&config.font_family_bold_italic) {
                families.bold_italic = std::sync::Arc::clone(&overridden);
            }
            families.regular = std::sync::Arc::clone(&overridden);
            overridden
        }
        None => std::sync::Arc::clone(&families.regular),
    };

    let font_size = appearance
        .and_then(|appearance| appearance.font_size)
        .filter(|size| size.is_finite())
        .map(|size| size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
        .unwrap_or(config.font_size);

    ResolvedAppearance {
        theme,
        font_family,
        font_size,
        families,
    }
}
