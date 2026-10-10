
use super::*;

#[test]
fn a_legacy_single_slot_theme_migrates_into_both_terminal_slots() {
    // The pre-split shape: one `theme` name that covered both appearances. Folding it into both
    // slots preserves exactly that meaning.
    let mut appearance: ProfileAppearance =
        serde_json::from_str(r#"{"theme":"iTerm2 Tango Light"}"#).unwrap();
    assert_eq!(
        appearance.legacy_theme.as_deref(),
        Some("iTerm2 Tango Light")
    );
    appearance.migrate_legacy();

    assert_eq!(
        appearance.terminal_dark_theme.as_deref(),
        Some("iTerm2 Tango Light")
    );
    assert_eq!(
        appearance.terminal_light_theme.as_deref(),
        Some("iTerm2 Tango Light")
    );
    assert!(
        appearance.legacy_theme.is_none(),
        "the legacy value is taken, not copied"
    );
    assert!(!appearance.is_empty());

    // Re-serializing drops the old key, so the migration happens once and leaves no trace.
    let encoded = serde_json::to_string(&appearance).unwrap();
    assert!(
        !encoded.contains("\"theme\""),
        "the legacy key must not be written back: {encoded}"
    );
    assert!(encoded.contains("terminal_dark_theme"));

    // Idempotent: a second pass changes nothing.
    let before = appearance.clone();
    appearance.migrate_legacy();
    assert_eq!(appearance, before);
}

#[test]
fn migration_never_overwrites_a_slot_the_file_already_carries() {
    // A hand-edited file could hold both shapes. The new fields are the more specific statement of
    // intent, so they win.
    let mut appearance: ProfileAppearance =
        serde_json::from_str(r#"{"theme":"Legacy","terminal_dark_theme":"Explicit Dark"}"#)
            .unwrap();
    appearance.migrate_legacy();
    assert_eq!(
        appearance.terminal_dark_theme.as_deref(),
        Some("Explicit Dark")
    );
    assert_eq!(
        appearance.terminal_light_theme.as_deref(),
        Some("Legacy"),
        "the empty slot still takes the legacy value"
    );
}

#[test]
fn an_unmigrated_legacy_value_does_not_read_as_an_override() {
    // `is_empty` gates serialization and the "does this profile override anything" question. Counting
    // `legacy_theme` would report an override that no slot actually holds.
    let appearance: ProfileAppearance = serde_json::from_str(r#"{"theme":"Legacy"}"#).unwrap();
    assert!(appearance.is_empty());
}

#[test]
fn a_terminal_override_is_selected_by_the_appearance_in_effect() {
    let appearance = ProfileAppearance {
        terminal_dark_theme: Some("Dark One".to_owned()),
        terminal_light_theme: None,
        ..Default::default()
    };
    assert_eq!(appearance.terminal_theme(true), Some("Dark One"));
    assert_eq!(
        appearance.terminal_theme(false),
        None,
        "an unset half inherits rather than borrowing the other half"
    );
}

#[test]
fn env_key_validation_rejects_every_injection_shape() {
    assert!(is_valid_env_key("AWS_PROFILE"));
    assert!(is_valid_env_key("_private"));
    assert!(is_valid_env_key("X1"));

    // A name is interpolated on the left of `NAME=value`, so these are not merely odd names:
    // each one ends the assignment and starts a command in the remote bootstrap script.
    assert!(
        !is_valid_env_key("X\nid > /tmp/pwned\nY"),
        "newline splits the line"
    );
    assert!(!is_valid_env_key("X;id"), "`;` chains a command inline");
    assert!(!is_valid_env_key("X$(id)"), "command substitution");
    assert!(!is_valid_env_key("X`id`"), "backtick substitution");
    assert!(!is_valid_env_key("X Y"), "a space makes two words");
    assert!(
        !is_valid_env_key("X=Y"),
        "an embedded `=` shifts the assignment"
    );
    // Shape rules.
    assert!(!is_valid_env_key(""), "empty is not a name");
    assert!(!is_valid_env_key("1ST"), "a name cannot start with a digit");
    assert!(
        !is_valid_env_key("PÄTH"),
        "non-ASCII is outside the accepted set"
    );
}

#[test]
fn the_consent_key_distinguishes_ports_but_the_ssh_destination_does_not() {
    let base = RemoteTarget {
        host: "host".to_owned(),
        user: Some("deploy".to_owned()),
        port: None,
    };
    let forwarded = RemoteTarget {
        port: Some(2222),
        ..base.clone()
    };
    // The ssh destination argument never carries the port (it travels as `-p`).
    assert_eq!(base.destination(), "deploy@host");
    assert_eq!(forwarded.destination(), "deploy@host");
    // But consent must: a forwarded port is very often a different machine, and consenting to
    // write into one account's home must not authorize another.
    assert_ne!(
        base.consent_key(),
        forwarded.consent_key(),
        "a different port must be asked about separately"
    );
    assert_eq!(forwarded.consent_key(), "deploy@host:2222");
    // Per account as well: the install lands in that account's $HOME.
    let other_user = RemoteTarget {
        user: Some("root".to_owned()),
        ..base.clone()
    };
    assert_ne!(base.consent_key(), other_user.consent_key());
}

#[test]
fn remote_targets_that_could_pass_as_ssh_options_are_rejected() {
    assert!(RemoteTarget::new("host.example.com".to_owned()).is_usable());
    assert!(
        RemoteTarget {
            host: "host".to_owned(),
            user: Some("deploy".to_owned()),
            port: Some(22),
        }
        .is_usable()
    );

    // Each of these would reach ssh's argv; a leading dash makes it an option, and
    // `-oProxyCommand=` in that position executes locally at connect time.
    for host in [
        "-oProxyCommand=touch /tmp/pwned",
        "--",
        "-p2222",
        "",
        "host with spaces",
        "host\twith\ttabs",
    ] {
        assert!(
            !RemoteTarget::new(host.to_owned()).is_usable(),
            "host {host:?} must be rejected"
        );
    }
    // The user half reaches the same argv entry, so it needs the same guard.
    assert!(
        !RemoteTarget {
            host: "host".to_owned(),
            user: Some("-oProxyCommand=x".to_owned()),
            port: None,
        }
        .is_usable()
    );
}

#[test]
fn groups_own_independent_tab_lists_after_split() {
    let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
    let mut layout = LayoutNode::group(first);
    let original_group = match &layout {
        LayoutNode::Group { group } => group.id,
        _ => unreachable!(),
    };
    let second = TabItem::terminal(Uuid::new_v4(), "shell 2");
    let second_group = layout
        .split_group(original_group, SplitAxis::Horizontal, second)
        .expect("group should split");

    assert_eq!(
        layout.find_group_mut(original_group).unwrap().items.len(),
        1
    );
    assert_eq!(layout.find_group_mut(second_group).unwrap().items.len(), 1);
}

#[test]
fn terminal_items_can_be_found_across_split_groups() {
    let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
    let mut layout = LayoutNode::group(first);
    let original_group = layout.first_group_id();
    let session_id = Uuid::new_v4();
    let second = TabItem::terminal(session_id, "shell 2");
    let second_item_id = second.id;
    let second_group = layout
        .split_group(original_group, SplitAxis::Horizontal, second)
        .unwrap();

    assert_eq!(
        layout.find_terminal_item(session_id),
        Some((second_group, second_item_id))
    );
}

#[test]
fn terminal_titles_update_across_split_groups_by_session_id() {
    let first_session_id = Uuid::new_v4();
    let second_session_id = Uuid::new_v4();
    let mut layout = LayoutNode::group(TabItem::terminal(first_session_id, "shell 1"));
    let first_group_id = layout.first_group_id();
    let second_group_id = layout
        .split_group(
            first_group_id,
            SplitAxis::Horizontal,
            TabItem::terminal(second_session_id, "shell 2"),
        )
        .unwrap();

    assert!(layout.update_terminal_title(second_session_id, "OSC title"));
    assert!(!layout.update_terminal_title(second_session_id, "OSC title"));
    assert_eq!(
        layout.find_group(first_group_id).unwrap().items[0].title,
        "shell 1"
    );
    assert_eq!(
        layout.find_group(second_group_id).unwrap().items[0].title,
        "OSC title"
    );
}

#[test]
fn custom_title_overrides_process_title_and_survives_osc_updates() {
    let session_id = Uuid::new_v4();
    let item = TabItem::terminal(session_id, "process title");
    let item_id = item.id;
    let mut layout = LayoutNode::group(item);
    let group_id = layout.first_group_id();

    // No override yet: display falls back to the process title.
    assert_eq!(
        layout.find_group(group_id).unwrap().items[0].display_title(),
        "process title"
    );

    // Setting a custom title wins over the process title.
    assert!(layout.set_custom_title(group_id, item_id, Some("My Tab".to_owned())));
    assert!(!layout.set_custom_title(group_id, item_id, Some("My Tab".to_owned())));
    assert_eq!(
        layout.find_group(group_id).unwrap().items[0].display_title(),
        "My Tab"
    );

    // An OSC title update changes the process title but never the custom override.
    assert!(layout.update_terminal_title(session_id, "new process title"));
    let item = &layout.find_group(group_id).unwrap().items[0];
    assert_eq!(item.title, "new process title");
    assert_eq!(item.custom_title.as_deref(), Some("My Tab"));
    assert_eq!(item.display_title(), "My Tab");

    // Clearing the override restores the (now-updated) process title.
    assert!(layout.set_custom_title(group_id, item_id, None));
    assert_eq!(
        layout.find_group(group_id).unwrap().items[0].display_title(),
        "new process title"
    );
}

#[test]
fn terminal_display_title_reflects_custom_title_by_session_id() {
    let session_id = Uuid::new_v4();
    let item = TabItem::terminal(session_id, "process title");
    let item_id = item.id;
    let mut layout = LayoutNode::group(item);
    let group_id = layout.first_group_id();

    // Falls back to the process title when no override is set.
    assert_eq!(
        layout.terminal_display_title(session_id),
        Some("process title")
    );
    assert_eq!(layout.terminal_custom_title(session_id), None);
    // An unknown session id has no tab.
    assert_eq!(layout.terminal_display_title(Uuid::new_v4()), None);

    // A custom title wins, keyed off the session id (what the sidebar uses).
    layout.set_custom_title(group_id, item_id, Some("Custom".to_owned()));
    assert_eq!(layout.terminal_display_title(session_id), Some("Custom"));
    assert_eq!(layout.terminal_custom_title(session_id), Some("Custom"));
}

#[test]
fn closing_the_active_item_activates_a_neighbor() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let first_id = first.id;
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let second_id = second.id;
    let mut group = TabGroup::new(first);
    group.add_item(second);

    group.close_item(second_id);

    assert_eq!(group.active_item_id, Some(first_id));
}

#[test]
fn closing_the_last_item_in_a_split_collapses_its_group() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let mut layout = LayoutNode::group(first);
    let original_group = layout.first_group_id();
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let second_item_id = second.id;
    let second_group = layout
        .split_group(original_group, SplitAxis::Vertical, second)
        .unwrap();

    let removed = layout.close_item(second_group, second_item_id).unwrap();

    assert_eq!(removed.title, "two");
    assert!(layout.find_group(second_group).is_none());
    assert_eq!(layout.first_group_id(), original_group);
}

#[test]
fn closing_the_only_root_item_keeps_an_empty_group_for_new_tabs() {
    let only = TabItem::terminal(Uuid::new_v4(), "one");
    let item_id = only.id;
    let mut layout = LayoutNode::group(only);
    let group_id = layout.first_group_id();

    layout.close_item(group_id, item_id).unwrap();

    let group = layout.find_group(group_id).unwrap();
    assert!(group.items.is_empty());
    assert_eq!(group.active_item_id, None);
}

#[test]
fn directional_splits_place_groups_on_the_requested_side() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let mut layout = LayoutNode::group(first);
    let original_group = layout.first_group_id();
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let new_group = layout
        .split_group_direction(original_group, Direction::Left, second)
        .unwrap();

    assert_eq!(layout.top_left_group_id(), new_group);
    assert_eq!(layout.top_right_group_id(), original_group);
    assert_eq!(
        layout.neighbor_group_id(new_group, Direction::Right),
        Some(original_group)
    );
    assert_eq!(
        layout.neighbor_group_id(original_group, Direction::Left),
        Some(new_group)
    );
}

