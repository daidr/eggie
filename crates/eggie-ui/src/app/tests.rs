use super::*;
use gpui::{Keystroke, Modifiers};
use uuid::Uuid;

#[test]
fn project_popover_keyboard_navigation_starts_at_the_active_terminal() {
    let project = ProjectId::new_v4();
    let window = WindowId::new_v4();
    let group = GroupId::new_v4();
    let mut tabs: Vec<_> = (0..3)
        .map(|index| {
            let item_id = ItemId::new_v4();
            ProjectPopoverTab {
                location: eggie_domain::ItemLocation {
                    window_id: window,
                    project_id: project,
                    group_id: group,
                    item_id,
                },
                session_id: None,
                item_id,
                title: "/".into(),
                subtitle: None,
                icon: ProcessIcon::Terminal,
                active: index == 1,
            }
        })
        .collect();
    assert_eq!(project_popover_selection_index(&tabs, None), Some(1));

    // A live catalog reorder must keep keyboard focus on the same terminal.
    let keyboard_target = Some(tabs[2].item_id);
    tabs.remove(0);
    assert_eq!(
        project_popover_selection_index(&tabs, keyboard_target),
        Some(1)
    );
    tabs.pop();
    assert_eq!(
        project_popover_selection_index(&tabs, keyboard_target),
        Some(0)
    );
    tabs[0].active = false;
    assert_eq!(project_popover_selection_index(&tabs, None), None);
}

#[test]
fn marquee_reverses_from_its_current_position() {
    let now = Instant::now();
    let returning = MarqueeState {
        from: 80.,
        to: 0.,
        started_at: Some(now - Duration::from_millis(130)),
        duration: Duration::from_millis(260),
        ..Default::default()
    };
    let current = marquee_offset(returning, now);
    assert!(current > 0. && current < 80.);
    let resuming = MarqueeState {
        from: current,
        to: 80.,
        started_at: Some(now),
        duration: marquee_duration(80. - current, true),
        ..Default::default()
    };
    assert_eq!(marquee_offset(resuming, now), current);
}

#[test]
fn scrollbar_thumb_hidden_without_scrollback() {
    // No history → no thumb (scrollbar is hidden entirely).
    assert_eq!(scrollbar_thumb_geometry(400., 0, 24, 0), None);
    // Zero-height track is degenerate.
    assert_eq!(scrollbar_thumb_geometry(0., 100, 24, 0), None);
}

#[test]
fn a_remote_directory_is_shown_with_its_host() {
    let mut session = test_session();
    session.reported_location = Some(eggie_protocol::TerminalReportedLocation {
        user: Some("deploy".to_owned()),
        host: Some("prod.example.com".to_owned()),
        path: "/srv/www".to_owned(),
        local: false,
    });
    assert_eq!(
        displayed_directory(&session),
        "deploy@prod.example.com:/srv/www",
        "a remote path is shown with its host so it is not mistaken for a local path"
    );
}

#[test]
fn a_local_directory_is_shown_as_a_plain_path() {
    let mut session = test_session();
    session.current_directory = std::path::PathBuf::from("/Users/alice/project");
    assert_eq!(
        displayed_directory(&session),
        "/Users/alice/project",
        "a local path is shown without a host"
    );
}

/// A minimal session for display tests, with only the fields the tests read.
fn test_session() -> eggie_protocol::SessionSummary {
    eggie_protocol::SessionSummary {
        id: uuid::Uuid::nil(),
        project_id: uuid::Uuid::nil(),

        title: String::new(),
        initial_directory: std::path::PathBuf::from("/"),
        current_directory: std::path::PathBuf::from("/"),
        reported_location: None,
        shell_pid: 0,
        current_process: eggie_protocol::ProcessSummary {
            pid: 0,
            name: String::new(),
            command_line: None,
        },
        active_command: None,
        program_status: None,
        remote: None,
        status: eggie_protocol::SessionStatus::Running,
    }
}



#[test]
fn scrollbar_thumb_sits_at_bottom_and_top_at_the_extremes() {
    let track = 400.;
    let (history, rows) = (100u32, 25u16);
    let thumb = scrollbar_thumb_geometry(track, history, rows, 0).unwrap();
    let travel = track - thumb.height;
    // D = 0 (live bottom) → thumb flush bottom.
    assert!((thumb.top - travel).abs() < 0.01, "bottom: {thumb:?}");
    // D = H (scrolled to top) → thumb flush top.
    let top = scrollbar_thumb_geometry(track, history, rows, history).unwrap();
    assert!(top.top.abs() < 0.01, "top: {top:?}");
    // Thumb represents R/(H+R) of the track: 25/125 = 0.2 → 80px.
    assert!((thumb.height - 80.).abs() < 0.01, "height: {thumb:?}");
}

