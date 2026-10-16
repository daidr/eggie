use super::*;
use uuid::Uuid;

/// Minimal snapshot carrying only the fields `plain_lines`/`has_scheme_candidate` read.
fn snapshot_with_cells(columns: u16, rows: u16, cells: Vec<TerminalCell>) -> TerminalSnapshot {
    TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        session_id: Uuid::nil(),
        size: TerminalSize {
            columns,
            rows,
            cell_width: 0,
            cell_height: 0,
        },
        cells: cells.into(),
        color_overrides: Vec::new(),
        cursor_line: 0,
        cursor_column: 0,
        cursor_shape: TerminalCursorShape::Hidden,
        cursor_width: 1,
        cursor_blinking: false,
        title: String::new(),
        revision: 0,
        last_input_sequence: 0,
        input_modes: TerminalInputModes::default(),
        images: Vec::new(),
        image_placements: Vec::new(),
        selection: None,
        detected_links: Vec::new(),
        display_offset: 0,
        history_size: 0,
    }
}

/// Row-major, ascending-column cells spelling `text` on `line` starting at column 0, mirroring
/// how the grid iterator emits a rendered line (empty cells omitted, so spaces create column
/// gaps just as the daemon's snapshot builder produces).
fn row_cells(line: u16, text: &str) -> Vec<TerminalCell> {
    text.chars()
        .enumerate()
        .filter(|(_, character)| *character != ' ')
        .map(|(column, character)| TerminalCell {
            line,
            column: column as u16,
            character,
            zerowidth: Vec::new(),
            foreground: TerminalColor::Indexed(0),
            background: TerminalColor::Indexed(0),
            underline_color: None,
            hyperlink: None,
            flags: 0,
        })
        .collect()
}

#[test]
fn has_scheme_candidate_matches_plain_lines_precheck() {
    // (columns, rows, per-row text). Each row's cells are built sparse (spaces omitted), so the
    // ` : / /` cases exercise the column-gap break that must NOT match `://`.
    let cases: &[(u16, u16, &[&str])] = &[
        (20, 1, &["visit https://x.io"]),    // scheme separator present
        (20, 1, &["plain text no urls"]),    // none
        (20, 1, &["mailto:me@example.com"]), // mailto needle
        (20, 1, &["scheme : // spaced"]),    // gaps between :,/,/ -> no match
        (10, 2, &["nothing", "a://b"]),      // match on a later row
        (8, 1, &["::://"]),                  // overlapping colons then //
        (5, 1, &["a://"]),                   // match at window edge
        (3, 1, &["://"]),                    // exactly the needle, full width
        (20, 1, &["trailing://   "]),        // match before trimmed trailing space
    ];
    for (columns, rows, lines) in cases {
        let cells: Vec<TerminalCell> = lines
            .iter()
            .enumerate()
            .flat_map(|(line, text)| row_cells(line as u16, text))
            .collect();
        let snapshot = snapshot_with_cells(*columns, *rows, cells);
        let expected = snapshot
            .plain_lines()
            .iter()
            .any(|line| line.contains("://") || line.contains("mailto:"));
        assert_eq!(
            snapshot.has_scheme_candidate(),
            expected,
            "mismatch for {lines:?}"
        );
    }
}