#[test]
fn moving_a_tab_to_another_group_prunes_an_empty_source_group() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let first_item_id = first.id;
    let mut layout = LayoutNode::group(first);
    let first_group = layout.first_group_id();
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let second_group = layout
        .split_group_direction(first_group, Direction::Right, second)
        .unwrap();

    assert_eq!(
        layout.move_item(first_group, first_item_id, second_group),
        Some(second_group)
    );
    assert!(layout.find_group(first_group).is_none());
    assert_eq!(layout.find_group(second_group).unwrap().items.len(), 2);
}

#[test]
fn moving_a_tab_at_an_index_reorders_within_its_group() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let first_item_id = first.id;
    let mut layout = LayoutNode::group(first);
    let group_id = layout.first_group_id();
    let group = layout.find_group_mut(group_id).unwrap();
    group.add_item(TabItem::terminal(Uuid::new_v4(), "two"));
    group.add_item(TabItem::terminal(Uuid::new_v4(), "three"));

    assert_eq!(
        layout.move_item_at(group_id, first_item_id, group_id, 3),
        Some(group_id)
    );

    let group = layout.find_group(group_id).unwrap();
    assert_eq!(
        group
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["two", "three", "one"]
    );
    assert_eq!(group.active_item_id, Some(first_item_id));

    assert_eq!(
        layout.move_item_at(group_id, first_item_id, group_id, 0),
        Some(group_id)
    );
    assert_eq!(
        layout
            .find_group(group_id)
            .unwrap()
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
}