#[test]
fn scrollbar_thumb_respects_minimum_height() {
    // Huge history would give a sub-pixel thumb; clamp to the minimum.
    let thumb = scrollbar_thumb_geometry(400., 1_000_000, 24, 0).unwrap();
    assert!((thumb.height - TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT).abs() < 0.01);
}

#[test]
fn scrollbar_offset_inverts_thumb_top() {
    let track = 400.;
    let (history, rows) = (100u32, 25u16);
    // Round-trip a few offsets through geometry → thumb top → offset.
    for offset in [0u32, 25, 50, 75, 100] {
        let thumb = scrollbar_thumb_geometry(track, history, rows, offset).unwrap();
        let recovered = scrollbar_offset_from_thumb_top(track, history, rows, thumb.top);
        assert_eq!(recovered, offset, "offset {offset} did not round-trip");
    }
}

#[test]
fn scrollbar_offset_clamps_out_of_range_positions() {
    let track = 400.;
    let (history, rows) = (100u32, 25u16);
    // Above the track → oldest line (full history). Below → live bottom (0).
    assert_eq!(
        scrollbar_offset_from_thumb_top(track, history, rows, -50.),
        history
    );
    assert_eq!(
        scrollbar_offset_from_thumb_top(track, history, rows, 10_000.),
        0
    );
}

fn process(pid: u32, parent_pid: Option<u32>, name: &str) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid,
        name: name.to_owned(),
        cpu_usage_tenths_percent: Some(0),
        memory_bytes: Some(0),
    }
}

#[test]
fn terminal_container_rounding_respects_sidebars_and_split_edges() {
    let all_rounded = TerminalContainerCorners {
        top_left: true,
        top_right: true,
        bottom_left: true,
        bottom_right: true,
    };
    let square_bottom = TerminalContainerCorners {
        top_left: true,
        top_right: true,
        bottom_left: false,
        bottom_right: false,
    };
    let square_left = TerminalContainerCorners {
        top_left: false,
        top_right: true,
        bottom_left: false,
        bottom_right: true,
    };
    let square_right = TerminalContainerCorners {
        top_left: true,
        top_right: false,
        bottom_left: true,
        bottom_right: false,
    };

    // Both sidebars separate the root terminal from the window, but its bottom still touches.
    assert_eq!(
        terminal_container_corners(true, true, true, false, false),
        square_bottom,
    );
    // A top group in a vertical split is also separated from the bottom window edge.
    assert_eq!(
        terminal_container_corners(true, true, false, false, false),
        all_rounded,
    );
    // A collapsed left sidebar squares both corners on that window edge.
    assert_eq!(
        terminal_container_corners(true, true, false, true, false),
        square_left,
    );
    // The two terminal corners meeting at a horizontal split stay square.
    assert_eq!(
        terminal_container_corners(true, false, false, false, false),
        square_right,
    );
    assert_eq!(
        terminal_container_corners(false, true, false, false, false),
        square_left,
    );
    // With every exposed root edge touching the window, no corner is rounded.
    assert_eq!(
        terminal_container_corners(true, true, true, true, true),
        TerminalContainerCorners {
            top_left: false,
            top_right: false,
            bottom_left: false,
            bottom_right: false,
        },
    );
}

#[test]
fn terminal_container_owns_only_its_content_edge_borders() {
    let no_borders = TerminalContainerBorders {
        left: false,
        right: false,
        bottom: false,
    };

    // A root terminal touching the window on every exposed side has no outer side borders.
    assert_eq!(
        terminal_container_borders(true, true, true, true, true),
        no_borders,
    );
    // At a horizontal split, the left-hand terminal owns the internal vertical seam.
    assert_eq!(
        terminal_container_borders(true, false, true, false, false),
        TerminalContainerBorders {
            left: true,
            right: true,
            bottom: false,
        },
    );
    assert_eq!(
        terminal_container_borders(false, true, true, false, false),
        TerminalContainerBorders {
            left: false,
            right: true,
            bottom: false,
        },
    );
    // At a vertical split, the upper terminal owns the internal horizontal seam.
    assert_eq!(
        terminal_container_borders(true, true, false, false, false),
        TerminalContainerBorders {
            left: true,
            right: true,
            bottom: true,
        },
    );
}