#[test]
fn request_round_trips_as_a_single_json_line() {
    for request in [
        ClientRequest::Input {
            session_id: Uuid::nil(),
            bytes: b"echo hello\r".to_vec(),
            sequence: 7,
        },
        ClientRequest::Paste {
            session_id: Uuid::nil(),
            text: "echo 界\n".to_owned(),
            sequence: 8,
        },
        ClientRequest::PasteClipboard {
            session_id: Uuid::nil(),
            selection: TerminalClipboardSelection::Clipboard,
            contents: vec![TerminalClipboardContent {
                mime_type: "text/plain;charset=utf-8".to_owned(),
                data: b"rich paste".to_vec(),
            }],
            sequence: 9,
            expected_context: None,
        },
        ClientRequest::Mouse {
            session_id: Uuid::nil(),
            event: TerminalMouseEvent {
                action: TerminalMouseAction::Press,
                button: Some(TerminalMouseButton::Left),
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers {
                    shift: false,
                    alt: true,
                    control: false,
                },
            },
        },
        ClientRequest::Scroll {
            session_id: Uuid::nil(),
            event: TerminalScrollEvent {
                delta: TerminalScrollDelta {
                    x: 0,
                    y: TERMINAL_SCROLL_DELTA_SCALE,
                    unit: TerminalScrollUnit::Lines,
                },
                phase: TerminalScrollPhase::Moved,
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            },
        },
        ClientRequest::TerminalSearch {
            session_id: Uuid::nil(),
            request: TerminalSearchRequest {
                query: "需要 find".to_owned(),
                regex: true,
                direction: TerminalSearchDirection::Backward,
                fresh: true,
            },
        },
        ClientRequest::Focus {
            session_id: Uuid::nil(),
            focused: true,
        },
        ClientRequest::WaitForSnapshot {
            session_id: Uuid::nil(),
            after_revision: 42,
            timeout_ms: 50,
        },
        ClientRequest::WaitForProgress {
            session_id: Uuid::nil(),
            after_revision: 7,
            timeout_ms: 50,
        },
        ClientRequest::WaitForOscEvents {
            session_id: Uuid::nil(),
            after_revision: 3,
            timeout_ms: 50,
        },
        ClientRequest::GetShellIntegrationState {
            session_id: Uuid::nil(),
        },
        ClientRequest::CompleteClipboardRead {
            session_id: Uuid::nil(),
            request_id: 11,
            contents: vec![TerminalClipboardContent {
                mime_type: "text/plain".to_owned(),
                data: b"clipboard".to_vec(),
            }],
        },
        ClientRequest::NotificationResponse {
            session_id: Uuid::nil(),
            notification_id: "build-finished".to_owned(),
            activated: true,
        },
        ClientRequest::CompleteFileTransfer {
            session_id: Uuid::nil(),
            request_id: 12,
            destination: Some(PathBuf::from("/tmp/download")),
        },
        ClientRequest::SetProgressTimeouts {
            session_id: Uuid::nil(),
            timeouts: TerminalProgressTimeouts {
                completed_ms: 5_000,
                stale_ms: 60_000,
            },
        },
        ClientRequest::TerminalImage {
            session_id: Uuid::nil(),
            key: TerminalImageKey {
                id: 7,
                generation: 9,
            },
            offset: 4096,
            length: 1024,
        },
        ClientRequest::SetOscPolicy {
            session_id: Uuid::nil(),
            allow_clipboard_read: true,
        },
    ] {
        let encoded = encode_line(&request).unwrap();
        assert_eq!(encoded.last(), Some(&b'\n'));
        let decoded: ClientRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, request);
    }
}