#[test]
fn moving_a_tab_at_an_index_inserts_into_another_group() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let first_item_id = first.id;
    let mut layout = LayoutNode::group(first);
    let first_group = layout.first_group_id();
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let second_group = layout
        .split_group_direction(first_group, Direction::Right, second)
        .unwrap();
    layout
        .find_group_mut(second_group)
        .unwrap()
        .add_item(TabItem::terminal(Uuid::new_v4(), "three"));

    assert_eq!(
        layout.move_item_at(first_group, first_item_id, second_group, 1),
        Some(second_group)
    );

    assert!(layout.find_group(first_group).is_none());
    let group = layout.find_group(second_group).unwrap();
    assert_eq!(
        group
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect::<Vec<_>>(),
        ["two", "one", "three"]
    );
}

#[test]
fn moving_a_tab_to_an_edge_creates_a_group_at_that_edge() {
    let first = TabItem::terminal(Uuid::new_v4(), "one");
    let second = TabItem::terminal(Uuid::new_v4(), "two");
    let second_item_id = second.id;
    let mut layout = LayoutNode::group(first);
    let original_group = layout.first_group_id();
    layout
        .find_group_mut(original_group)
        .unwrap()
        .add_item(second);

    let new_group = layout
        .move_item_to_new_group(
            original_group,
            second_item_id,
            original_group,
            Direction::Up,
        )
        .unwrap();

    assert_eq!(layout.top_left_group_id(), new_group);
    assert_eq!(layout.find_group(new_group).unwrap().items.len(), 1);
    assert_eq!(layout.find_group(original_group).unwrap().items.len(), 1);
}