#[test]
fn every_split_groups_active_terminal_is_visible() {
    let first_session = Uuid::new_v4();
    let hidden_session = Uuid::new_v4();
    let right_session = Uuid::new_v4();
    let first_item = TabItem::terminal(first_session, "first");
    let first_item_id = first_item.id;
    let mut layout = LayoutNode::group(first_item);
    let first_group_id = layout.first_group_id();
    layout
        .find_group_mut(first_group_id)
        .expect("first group")
        .add_item(TabItem::terminal(hidden_session, "hidden"));
    layout
        .split_group_direction(
            first_group_id,
            Direction::Right,
            TabItem::terminal(right_session, "right"),
        )
        .expect("right group");

    let visible = visible_active_terminal_sessions(&layout)
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(visible, HashSet::from([hidden_session, right_session]));
    assert!(!layout_has_visible_active_terminal(&layout, first_session));
    assert!(layout_has_visible_active_terminal(&layout, right_session));

    layout
        .find_group_mut(first_group_id)
        .expect("first group")
        .active_item_id = Some(first_item_id);
    let visible = visible_active_terminal_sessions(&layout)
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(visible, HashSet::from([first_session, right_session]));
    assert!(!layout_has_visible_active_terminal(&layout, hidden_session));
}

fn key_event(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        },
        is_held: false,
        prefer_character_input: false,
    }
}

#[test]
fn terminal_size_matches_the_visible_cell_grid() {
    assert_eq!(
        terminal_size_for_viewport(800., 360., 8., 18., 1.),
        TerminalSize {
            columns: 100,
            rows: 20,
            cell_width: 8,
            cell_height: 18,
        }
    );
    assert_eq!(
        terminal_size_for_viewport(1., 1., 8., 18., 1.),
        TerminalSize {
            columns: 2,
            rows: 1,
            cell_width: 8,
            cell_height: 18,
        }
    );
    assert_eq!(
        terminal_size_for_viewport(800., 360., 8.5, 18., 2.),
        TerminalSize {
            columns: 94,
            rows: 20,
            cell_width: 17,
            cell_height: 36,
        }
    );
}

#[test]
fn processes_are_flattened_as_a_stable_parent_child_tree() {
    let processes = vec![
        process(13, Some(11), "worker"),
        process(10, Some(1), "zsh"),
        process(12, Some(10), "bash-b"),
        process(11, Some(10), "bash-a"),
        process(14, Some(12), "ruby"),
    ];

    let rows = process_tree_rows(&processes);
    assert_eq!(
        rows.iter()
            .map(|row| processes[row.process_index].pid)
            .collect::<Vec<_>>(),
        [10, 11, 13, 12, 14]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.branch_has_next.clone())
            .collect::<Vec<_>>(),
        [
            vec![],
            vec![true],
            vec![true, false],
            vec![false],
            vec![false, false]
        ]
    );
}

#[test]
fn malformed_process_cycles_are_rendered_once_instead_of_recursing_forever() {
    let processes = vec![process(20, Some(21), "a"), process(21, Some(20), "b")];
    let rows = process_tree_rows(&processes);

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .map(|row| processes[row.process_index].pid)
            .collect::<HashSet<_>>(),
        HashSet::from([20, 21])
    );
}

#[test]
fn process_metrics_use_compact_sidebar_units() {
    assert_eq!(format_cpu_usage(Some(123)), "12.3%");
    assert_eq!(format_cpu_usage(None), "—");
    assert_eq!(format_memory_usage(Some(512)), "512B");
    assert_eq!(format_memory_usage(Some(12 * 1024)), "12KB");
    assert_eq!(format_memory_usage(Some(12 * 1024 * 1024)), "12.0MB");
    assert_eq!(format_memory_usage(None), "—");

    let mut process = process(42, None, "node");
    process.cpu_usage_tenths_percent = Some(12);
    process.memory_bytes = Some(1536 * 1024);
    assert_eq!(format_process_metrics(&process), "1.2% / 1.5MB");
}