#[test]
fn terminal_search_request_omitted_fields_use_defaults() {
    // A request that specifies only the query must decode, defaulting regex/fresh to false and
    // direction to Forward, so partial or older-client payloads don't hard-fail.
    let decoded: TerminalSearchRequest = serde_json::from_str(r#"{"query":"foo"}"#).unwrap();
    assert_eq!(decoded.query, "foo");
    assert!(!decoded.regex);
    assert!(!decoded.fresh);
    assert_eq!(decoded.direction, TerminalSearchDirection::Forward);
}

#[test]
fn terminal_selection_requests_and_reply_round_trip() {
    for request in [
        ClientRequest::TerminalSelectionStart {
            session_id: Uuid::nil(),
            point: TerminalCellPosition { line: 3, column: 7 },
            side: TerminalSelectionSide::Right,
            kind: TerminalSelectionKind::Semantic,
        },
        ClientRequest::TerminalSelectionUpdate {
            session_id: Uuid::nil(),
            point: TerminalCellPosition {
                line: 12,
                column: 0,
            },
            side: TerminalSelectionSide::Left,
        },
        ClientRequest::TerminalSelectionClear {
            session_id: Uuid::nil(),
        },
        ClientRequest::TerminalSelectAll {
            session_id: Uuid::nil(),
        },
        ClientRequest::TerminalCopySelection {
            session_id: Uuid::nil(),
        },
    ] {
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }

    let reply = DaemonResponse::SelectionText {
        session_id: Uuid::nil(),
        text: Some("hello\nworld".to_owned()),
    };
    let encoded = encode_line(&reply).unwrap();
    assert_eq!(
        serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
        reply
    );
}

#[test]
fn terminal_scroll_to_commands_round_trip() {
    for command in [
        TerminalScrollCommand::Top,
        TerminalScrollCommand::Bottom,
        TerminalScrollCommand::PageUp,
        TerminalScrollCommand::PageDown,
        TerminalScrollCommand::PrevPrompt,
        TerminalScrollCommand::NextPrompt,
    ] {
        let request = ClientRequest::TerminalScrollTo {
            session_id: Uuid::nil(),
            command,
        };
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }
}

#[test]
fn terminal_scroll_to_offset_round_trips() {
    for offset in [0u32, 1, 200, 100_000, u32::MAX] {
        let request = ClientRequest::TerminalScrollToOffset {
            session_id: Uuid::nil(),
            offset,
        };
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }
}

#[test]
fn set_cursor_style_request_round_trips() {
    for shape in [
        TerminalCursorShape::Block,
        TerminalCursorShape::Underline,
        TerminalCursorShape::Beam,
        TerminalCursorShape::HollowBlock,
        TerminalCursorShape::Hidden,
    ] {
        let request = ClientRequest::SetCursorStyle {
            session_id: Uuid::nil(),
            shape,
        };
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }
}

#[test]
fn set_scrollback_limit_request_round_trips() {
    for limit in [0usize, 500, 10_000, 1_000_000] {
        let request = ClientRequest::SetScrollbackLimit {
            session_id: Uuid::nil(),
            limit,
        };
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }
}

#[test]
fn create_session_carries_scrollback_and_shell_overrides() {
    let request = ClientRequest::CreateSession {
        project_id: Uuid::nil(),

        cwd: PathBuf::from("/tmp"),
        size: TerminalSize::default(),
        appearance: TerminalAppearance::default(),
        scrollback_limit: 500,
        shell_program: "/opt/homebrew/bin/fish".to_owned(),
        shell_args: vec!["-l".to_owned(), "-c".to_owned(), "echo hi".to_owned()],
        shell_features: "path".to_owned(),
        env: std::collections::BTreeMap::from([
            ("AWS_PROFILE".to_owned(), "prod".to_owned()),
            // Values with punctuation and non-ASCII must survive both encodings.
            ("GREETING".to_owned(), "你好; world".to_owned()),
        ]),
        remote: Some(eggie_domain::RemoteTarget {
            host: "prod.example.com".to_owned(),
            user: Some("deploy".to_owned()),
            port: Some(2222),
        }),
        remote_install_consented: true,
        remote_cwd: Some("/srv/www".to_owned()),
    };
    let encoded = encode_line(&request).unwrap();
    assert_eq!(
        serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
        request
    );
    assert_eq!(
        rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
            .unwrap(),
        request
    );
}

#[test]
fn create_session_shell_overrides_default_to_empty_when_absent() {
    // A request omitting the optional shell fields (older client shape) decodes with empty/zero
    // defaults; project identity is required independently of any window.
    let appearance = serde_json::to_string(&TerminalAppearance::default()).unwrap();
    let json = format!(
        r#"{{"type":"create_session","project_id":"00000000-0000-0000-0000-000000000000","cwd":"/tmp","size":{{"columns":80,"rows":24,"cell_width":8,"cell_height":16}},"appearance":{appearance}}}"#
    );
    let decoded: ClientRequest = serde_json::from_str(&json).unwrap();
    match decoded {
        ClientRequest::CreateSession {
            scrollback_limit,
            shell_program,
            shell_args,
            shell_features,
            env,
            remote,
            remote_install_consented,
            remote_cwd,
            ..
        } => {
            assert_eq!(scrollback_limit, 0);
            assert!(shell_program.is_empty());
            assert!(shell_args.is_empty());
            // Absent field falls back to the default feature set (`path`), not empty.
            assert_eq!(shell_features, "path");
            // A client that predates profiles sends no env; the shell gets Eggie's own only.
            assert!(env.is_empty());
            // And no remote fields: a local session, with consent absent rather than assumed.
            assert!(
                remote.is_none(),
                "absent `remote` must mean a local session"
            );
            assert!(
                !remote_install_consented,
                "consent must never default to true — it authorizes writing to another host"
            );
            assert!(remote_cwd.is_none());
        }
        other => panic!("unexpected decode: {other:?}"),
    }
}

#[test]
fn bell_osc_event_round_trips() {
    let payload = TerminalOscEventPayload::Bell;
    let json = serde_json::to_string(&payload).unwrap();
    assert_eq!(json, r#"{"type":"bell"}"#);
    assert_eq!(
        serde_json::from_str::<TerminalOscEventPayload>(&json).unwrap(),
        payload
    );
    assert_eq!(
        rmp_serde::from_slice::<TerminalOscEventPayload>(
            &rmp_serde::to_vec_named(&payload).unwrap()
        )
        .unwrap(),
        payload
    );
}

#[test]
fn selection_side_and_kind_default_and_omitted_block_flag() {
    assert_eq!(
        TerminalSelectionSide::default(),
        TerminalSelectionSide::Left
    );
    assert_eq!(
        TerminalSelectionKind::default(),
        TerminalSelectionKind::Simple
    );
    // A range without the block flag omits "b" on the wire and decodes back to false.
    let range = TerminalSelectionRange {
        start: TerminalCellPosition { line: 0, column: 0 },
        end: TerminalCellPosition { line: 1, column: 4 },
        is_block: false,
    };
    let json = serde_json::to_string(&range).unwrap();
    assert!(!json.contains("\"b\""));
    assert_eq!(
        serde_json::from_str::<TerminalSelectionRange>(&json).unwrap(),
        range
    );
}

#[test]
fn snapshot_without_selection_key_decodes_to_none() {
    // An older daemon's snapshot has no "sel" key; it must decode with selection == None so a
    // version-skewed client does not hard-fail.
    let json = r#"{"id":"00000000-0000-0000-0000-000000000000","s":{"columns":80,"rows":24,"cell_width":8,"cell_height":16},"c":[],"cl":0,"cc":0,"cs":"block","cw":1,"t":"","r":1}"#;
    let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(snapshot.selection, None);
    assert!(snapshot.detected_links.is_empty());
}

#[test]
fn detected_links_round_trip_and_default_empty() {
    let link = TerminalLinkRange {
        start: TerminalCellPosition { line: 2, column: 6 },
        end: TerminalCellPosition {
            line: 2,
            column: 24,
        },
        url: "https://example.com/x".to_owned(),
    };
    let encoded = encode_line(&link).unwrap();
    assert_eq!(
        serde_json::from_slice::<TerminalLinkRange>(&encoded).unwrap(),
        link
    );
    // A snapshot without "dl" decodes to an empty link list.
    let json = r#"{"id":"00000000-0000-0000-0000-000000000000","s":{"columns":80,"rows":24,"cell_width":8,"cell_height":16},"c":[],"cl":0,"cc":0,"cs":"block","cw":1,"t":"","r":1}"#;
    let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
    assert!(snapshot.detected_links.is_empty());
}

#[test]
fn terminal_progress_update_round_trips() {
    let response = DaemonResponse::Progress {
        update: TerminalProgressUpdate {
            session_id: Uuid::nil(),
            revision: 9,
            progress: Some(TerminalProgress {
                state: TerminalProgressState::Paused,
                percent: Some(73),
                updated_at_unix_ms: 1_725_000_000_123,
            }),
        },
    };
    let encoded = encode_line(&response).unwrap();
    assert_eq!(
        serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
        response
    );
    assert_eq!(
        rmp_serde::from_slice::<DaemonResponse>(&rmp_serde::to_vec_named(&response).unwrap())
            .unwrap(),
        response
    );
}

#[test]
fn terminal_image_chunks_use_compact_base64_json_and_round_trip() {
    let response = DaemonResponse::TerminalImage {
        key: TerminalImageKey {
            id: 7,
            generation: 9,
        },
        width: 2,
        height: 1,
        total_length: 8,
        offset: 0,
        bytes: vec![0, 1, 2, 3, 252, 253, 254, 255],
    };
    let encoded = encode_line(&response).unwrap();
    let json = std::str::from_utf8(&encoded).unwrap();
    assert!(json.contains(r#""bytes":"AAECA/z9/v8=""#));
    assert!(!json.contains(r#""bytes":["#));
    assert_eq!(
        serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
        response
    );
}

#[test]
fn terminal_image_chunks_are_raw_binary_on_the_daemon_wire() {
    let bytes = vec![0x5a; 1024 * 1024];
    let response = DaemonResponse::TerminalImage {
        key: TerminalImageKey {
            id: 7,
            generation: 9,
        },
        width: 512,
        height: 512,
        total_length: bytes.len() as u32,
        offset: 0,
        bytes,
    };
    let encoded = rmp_serde::to_vec_named(&response).unwrap();
    assert!(
        encoded.len() < 1024 * 1024 + 256,
        "image chunk unexpectedly expanded to {} bytes",
        encoded.len()
    );
    assert_eq!(
        rmp_serde::from_slice::<DaemonResponse>(&encoded).unwrap(),
        response
    );
}

#[test]
fn process_metrics_are_additive_for_running_daemon_compatibility() {
    let process: ProcessInfo =
        serde_json::from_str(r#"{"pid":42,"parent_pid":1,"name":"zsh"}"#).unwrap();

    assert_eq!(process.cpu_usage_tenths_percent, None);
    assert_eq!(process.memory_bytes, None);
}

#[test]
fn handshake_without_a_build_id_is_treated_as_a_stale_daemon() {
    let response: DaemonResponse =
        serde_json::from_str(r#"{"type":"handshake_accepted","protocol_version":9}"#).unwrap();
    assert_eq!(
        response,
        DaemonResponse::HandshakeAccepted {
            protocol_version: 9,
            build_id: String::new(),
        }
    );
}

#[test]
fn render_snapshot_round_trips_without_losing_terminal_semantics() {
    let snapshot = TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        session_id: Uuid::nil(),
        size: TerminalSize {
            columns: 2,
            rows: 1,
            ..TerminalSize::default()
        },
        cells: vec![TerminalCell {
            line: 0,
            column: 0,
            character: 'e',
            zerowidth: vec!['\u{301}'],
            foreground: TerminalColor::Indexed(1),
            background: TerminalColor::Named(257),
            underline_color: Some(TerminalColor::Rgb(0x112233ff)),
            hyperlink: Some("https://example.com".to_owned()),
            flags: 0x4a0a,
        }]
        .into(),
        color_overrides: vec![TerminalColorOverride {
            index: 1,
            color: 0x123456ff,
        }],
        cursor_line: 0,
        cursor_column: 0,
        cursor_shape: TerminalCursorShape::Beam,
        cursor_width: 1,
        cursor_blinking: true,
        title: "semantic snapshot".to_owned(),
        revision: 42,
        last_input_sequence: 9,
        input_modes: TerminalInputModes {
            bracketed_paste: false,
            mouse_tracking: TerminalMouseTracking::Drag,
            mouse_encoding: TerminalMouseEncoding::Sgr,
            focus_reporting: true,
            alternate_screen: true,
            alternate_scroll: true,
            paste_events: false,
            kitty_keyboard_flags: 0,
        },
        images: vec![TerminalImageDescriptor {
            key: TerminalImageKey {
                id: 7,
                generation: 9,
            },
            width: 2,
            height: 1,
        }],
        image_placements: vec![TerminalImagePlacement {
            image: TerminalImageKey {
                id: 7,
                generation: 9,
            },
            placement_id: 3,
            line: 0,
            column: 1,
            source_x: 0,
            source_y: 0,
            source_width: 2,
            source_height: 1,
            x_offset: 0,
            y_offset: 0,
            columns: 1,
            rows: 1,
            destination_width: 8,
            destination_height: 4,
            z: -1,
        }],
        selection: None,
        detected_links: Vec::new(),
        display_offset: 37,
        history_size: 512,
    };
    let encoded = encode_line(&DaemonResponse::Snapshot {
        snapshot: Arc::new(snapshot.clone()),
    })
    .unwrap();
    let decoded: DaemonResponse = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded,
        DaemonResponse::Snapshot {
            snapshot: Arc::new(snapshot)
        }
    );
}

#[test]
fn snapshot_cursor_blinking_defaults_to_false_when_absent() {
    // A snapshot serialized before the field existed omits `cb`; it must decode as not-blinking.
    let json = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "s": {"columns": 1, "rows": 1, "cell_width": 8, "cell_height": 18},
            "c": [],
            "cl": 0,
            "cc": 0,
            "cs": "block",
            "cw": 1,
            "t": "",
            "r": 1
        }"#;
    let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
    assert!(!snapshot.cursor_blinking);

    // And when present it round-trips.
    let blinking = TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        cursor_blinking: true,
        ..snapshot
    };
    let encoded = encode_line(&blinking).unwrap();
    assert!(
        serde_json::from_slice::<TerminalSnapshot>(&encoded)
            .unwrap()
            .cursor_blinking
    );
}

#[test]
fn snapshot_without_scroll_position_keys_decodes_to_zero() {
    // A snapshot serialized before the scroll fields existed omits `do`/`hs`; both must default
    // to 0 (live bottom, no scrollback) so the scrollbar stays hidden for legacy frames.
    let json = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "s": {"columns": 1, "rows": 1, "cell_width": 8, "cell_height": 18},
            "c": [],
            "cl": 0,
            "cc": 0,
            "cs": "block",
            "cw": 1,
            "t": "",
            "r": 1
        }"#;
    let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(snapshot.display_offset, 0);
    assert_eq!(snapshot.history_size, 0);

    // And when present they round-trip.
    let scrolled = TerminalSnapshot {
        display_offset: 42,
        history_size: 1000,
        ..snapshot
    };
    let decoded: TerminalSnapshot =
        serde_json::from_slice(&encode_line(&scrolled).unwrap()).unwrap();
    assert_eq!(decoded.display_offset, 42);
    assert_eq!(decoded.history_size, 1000);
}

#[test]
fn snapshot_delta_replaces_and_clears_cells_without_losing_metadata() {
    let snapshot = TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        session_id: Uuid::nil(),
        size: TerminalSize {
            columns: 3,
            rows: 1,
            ..TerminalSize::default()
        },
        cells: vec![
            TerminalCell {
                line: 0,
                column: 0,
                character: 'a',
                zerowidth: Vec::new(),
                foreground: TerminalColor::Named(256),
                background: TerminalColor::Named(257),
                underline_color: None,
                hyperlink: None,
                flags: 0,
            },
            TerminalCell {
                line: 0,
                column: 1,
                character: 'b',
                zerowidth: Vec::new(),
                foreground: TerminalColor::Named(256),
                background: TerminalColor::Named(257),
                underline_color: None,
                hyperlink: None,
                flags: 0,
            },
        ]
        .into(),
        color_overrides: Vec::new(),
        cursor_line: 0,
        cursor_column: 1,
        cursor_shape: TerminalCursorShape::Block,
        cursor_width: 1,
        cursor_blinking: false,
        title: "before".to_owned(),
        revision: 4,
        last_input_sequence: 1,
        input_modes: TerminalInputModes::default(),
        images: vec![TerminalImageDescriptor {
            key: TerminalImageKey {
                id: 12,
                generation: 13,
            },
            width: 64,
            height: 32,
        }],
        image_placements: vec![TerminalImagePlacement {
            image: TerminalImageKey {
                id: 12,
                generation: 13,
            },
            placement_id: 4,
            line: -1,
            column: 2,
            source_x: 1,
            source_y: 2,
            source_width: 32,
            source_height: 16,
            x_offset: 3,
            y_offset: 4,
            columns: 4,
            rows: 2,
            destination_width: 32,
            destination_height: 36,
            z: 1,
        }],
        selection: None,
        detected_links: Vec::new(),
        display_offset: 0,
        history_size: 0,
    };
    let replacement = TerminalCell {
        line: 0,
        column: 1,
        character: 'x',
        zerowidth: vec!['\u{301}'],
        foreground: TerminalColor::Indexed(2),
        background: TerminalColor::Named(257),
        underline_color: None,
        hyperlink: Some("https://example.com/replaced".to_owned()),
        flags: 4,
    };
    let delta = TerminalSnapshotDelta {
        prompt_editable: true,
        prompt_input: Some(PromptInput {
            text: "echo 中".into(),
            cursor: "echo 中".len(),
            native: true,
            ..Default::default()
        }),
        session_id: Uuid::nil(),
        base_revision: 4,
        size: snapshot.size,
        cells: vec![replacement.clone()],
        cleared: vec![TerminalCellPosition { line: 0, column: 0 }],
        color_overrides: vec![TerminalColorOverride {
            index: 2,
            color: 0xabcd_efff,
        }],
        cursor_line: 0,
        cursor_column: 2,
        cursor_shape: TerminalCursorShape::Beam,
        cursor_width: 1,
        cursor_blinking: false,
        title: "after".to_owned(),
        revision: 5,
        last_input_sequence: 2,
        input_modes: TerminalInputModes {
            focus_reporting: true,
            ..TerminalInputModes::default()
        },
        images: Some(Vec::new()),
        image_placements: Some(Vec::new()),
        selection: None,
        detected_links: Vec::new(),
        display_offset: 5,
        history_size: 200,
    };

    let updated = snapshot.apply_delta(&delta).unwrap();
    assert_eq!(updated.cells, vec![replacement]);
    assert_eq!(updated.title, "after");
    assert_eq!(updated.revision, 5);
    assert_eq!(updated.last_input_sequence, 2);
    assert!(updated.input_modes.focus_reporting);
    assert_eq!(updated.color_overrides, delta.color_overrides);
    assert_eq!(updated.images, delta.images.clone().unwrap());
    assert_eq!(
        updated.image_placements,
        delta.image_placements.clone().unwrap()
    );
    // Scroll position is taken from the delta (like cursor_line), not diffed against the base.
    assert_eq!(updated.display_offset, 5);
    assert_eq!(updated.history_size, 200);
    assert_eq!(updated.prompt_input, delta.prompt_input);
    assert!(updated.prompt_editable);
    let mut clear = delta.clone();
    clear.base_revision = updated.revision;
    clear.revision += 1;
    clear.prompt_input = None;
    let pending = updated.apply_delta(&clear).unwrap();
    assert!(pending.prompt_input.is_none());
    assert!(
        pending.prompt_editable,
        "a redraw must retain prompt ownership"
    );
    clear.prompt_editable = false;
    assert!(!updated.apply_delta(&clear).unwrap().prompt_editable);
    let mut wrong_base = delta;
    wrong_base.base_revision = 3;
    assert!(snapshot.apply_delta(&wrong_base).is_none());
}

#[test]
fn ssh_process_lifecycle_distinguishes_failures_and_normal_exit() {
    use RemoteSessionPhase as P;
    assert_eq!(P::from_process(&SessionStatus::Running, false), P::Connecting);
    assert_eq!(P::from_process(&SessionStatus::Running, true), P::Connected);
    assert_eq!(P::from_process(&SessionStatus::Exited { code: Some(0) }, true), P::Exited);
    assert_eq!(P::from_process(&SessionStatus::Exited { code: Some(0) }, false), P::Failed);
    for code in [Some(255), None] {
        assert_eq!(P::from_process(&SessionStatus::Exited { code }, true), P::Disconnected);
        assert_eq!(P::from_process(&SessionStatus::Exited { code }, false), P::Failed);
    }
    assert!(P::Disconnected.needs_recovery());
    assert!(P::Failed.needs_recovery());
    assert!(!P::Exited.needs_recovery());
    let request = ClientRequest::ReconnectSession { session_id: SessionId::new_v4() };
    let bytes = serde_json::to_vec(&request).unwrap();
    assert_eq!(serde_json::from_slice::<ClientRequest>(&bytes).unwrap(), request);
}