#[test]
fn a_sole_tab_cannot_split_its_own_group_by_moving_it() {
    let only = TabItem::terminal(Uuid::new_v4(), "one");
    let item_id = only.id;
    let mut layout = LayoutNode::group(only);
    let group_id = layout.first_group_id();

    assert_eq!(
        layout.move_item_to_new_group(group_id, item_id, group_id, Direction::Down),
        None
    );
    assert_eq!(layout.find_group(group_id).unwrap().items.len(), 1);
}

#[test]
fn nested_splits_have_distinct_ids_and_resize_independently() {
    // Build: root Horizontal split, whose `first` child is itself a Vertical split. The inner
    // split's first group is also the root's first group, so identifying a split by its first
    // group would make the two collide. Each must instead have its own id.
    let top_left = TabItem::terminal(Uuid::new_v4(), "top-left");
    let mut layout = LayoutNode::group(top_left);
    let left_group = layout.first_group_id();

    // Split to the right first -> outer Horizontal split (left column / right).
    layout
        .split_group_direction(
            left_group,
            Direction::Right,
            TabItem::terminal(Uuid::new_v4(), "right"),
        )
        .unwrap();
    // Then split the left group downward -> inner Vertical split nested in the outer's `first`.
    layout
        .split_group_direction(
            left_group,
            Direction::Down,
            TabItem::terminal(Uuid::new_v4(), "bottom-left"),
        )
        .unwrap();

    let (outer_id, inner_id) = match &layout {
        LayoutNode::Split {
            id: outer_id,
            axis: SplitAxis::Horizontal,
            first,
            ..
        } => match first.as_ref() {
            LayoutNode::Split {
                id: inner_id,
                axis: SplitAxis::Vertical,
                ..
            } => (*outer_id, *inner_id),
            other => panic!("expected inner vertical split, got {other:?}"),
        },
        other => panic!("expected outer horizontal split, got {other:?}"),
    };

    assert_ne!(outer_id, inner_id, "nested splits must not share an id");
    assert_eq!(layout.split_axis(outer_id), Some(SplitAxis::Horizontal));
    assert_eq!(layout.split_axis(inner_id), Some(SplitAxis::Vertical));

    // Resizing the inner split must not touch the outer split's ratio, and vice versa.
    assert!(layout.set_split_ratio(inner_id, 0.3));
    assert_eq!(layout.split_ratio(inner_id), Some(0.3));
    assert_eq!(layout.split_ratio(outer_id), Some(0.5));

    assert!(layout.set_split_ratio(outer_id, 0.7));
    assert_eq!(layout.split_ratio(outer_id), Some(0.7));
    assert_eq!(layout.split_ratio(inner_id), Some(0.3));
}