#[test]
fn listening_port_owners_use_compact_process_pid_format() {
    assert_eq!(listening_port_owner(41_238, Some("node")), "node(41238)");
    assert_eq!(listening_port_owner(41_238, None), "41238");
}

#[test]
fn ports_section_is_omitted_when_nothing_is_listening() {
    let inspection = SessionInspection {
        port_mappings: None,
        session_id: Uuid::nil(),
        processes: Vec::new(),
        ports: Vec::new(),
    };
    let colors = UiColors {
        background: 0,
        panel: 0,
        panel_alt: 0,
        hover: 0,
        border: 0,
        text: 0,
        muted: 0,
        accent: 0,
        accent_foreground: 0,
        accent_hover: 0,
    };

    assert!(ports_section(&inspection, colors, Language::English).is_none());
}

#[test]
fn terminal_input_preserves_non_bmp_utf8_text() {
    let text = "你好🙂👩🏽‍💻";
    assert_eq!(terminal_text_bytes(text), text.as_bytes());
    assert_eq!(
        terminal_bytes(&key_event("🙂", Some("🙂"), Modifiers::default())),
        Some("🙂".as_bytes().to_vec())
    );
}

#[test]
fn terminal_input_does_not_forward_appkit_reserved_characters() {
    for character in ['\u{f700}', '\u{f8ff}', '\u{fdd0}', '\u{fffe}', '\u{ffff}'] {
        let text = character.to_string();
        assert_eq!(
            terminal_bytes(&key_event(&text, Some(&text), Modifiers::default())),
            None,
            "reserved key character U+{:04X} must stay in AppKit",
            character as u32
        );
    }

    assert_eq!(
        terminal_bytes(&key_event(
            "e",
            Some("e"),
            Modifiers {
                function: true,
                ..Modifiers::default()
            },
        )),
        None
    );
}

#[test]
fn terminal_input_keeps_known_navigation_keys_with_function_modifier() {
    assert_eq!(
        terminal_bytes(&key_event(
            "up",
            None,
            Modifiers {
                function: true,
                ..Modifiers::default()
            },
        )),
        Some(b"\x1b[A".to_vec())
    );
}

#[test]
fn kitty_keyboard_mode_reports_press_repeat_release_and_associated_text() {
    let flags = KITTY_DISAMBIGUATE_ESCAPE_CODES
        | KITTY_REPORT_EVENT_TYPES
        | KITTY_REPORT_ALL_KEYS
        | KITTY_REPORT_ASSOCIATED_TEXT;
    let control_c = key_event(
        "c",
        Some("c"),
        Modifiers {
            control: true,
            ..Modifiers::default()
        },
    );
    assert_eq!(
        terminal_bytes_with_mode(&control_c, flags),
        Some(b"\x1b[99;5:1u".to_vec())
    );
    assert_eq!(
        kitty_key_bytes(&control_c.keystroke, flags, 2),
        Some(b"\x1b[99;5:2u".to_vec())
    );
    assert_eq!(
        kitty_key_bytes(&control_c.keystroke, flags, 3),
        Some(b"\x1b[99;5:3u".to_vec())
    );
    assert_eq!(
        terminal_bytes_with_mode(&key_event("up", None, Modifiers::default()), flags,),
        Some(b"\x1b[1;1:1A".to_vec())
    );
    assert_eq!(
        terminal_bytes_with_mode(
            &key_event(
                "up",
                None,
                Modifiers {
                    function: true,
                    ..Modifiers::default()
                }
            ),
            flags,
        ),
        Some(b"\x1b[1;1:1A".to_vec())
    );
}

#[test]
fn macos_character_palette_shortcut_is_recognized() {
    assert!(is_character_palette_shortcut(&key_event(
        "space",
        Some(" "),
        Modifiers {
            control: true,
            platform: true,
            ..Modifiers::default()
        },
    )));
    assert!(!is_character_palette_shortcut(&key_event(
        "space",
        Some(" "),
        Modifiers {
            control: true,
            ..Modifiers::default()
        },
    )));
}

#[test]
fn terminal_mouse_coordinates_clamp_to_the_visible_grid() {
    let session_id = Uuid::nil();
    let viewport = TerminalViewport {
        session_id,
        bounds: Bounds::new(point(px(20.), px(40.)), size(px(80.), px(36.))),
        cell_width: 8.,
        line_height: 18.,
        rows: 2,
        columns: 10,
    };
    assert_eq!(
        terminal_point_from_position(viewport, point(px(44.), px(59.))),
        Some(TerminalPoint { line: 1, column: 3 })
    );
    assert_eq!(
        terminal_point_from_position(viewport, point(px(-100.), px(1000.))),
        Some(TerminalPoint { line: 1, column: 0 })
    );
}

