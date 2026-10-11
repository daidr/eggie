use super::{SelectorKind, selector_matches};

#[cfg(target_os = "macos")]
#[test]
fn filtered_font_query_preserves_installed_monospace_families() {
    use core_text::font_descriptor::{SymbolicTraitAccessors, TraitAccessors};
    use std::collections::BTreeSet;

    let collection = core_text::font_collection::create_for_all_families();
    let descriptors = collection.get_descriptors().expect("installed fonts");
    let mut expected = descriptors
        .into_iter()
        .filter(|descriptor| descriptor.traits().symbolic_traits().is_monospace())
        .map(|descriptor| descriptor.family_name())
        .collect::<BTreeSet<_>>();
    expected.insert("Menlo".into());
    let actual = super::macos_monospace_font_names();
    assert_eq!(actual.iter().cloned().collect::<BTreeSet<_>>(), expected);
    assert!(
        actual
            .windows(2)
            .all(|names| names[0].to_lowercase() <= names[1].to_lowercase())
    );
    assert_eq!(actual.len(), expected.len());
}

#[test]
fn selector_indices_are_a_bijection_onto_the_bounds_array() {
    // `index()`, `ALL`, and the `selector_bounds` array length are three hand-maintained facts that
    // must agree. They have drifted before (see CAPABILITIES_TODO), and the failure mode is a panic
    // when the dropdown reads or writes its bounds slot — invisible until that selector is opened.
    let mut seen = [false; SelectorKind::ALL.len()];
    for kind in SelectorKind::ALL {
        let index = kind.index();
        assert!(
            index < seen.len(),
            "{kind:?} indexes {index}, past the end of selector_bounds ({})",
            seen.len()
        );
        assert!(!seen[index], "{kind:?} reuses index {index}");
        seen[index] = true;
    }
    assert!(
        seen.into_iter().all(|hit| hit),
        "every bounds slot must be claimed by exactly one selector"
    );
}

#[test]
fn every_theme_selector_belongs_to_exactly_one_half() {
    // Each theme slot offers one half of the catalog, so `is_dark` has to partition them. A font
    // selector is not a theme selector and never previews.
    for kind in SelectorKind::ALL {
        if kind.is_font() {
            assert!(
                !kind.is_theme(),
                "{kind:?} cannot be both a font and a theme selector"
            );
        } else {
            assert!(kind.is_theme(), "{kind:?} must be a theme selector");
        }
    }
    assert!(SelectorKind::InterfaceDark.is_dark());
    assert!(!SelectorKind::InterfaceLight.is_dark());
    assert!(SelectorKind::TerminalDark.is_dark());
    assert!(!SelectorKind::TerminalLight.is_dark());
    assert!(SelectorKind::ProfileTerminalDark.is_dark());
    assert!(!SelectorKind::ProfileTerminalLight.is_dark());
}

#[test]
fn only_theme_selectors_have_a_preview_slot() {
    let profile_id = eggie_domain::ProfileId::new_v4();
    for kind in SelectorKind::ALL {
        assert_eq!(
            kind.preview_slot(profile_id).is_some(),
            kind.is_theme(),
            "{kind:?}: a preview slot exists exactly for theme selectors"
        );
    }
}

#[test]
fn selector_search_is_case_insensitive_and_supports_partial_matches() {
    assert!(selector_matches("Maple Mono NF CN", "maple"));
    assert!(selector_matches("Builtin Dark", "IN DA"));
    assert!(!selector_matches("Menlo", "mono"));
    assert!(selector_matches("Menlo", ""));
}