#[test]
fn terminal_pointer_protocol_conversion_preserves_buttons_modifiers_and_subpixels() {
    assert_eq!(
        protocol_mouse_button(MouseButton::Left),
        Some(TerminalMouseButton::Left)
    );
    assert_eq!(
        protocol_mouse_button(MouseButton::Middle),
        Some(TerminalMouseButton::Middle)
    );
    assert_eq!(
        protocol_mouse_button(MouseButton::Right),
        Some(TerminalMouseButton::Right)
    );
    assert_eq!(
        protocol_mouse_button(MouseButton::Navigate(gpui::NavigationDirection::Back)),
        None
    );
    assert_eq!(
        protocol_terminal_modifiers(gpui::Modifiers {
            control: true,
            alt: true,
            shift: true,
            platform: true,
            function: true,
        }),
        TerminalModifiers {
            control: true,
            alt: true,
            shift: true,
        }
    );
    assert_eq!(fixed_terminal_scroll_delta(0.5), 512);
    assert_eq!(fixed_terminal_scroll_delta(-1.25), -1280);
    assert_eq!(fixed_terminal_scroll_delta(f32::NAN), 0);
    assert_eq!(fixed_terminal_scroll_delta(f32::MAX), i32::MAX);
}

#[test]
fn a_sole_tab_dragged_over_its_own_content_has_no_drop_target() {
    let group_id = GroupId::new_v4();

    assert!(!should_show_content_drop_target(group_id, group_id, 1));
    assert!(should_show_content_drop_target(group_id, group_id, 2));
    assert!(should_show_content_drop_target(
        group_id,
        GroupId::new_v4(),
        1
    ));
}

#[test]
fn tab_bar_uses_each_tabs_halfway_point_as_its_insertion_boundary() {
    assert_eq!(tab_bar_insertion_index(2, 100., 50.), 2);
    assert_eq!(tab_bar_insertion_index(2, 100., 50.1), 3);
}

#[test]
fn content_drop_zones_follow_vscode_style_edge_thresholds() {
    assert_eq!(
        content_drop_zone(1000., 600., 500., 300.),
        TabDropZone::Center
    );
    assert_eq!(
        content_drop_zone(1000., 600., 50., 300.),
        TabDropZone::Edge(Direction::Left)
    );
    assert_eq!(
        content_drop_zone(1000., 600., 950., 300.),
        TabDropZone::Edge(Direction::Right)
    );
    assert_eq!(
        content_drop_zone(1000., 600., 500., 30.),
        TabDropZone::Edge(Direction::Up)
    );
    assert_eq!(
        content_drop_zone(1000., 600., 500., 570.),
        TabDropZone::Edge(Direction::Down)
    );
}

#[test]
fn progress_labels_include_status_percent_and_recency() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let label = progress_label(
        TerminalProgress {
            state: TerminalProgressState::Paused,
            percent: Some(73),
            updated_at_unix_ms: now.saturating_sub(3_000),
        },
        Language::English,
    );
    assert_eq!(label, "Paused · 73% · updated 3s ago");

    let completed = progress_label(
        TerminalProgress {
            state: TerminalProgressState::Normal,
            percent: Some(100),
            updated_at_unix_ms: now,
        },
        Language::English,
    );
    assert!(completed.starts_with("Complete · 100% · updated"));
}

#[test]
fn program_status_uses_the_single_icon_indicator_before_osc_progress() {
    let progress = TerminalProgress {
        state: TerminalProgressState::Indeterminate,
        percent: None,
        updated_at_unix_ms: 0,
    };
    let mut status = TerminalProgramStatus {
        state: TerminalProgramState::Working,
        kind: None,
        app: None,
        title: None,
        message: None,
        progress: None,
    };
    assert_eq!(
        icon_indicator(None, Some(progress)),
        Some(IconIndicator::Progress(progress))
    );
    assert_eq!(
        icon_indicator(Some(&status), Some(progress)),
        Some(IconIndicator::Working(None))
    );
    assert!(
        icon_indicator(Some(&status), Some(progress))
            .unwrap()
            .is_animated()
    );

    status.state = TerminalProgramState::Blocked;
    assert_eq!(
        icon_indicator(Some(&status), Some(progress)),
        Some(IconIndicator::Attention(TerminalProgramState::Blocked))
    );
    assert!(
        !icon_indicator(Some(&status), Some(progress))
            .unwrap()
            .is_animated()
    );

    status.state = TerminalProgramState::Idle;
    assert_eq!(
        icon_indicator(Some(&status), Some(progress)),
        Some(IconIndicator::Progress(progress))
    );
}

#[test]
fn new_terminal_cwd_inherits_live_directory_and_falls_back_safely() {
    // RAII cleanup so a failed assert never leaks the temp directories.
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for dir in &self.0 {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    let root = std::env::temp_dir().join(format!(
        "eggie-cwd-inherit-test-{}-root",
        std::process::id()
    ));
    let live = std::env::temp_dir().join(format!(
        "eggie-cwd-inherit-test-{}-live",
        std::process::id()
    ));
    let _cleanup = Cleanup(vec![root.clone(), live.clone()]);
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::create_dir_all(&live);

    // A live directory is inherited as-is.
    assert_eq!(
        resolve_new_terminal_cwd(Some(&live), &root),
        live,
        "new terminals should inherit the active session's cwd"
    );

    // A deleted directory falls back to the project root instead of failing the PTY spawn.
    std::fs::remove_dir_all(&live).unwrap();
    assert_eq!(
        resolve_new_terminal_cwd(Some(&live), &root),
        root,
        "a vanished cwd must fall back to the project root"
    );

    // A path that exists but is not a directory (e.g. a swapped-in file) falls back too.
    let _ = std::fs::write(&live, b"not a directory");
    assert_eq!(
        resolve_new_terminal_cwd(Some(&live), &root),
        root,
        "a non-directory cwd must fall back to the project root"
    );
    let _ = std::fs::remove_file(&live);

    // No active session (empty group / brand-new project) uses the project root.
    assert_eq!(resolve_new_terminal_cwd(None, &root), root);
}

#[test]
fn the_session_set_key_distinguishes_a_swap_from_no_change() {
    let a = SessionId::new_v4();
    let b = SessionId::new_v4();
    let c = SessionId::new_v4();

    let two = session_set_key([a, b].into_iter());
    // Identical sets key identically, so a steady-state repaint skips the reconcile.
    assert_eq!(two, session_set_key([a, b].into_iter()));
    // Order must not matter: reordering tabs cannot change any session's profile, so it must not
    // invalidate the gate and trigger a pointless O(sessions × tabs) walk.
    assert_eq!(two, session_set_key([b, a].into_iter()));
    // The case a plain count would miss: one tab closed, another opened, same total.
    assert_ne!(
        two,
        session_set_key([a, c].into_iter()),
        "swapping a session must invalidate the gate, or the new terminal keeps the old palette"
    );
    // Adding and removing are both visible.
    assert_ne!(two, session_set_key([a, b, c].into_iter()));
    assert_ne!(two, session_set_key([a].into_iter()));
    assert_eq!(session_set_key(std::iter::empty()), 0);
}

#[test]
fn spawn_config_takes_launch_fields_from_the_profile() {
    let mut profile = Profile::new("Fish".to_owned());
    profile.shell_program = "/opt/homebrew/bin/fish".to_owned();
    profile.shell_args = vec!["-l".to_owned(), "-C".to_owned(), "echo hi".to_owned()];
    profile
        .env
        .insert("AWS_PROFILE".to_owned(), "prod".to_owned());

    let root = PathBuf::from("/tmp");
    let spawn = resolve_spawn_config(&profile, None, &root);
    assert_eq!(spawn.shell_program, "/opt/homebrew/bin/fish");
    assert_eq!(spawn.shell_args, profile.shell_args);
    assert_eq!(
        spawn.env.get("AWS_PROFILE").map(String::as_str),
        Some("prod")
    );
    // The `path` feature defaults on, so the assembled token list carries it.
    assert_eq!(spawn.shell_features, "path");

    // Turning the profile's own toggle off drops the token — a remote/container profile has no
    // local eggie binary worth putting on PATH.
    profile.shell_integration_path = false;
    assert_eq!(
        resolve_spawn_config(&profile, None, &root).shell_features,
        "",
    );
}

#[test]
fn spawn_config_cwd_precedence_prefers_profile_then_inheritance() {
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for dir in &self.0 {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    let base = std::env::temp_dir().join(format!("eggie-spawn-cwd-{}", std::process::id()));
    let root = base.join("root");
    let live = base.join("live");
    let pinned = base.join("pinned");
    let _cleanup = Cleanup(vec![base.clone()]);
    for dir in [&root, &live, &pinned] {
        std::fs::create_dir_all(dir).expect("create temp dirs");
    }

    let mut profile = Profile::new("P".to_owned());

    // No profile cwd: inherit the live directory (the step-1 behavior is preserved).
    assert_eq!(resolve_spawn_config(&profile, Some(&live), &root).cwd, live);

    // An explicit profile cwd wins over inheritance — it names a fixed directory on purpose.
    profile.cwd = Some(pinned.clone());
    assert_eq!(
        resolve_spawn_config(&profile, Some(&live), &root).cwd,
        pinned
    );

    // A profile cwd that no longer exists falls back rather than failing the spawn.
    std::fs::remove_dir_all(&pinned).expect("remove pinned dir");
    assert_eq!(
        resolve_spawn_config(&profile, Some(&live), &root).cwd,
        live,
        "a missing profile cwd falls back to the inherited directory"
    );
    assert_eq!(
        resolve_spawn_config(&profile, None, &root).cwd,
        root,
        "with nothing to inherit it falls back to the project root"
    );
}

#[test]
fn spawn_config_drops_unusable_env_keys() {
    let mut profile = Profile::new("Sloppy".to_owned());
    // A hand-edited profiles.json can contain any of these; each would otherwise either fail the
    // PTY spawn with an opaque error (NUL) or create an unreferenceable variable.
    profile.env.insert("GOOD".to_owned(), "yes".to_owned());
    profile.env.insert(String::new(), "empty key".to_owned());
    profile.env.insert("HAS=EQUALS".to_owned(), "x".to_owned());
    profile.env.insert("HAS\0NUL".to_owned(), "x".to_owned());

    let spawn = resolve_spawn_config(&profile, None, Path::new("/tmp"));
    assert_eq!(spawn.env.len(), 1, "only the usable key survives");
    assert_eq!(spawn.env.get("GOOD").map(String::as_str), Some("yes"));
}

#[test]
fn a_remote_profiles_cwd_is_not_used_as_the_local_working_directory() {
    let root = std::env::temp_dir().join(format!("eggie-remote-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create temp dir");

    let mut profile = Profile::new("Prod".to_owned());
    profile.remote = Some(eggie_domain::RemoteTarget::new(
        "prod.example.com".to_owned(),
    ));
    // A path that means something on the remote host and (almost certainly) nothing here.
    profile.cwd = Some(PathBuf::from("/srv/www"));

    // The local PTY must start somewhere local; the remote directory is the bootstrap's job.
    assert_eq!(
        resolve_spawn_config(&profile, None, &root).cwd,
        root,
        "a remote profile's cwd describes the far side and must not be sent as the local cwd"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ssh_directory_inheritance_requires_the_same_target_and_uses_remote_paths() {
    let mut source = test_session();
    let target = eggie_domain::RemoteTarget::new("devbox".into());
    source.remote = Some(eggie_protocol::RemoteSessionInfo {
        target: target.clone(), phase: eggie_protocol::RemoteSessionPhase::Connected,
        initial_directory: Some("/home/remote".into()),
    });
    source.current_directory = "/Users/local".into();
    let mut profile = Profile::new("SSH".into());
    profile.remote = Some(target);
    assert_eq!(ssh::inherited_remote_directory(&profile, Some(&source)).as_deref(), Some("/home/remote"));
    source.reported_location = Some(eggie_protocol::TerminalReportedLocation {
        path: "/srv/project/subdir".into(), host: Some("devbox".into()), user: None, local: false,
    });
    assert_eq!(ssh::inherited_remote_directory(&profile, Some(&source)).as_deref(), Some("/srv/project/subdir"));
    assert_eq!(displayed_directory(&source), "/srv/project/subdir");
    profile.remote.as_mut().unwrap().host = "another-host".into();
    assert_eq!(ssh::inherited_remote_directory(&profile, Some(&source)), None);
    profile.remote = None;
    assert_eq!(ssh::inherited_remote_directory(&profile, Some(&source)), None);
}
