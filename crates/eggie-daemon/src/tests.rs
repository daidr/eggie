use super::*;

static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn command_line_from_options_prefers_base64_then_url_then_raw() {
    // base64 wins and round-trips UTF-8 (CJK) that would break a raw OSC payload.
    let b64 = BASE64.encode("echo 你好;ls".as_bytes());
    let opts = format!("cmdline_b64={b64};cmdline_url=echo%20x;cmdline=raw");
    assert_eq!(
        command_line_from_options(&opts).as_deref(),
        Some("echo 你好;ls")
    );
    // Falls back to percent-decoded cmdline_url when no b64.
    assert_eq!(
        command_line_from_options("cmdline_url=echo%20hi").as_deref(),
        Some("echo hi")
    );
    // Then raw cmdline.
    assert_eq!(
        command_line_from_options("cmdline=plain").as_deref(),
        Some("plain")
    );
    // Empty / absent yields None (no blank rows in the UI).
    assert_eq!(command_line_from_options(""), None);
    assert_eq!(command_line_from_options("cmdline="), None);
}

#[test]
fn default_socket_is_scoped_by_user_and_protocol() {
    let path = daemon_socket_path();
    assert!(path.to_string_lossy().contains("eggie-"));
    assert!(
        path.to_string_lossy()
            .contains(&format!("daemon-v{PROTOCOL_VERSION}.sock"))
    );
}

#[test]
fn default_client_removes_only_obsolete_protocol_sockets() {
    let directory = std::env::temp_dir().join(format!(
        "eggie-daemon-cleanup-test-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&directory).unwrap();
    let current = directory.join(format!("daemon-v{PROTOCOL_VERSION}.sock"));
    let obsolete = directory.join("daemon-v999.sock");
    let unrelated = directory.join("keep-me.sock");
    fs::write(&current, []).unwrap();
    fs::write(&obsolete, []).unwrap();
    fs::write(&unrelated, []).unwrap();

    DaemonClient::new(current.clone(), "test").terminate_obsolete_daemons();

    assert!(current.exists());
    assert!(!obsolete.exists());
    assert!(unrelated.exists());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn raw_terminal_image_wire_appends_pixels_without_messagepack_copying() {
    let (mut writer, reader) = UnixStream::pair().unwrap();
    let pixels = Arc::new(PixelBuffer::from_vec(
        (0..=255).cycle().take(1024 * 1024).collect::<Vec<_>>(),
    ));
    let chunk = PublishedTerminalImageChunk {
        key: TerminalImageKey {
            id: 42,
            generation: 7,
        },
        width: 512,
        height: 512,
        total_length: pixels.len() as u32,
        offset: 128,
        end: pixels.len(),
        pixels: Arc::clone(&pixels),
    };
    let expected = chunk.bytes().to_vec();
    let writer_thread = thread::spawn(move || write_terminal_image_wire(&mut writer, &chunk));

    let mut reader = BufReader::new(reader);
    let header = read_wire_header(&mut reader).unwrap().unwrap();
    assert_ne!(header & RAW_TERMINAL_IMAGE_WIRE_FLAG, 0);
    let mut destination = vec![1, 2, 3];
    let metadata = read_terminal_image_wire_into(
        &mut reader,
        (header & !RAW_TERMINAL_IMAGE_WIRE_FLAG) as usize,
        &mut destination,
    )
    .unwrap();

    writer_thread.join().unwrap().unwrap();
    assert_eq!(metadata.key.id, 42);
    assert_eq!(metadata.key.generation, 7);
    assert_eq!(metadata.offset, 128);
    assert_eq!(metadata.chunk_length as usize, expected.len());
    assert_eq!(&destination[..3], &[1, 2, 3]);
    assert_eq!(&destination[3..], expected);
}

#[test]
fn shm_terminal_image_wire_round_trips_pixels_through_shared_memory() {
    // Full A1 round-trip: create an shm segment, send only its name + metadata over the wire,
    // and reconstruct the pixels on the far side by opening the segment. No pixel bytes travel
    // over the socket.
    let expected: Vec<u8> = (0..=255).cycle().take(1024 * 1024 + 3).collect();
    let segment = create_image_shm_segment(&expected).unwrap();
    let metadata = TerminalImageChunkMetadata {
        key: TerminalImageKey {
            id: 7,
            generation: 99,
        },
        width: 256,
        height: 1024,
        total_length: expected.len() as u32,
        offset: 0,
        chunk_length: expected.len() as u32,
    };

    let (mut writer, reader) = UnixStream::pair().unwrap();
    let name = segment.name_bytes().to_vec();
    let writer_thread = thread::spawn(move || {
        let mut segment = segment;
        let result = write_terminal_image_shm_wire(&mut writer, &metadata, &name);
        // The name is on the wire; the consumer now owns the unlink.
        segment.disarm();
        result
    });

    let mut reader = BufReader::new(reader);
    let header = read_wire_header(&mut reader).unwrap().unwrap();
    let (kind, length) = classify_wire_header(header);
    assert_eq!(kind, WireFrameKind::ShmImage);
    // The shm flag must be mutually exclusive with the inline-raw flag.
    assert_eq!(header & RAW_TERMINAL_IMAGE_WIRE_FLAG, 0);
    let mut destination = vec![9, 9];
    let decoded = read_terminal_image_shm_wire_into(&mut reader, length, &mut destination).unwrap();

    writer_thread.join().unwrap().unwrap();
    assert_eq!(decoded.key.id, 7);
    assert_eq!(decoded.key.generation, 99);
    assert_eq!(decoded.width, 256);
    assert_eq!(decoded.total_length as usize, expected.len());
    assert_eq!(decoded.chunk_length as usize, expected.len());
    assert_eq!(&destination[..2], &[9, 9]);
    assert_eq!(&destination[2..], &expected[..]);
}

#[test]
fn shm_transport_carries_images_larger_than_the_wire_frame_limit() {
    // Regression: shm pixels never cross the socket, so the segment must be bounded by the
    // decoder's image limit (400 MiB), not MAX_WIRE_MESSAGE_SIZE (64 MiB). An image just over
    // 64 MiB (e.g. 4097x4096x4) must round-trip; the earlier code bailed at the wire limit and
    // blanked every image larger than 4096x4096.
    let total = 64 * 1024 * 1024 + 4096; // just past the old 64 MiB cap
    let expected: Vec<u8> = (0..=255u8).cycle().take(total).collect();
    let segment = create_image_shm_segment(&expected).unwrap();
    let metadata = TerminalImageChunkMetadata {
        key: TerminalImageKey {
            id: 3,
            generation: 1,
        },
        width: 4097,
        height: 4096,
        total_length: expected.len() as u32,
        offset: 0,
        chunk_length: expected.len() as u32,
    };

    let (mut writer, reader) = UnixStream::pair().unwrap();
    let name = segment.name_bytes().to_vec();
    let writer_thread = thread::spawn(move || {
        let mut segment = segment;
        let result = write_terminal_image_shm_wire(&mut writer, &metadata, &name);
        segment.disarm();
        result
    });

    let mut reader = BufReader::new(reader);
    let header = read_wire_header(&mut reader).unwrap().unwrap();
    let (kind, length) = classify_wire_header(header);
    assert_eq!(kind, WireFrameKind::ShmImage);
    let mut destination = Vec::new();
    let decoded =
        read_terminal_image_shm_wire_into(&mut reader, length, &mut destination).unwrap();

    writer_thread.join().unwrap().unwrap();
    assert_eq!(decoded.total_length as usize, total);
    assert_eq!(destination.len(), total);
    assert_eq!(destination, expected);
}

#[test]
fn image_shm_segment_unlinks_on_drop_when_not_disarmed() {
    // An armed guard that is never disarmed (name never sent) must reclaim its segment on drop,
    // so an error before hand-off cannot leak. After drop, opening the name must fail.
    let segment = create_image_shm_segment(&[1, 2, 3, 4]).unwrap();
    let name = std::ffi::CString::new(segment.name_bytes()).unwrap();
    drop(segment);
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    assert!(fd < 0, "dropped-armed segment should have been unlinked");
}

#[test]
fn serve_connection_delivers_images_through_shared_memory() {
    // End-to-end A1: a real image published by the terminal, fetched over a real socket via
    // serve_connection, must arrive intact. On unix the daemon now answers offset==0 with an
    // shm frame, so this exercises the full create→send-name→open→copy path (and, via the
    // clean shutdown, that the transfer completes without leaking the client connection).
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = Arc::new(
        TerminalSession::spawn_default(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                cell_width: 8,
                cell_height: 18,
            },
            TerminalAppearance::default(),
        )
        .unwrap(),
    );
    let session_id = session.id;
    let state = Arc::new(DaemonState {
        sessions: RwLock::new(HashMap::from([(session_id, session.clone())])),
        build_id: Arc::from("shm-test"),
    });
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let server_state = state.clone();
    let server = thread::spawn(move || serve_connection(server_stream, &server_state));
    let mut connection = DaemonConnection {
        stream: BufReader::new(client_stream),
        request: Vec::with_capacity(512),
        response: Vec::with_capacity(1024 * 1024),
    };

    thread::sleep(Duration::from_millis(100));
    // A 2x2 RGBA image: bytes 0..16 map to the four pixels. `AQIDBAUGBwgJCgsMDQ4PEA==`
    // decodes to 1..=16.
    {
        let mut terminal = session.terminal.lock();
        terminal.kitty_graphics_command(
            b"a=T,f=32,s=2,v=2,i=11,c=1,r=1,C=1;AQIDBAUGBwgJCgsMDQ4PEA==",
        );
        session.events.publish_terminal(&terminal);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let key = loop {
        if let Some(descriptor) = session
            .snapshot()
            .images
            .iter()
            .find(|image| image.key.id == 11)
        {
            assert_eq!((descriptor.width, descriptor.height), (2, 2));
            break descriptor.key;
        }
        assert!(Instant::now() < deadline, "image never reached the snapshot");
        thread::sleep(Duration::from_millis(10));
    };

    let mut pixels = Vec::new();
    let metadata = connection
        .append_terminal_image_chunk(session_id, key, 0, 16 * 1024 * 1024, &mut pixels)
        .unwrap();
    assert_eq!(metadata.key, key);
    assert_eq!((metadata.width, metadata.height), (2, 2));
    assert_eq!(metadata.total_length, 16);
    assert_eq!(metadata.chunk_length, 16, "shm frame delivers the whole image at once");
    assert_eq!(pixels, (1..=16).collect::<Vec<u8>>());

    session.terminate();
    drop(connection);
    server.join().unwrap().unwrap();
}

#[test]
fn bundled_alacritty_terminfo_is_installed_for_child_sessions() {
    let database = install_bundled_terminfo().unwrap();
    let entry = database.join("61/alacritty");
    assert_eq!(fs::read(entry).unwrap(), BUNDLED_ALACRITTY_TERMINFO);
}

#[test]
fn child_sessions_report_the_alacritty_terminal_contract() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 100,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    let initial_revision = session.snapshot().revision;
    assert!(
        session
            .events
            .wait_for_revision(initial_revision, Duration::from_secs(5)),
        "shell did not publish its first PTY output"
    );
    session
        .input(
            b"printf 'EGGIE_ENV:%s:%s:%s\\n' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\"\r"
                .to_vec(),
            1,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let lines = session.snapshot().plain_lines();
        if lines
            .iter()
            .any(|line| line.contains("EGGIE_ENV:alacritty:truecolor:Eggie"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child session did not inherit Eggie's terminal contract: {lines:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    session.terminate();
}

#[test]
fn kitty_graphics_crosses_the_real_pty_snapshot_and_resource_paths() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let size = TerminalSize {
        columns: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 18,
    };
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        size,
        TerminalAppearance::default(),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));
    session
        .input(
            b"printf '\\033_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQIDBA==\\033\\\\'\r".to_vec(),
            1,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        let snapshot = session.snapshot();
        if snapshot.images.iter().any(|image| image.key.id == 7) {
            break snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "Kitty image did not reach the published terminal snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let descriptor = snapshot
        .images
        .iter()
        .find(|image| image.key.id == 7)
        .unwrap();
    assert_eq!((descriptor.width, descriptor.height), (1, 1));
    let placement = snapshot
        .image_placements
        .iter()
        .find(|placement| placement.image == descriptor.key)
        .unwrap();
    assert_eq!(
        (placement.destination_width, placement.destination_height),
        (8, 18)
    );

    // Advance the same Kitty image before fetching the generation referenced by `snapshot`.
    // Published frames must retain their immutable pixels instead of racing the live terminal.
    {
        let mut terminal = session.terminal.lock();
        terminal.kitty_graphics_command(b"a=d,d=I,i=7,q=2;");
        terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;BQYHCA==");
        session.events.publish_terminal(&terminal);
    }
    let newer_descriptor = loop {
        let current = session.snapshot();
        if let Some(current) = current
            .images
            .iter()
            .find(|image| image.key.id == 7 && image.key != descriptor.key)
        {
            break current.clone();
        }
        assert!(
            Instant::now() < deadline,
            "replacement Kitty image did not reach the published snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let (width, height, total, pixels) = session.image_chunk(descriptor.key, 0, 4).unwrap();
    assert_eq!((width, height, total), (1, 1, 4));
    assert_eq!(pixels, [1, 2, 3, 4]);
    let (_, _, _, pixels) = session.image_chunk(newer_descriptor.key, 0, 4).unwrap();
    assert_eq!(pixels, [5, 6, 7, 8]);
    session.terminate();
}

#[test]
fn installed_notcurses_exercises_implemented_terminal_compatibility() {
    if !Command::new("sh")
        .args(["-c", "command -v notcurses-info >/dev/null 2>&1"])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }

    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 180,
            rows: 60,
            cell_width: 8,
            cell_height: 18,
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));
    session.input(b"notcurses-info\r".to_vec(), 1).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = session.snapshot();
        let lines = snapshot.plain_lines();
        let identified = lines
            .iter()
            .any(|line| line.contains("notcurses") && line.contains(" on Kitty "));
        // Reporting a `kitty(...)` XTVERSION makes notcurses run its Kitty heuristics, which
        // grant both quadrant (2x2) and Unicode-13 sextant (3x2) blitters. The Alacritty
        // profile Eggie used to match is the one notcurses explicitly denies sextants.
        let quadrants = lines.iter().any(|line| line.contains("2x2+"));
        let sextants = lines.iter().any(|line| line.contains("3x2+"));
        let terminal_capabilities = lines
            .iter()
            .any(|line| line.contains("uline+") && line.contains("rgb+"));
        let graphics_and_input = lines.iter().any(|line| line.contains("kbd+"))
            && lines.iter().any(|line| line.contains("pmouse+"))
            // Kitty heuristics enable notcurses' animated pixel backend, so the banner reads
            // "rgba pixel animation support" rather than the "…graphics support" wording seen
            // under the old Alacritty profile. Match the substring common to both.
            && lines
                .iter()
                .any(|line| line.contains("rgba pixel") && line.contains("support"));
        let finished = lines.iter().any(|line| line.contains("renders,"));
        if identified && quadrants && sextants && terminal_capabilities && graphics_and_input && finished {
            let placement = snapshot
                .image_placements
                .first()
                .expect("notcurses Kitty logo placement must be published");
            assert_eq!(
                placement.column, 55,
                "synchronized cursor positioning must precede the Kitty display APC"
            );
            assert!(
                (6..=9).contains(&placement.line),
                "the notcurses logo must remain in its top information block: {placement:?}"
            );
            let rgb_backgrounds = snapshot
                .cells
                .iter()
                .filter_map(|cell| match cell.background {
                    eggie_protocol::TerminalColor::Rgb(color) => Some(color),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            assert!(
                lines.iter().any(
                    |line| line.contains("default fg 0x") && line.contains("default bg 0x")
                ),
                "OSC default color queries were not answered: {lines:?}"
            );
            assert!(
                rgb_backgrounds.len() > 256,
                "notcurses truecolor gradient collapsed to {} backgrounds",
                rgb_backgrounds.len()
            );
            let emoji_line = snapshot
                .cells
                .iter()
                .find(|cell| cell.character == '👾')
                .map(|cell| cell.line)
                .expect("notcurses emoji coverage row must be present");
            let emoji_cells = snapshot
                .cells
                .iter()
                .filter(|cell| cell.line == emoji_line)
                .collect::<Vec<_>>();
            for (base, suffix) in [
                ('👩', vec!['\u{200d}', '🔬']),
                ('✊', vec!['🏿']),
                ('🇦', vec!['🇶']),
                ('🏴', vec!['\u{200d}', '☠', '\u{fe0f}']),
                ('🤽', vec!['🏼', '\u{200d}', '♀', '\u{fe0f}']),
            ] {
                let cell = emoji_cells
                    .iter()
                    .find(|cell| cell.character == base && cell.zerowidth == suffix)
                    .unwrap_or_else(|| {
                        panic!("missing grapheme base {base:?}: {emoji_cells:?}")
                    });
                assert!(Flags::from_bits_retain(cell.flags).contains(Flags::WIDE_CHAR));
            }
            break;
        }
        assert!(
            Instant::now() < deadline,
            "notcurses did not detect Eggie's implemented Kitty compatibility: {lines:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    session.input(vec![0x03], 2).unwrap();
    session.terminate();
}

#[test]
fn paste_respects_alacritty_bracketed_paste_mode() {
    assert_eq!(paste_bytes("one\ntwo", false), b"one\rtwo");
    assert_eq!(
        paste_bytes("one\n\x1btwo", true),
        b"\x1b[200~one\ntwo\x1b[201~"
    );
}

#[test]
fn snapshot_omits_only_visually_empty_default_cells() {
    let mut cell = Cell::default();
    assert!(snapshot_cell_is_empty(&cell));
    cell.flags.insert(Flags::BOLD);
    assert!(snapshot_cell_is_empty(&cell));
    cell.flags.insert(Flags::UNDERLINE);
    assert!(!snapshot_cell_is_empty(&cell));

    let cell = Cell {
        bg: Color::Indexed(1),
        ..Cell::default()
    };
    assert!(!snapshot_cell_is_empty(&cell));
    let mut cell = Cell::default();
    cell.flags.insert(Flags::WRAPLINE);
    assert!(!snapshot_cell_is_empty(&cell));
}

#[test]
fn messagepack_wire_round_trip_preserves_truecolor_cells() {
    let snapshot = Arc::new(TerminalSnapshot {
        session_id: SessionId::nil(),
        size: TerminalSize {
            columns: 1,
            rows: 1,
            ..TerminalSize::default()
        },
        cells: vec![TerminalCell {
            line: 0,
            column: 0,
            character: '▀',
            zerowidth: Vec::new(),
            foreground: eggie_protocol::TerminalColor::Rgb(0x46b4c8ff),
            background: eggie_protocol::TerminalColor::Rgb(0xbee8f5ff),
            underline_color: Some(eggie_protocol::TerminalColor::Rgb(0x112233ff)),
            hyperlink: Some("https://example.com".to_owned()),
            flags: 0,
        }],
        color_overrides: vec![TerminalColorOverride {
            index: 42,
            color: 0xaabbccff,
        }],
        cursor_line: 0,
        cursor_column: 0,
        cursor_shape: TerminalCursorShape::Hidden,
        cursor_width: 1,
        cursor_blinking: false,
        title: "truecolor".to_owned(),
        revision: 7,
        last_input_sequence: 3,
        input_modes: TerminalInputModes::default(),
        images: Vec::new(),
        image_placements: Vec::new(),
        selection: None,
        detected_links: Vec::new(),
        display_offset: 0,
        history_size: 0,
    });
    let response = DaemonResponse::Snapshot {
        snapshot: snapshot.clone(),
    };
    let mut encoded = Vec::new();
    let mut scratch = Vec::new();
    write_wire_message(&mut encoded, &mut scratch, &response).unwrap();
    let decoded = read_wire_message::<DaemonResponse>(
        &mut std::io::Cursor::new(encoded),
        &mut Vec::new(),
    )
    .unwrap()
    .expect("wire frame is present");
    assert_eq!(decoded, response);
}

#[test]
fn snapshot_delta_keeps_color_only_cell_changes() {
    let cell = |column, foreground, background| TerminalCell {
        line: 0,
        column,
        character: '▀',
        zerowidth: Vec::new(),
        foreground: eggie_protocol::TerminalColor::Rgb(foreground),
        background: eggie_protocol::TerminalColor::Rgb(background),
        underline_color: None,
        hyperlink: None,
        flags: 0,
    };
    let mut base = TerminalSnapshot {
        session_id: SessionId::nil(),
        size: TerminalSize {
            columns: 2,
            rows: 1,
            ..TerminalSize::default()
        },
        cells: vec![
            cell(0, 0x111111ff, 0x222222ff),
            cell(1, 0x333333ff, 0x444444ff),
        ],
        color_overrides: Vec::new(),
        cursor_line: 0,
        cursor_column: 0,
        cursor_shape: TerminalCursorShape::Hidden,
        cursor_width: 1,
        cursor_blinking: false,
        title: String::new(),
        revision: 10,
        last_input_sequence: 0,
        input_modes: TerminalInputModes::default(),
        images: Vec::new(),
        image_placements: Vec::new(),
        selection: None,
        detected_links: Vec::new(),
        display_offset: 0,
        history_size: 0,
    };
    let image = TerminalImageKey {
        id: 7,
        generation: 9,
    };
    base.images.push(TerminalImageDescriptor {
        key: image,
        width: 1,
        height: 1,
    });
    base.image_placements.push(TerminalImagePlacement {
        image,
        placement_id: 3,
        line: 0,
        column: 0,
        source_x: 0,
        source_y: 0,
        source_width: 1,
        source_height: 1,
        x_offset: 0,
        y_offset: 0,
        columns: 1,
        rows: 1,
        destination_width: 1,
        destination_height: 1,
        z: 0,
    });
    let mut current = base.clone();
    current.revision = 11;
    current.cells[1] = cell(1, 0xaabbccff, 0x123456ff);

    let delta = snapshot_delta(&base, &current).expect("one of two cells changed");
    assert_eq!(delta.cells, vec![current.cells[1].clone()]);
    assert!(delta.cleared.is_empty());
    assert!(delta.images.is_none());
    assert!(delta.image_placements.is_none());
    assert_eq!(base.apply_delta(&delta), Some(current));
}

#[test]
fn snapshot_wait_wakes_on_revision_without_polling_delay() {
    let state = Arc::new(ListenerState::new(
        SessionId::new_v4(),
        TerminalSize::default(),
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let worker_state = state.clone();
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        worker_state.signal_revision_for_test();
    });
    let started = Instant::now();
    assert!(state.wait_for_revision(0, Duration::from_secs(1)));
    assert!(started.elapsed() < Duration::from_millis(250));
    worker.join().unwrap();

    let revision = state.revision.load(Ordering::Acquire);
    let started = Instant::now();
    assert!(!state.wait_for_revision(revision, Duration::from_millis(20)));
    assert!(started.elapsed() >= Duration::from_millis(15));
}

#[test]
fn progress_tracker_coalesces_reports_and_expires_completed_work() {
    use alacritty_terminal::vte::ansi::{ProgressReport, ProgressState};

    let session_id = SessionId::new_v4();
    let tracker = ProgressTracker::new(session_id);
    tracker.set_timeouts(TerminalProgressTimeouts {
        completed_ms: 100,
        stale_ms: 500,
    });
    tracker.report(Some(ProgressReport {
        state: ProgressState::Normal,
        percent: Some(1),
    }));
    let first = tracker
        .wait_after(0, Duration::from_millis(50))
        .expect("first report publishes immediately");
    assert_eq!(first.progress.unwrap().percent, Some(1));

    tracker.report(Some(ProgressReport {
        state: ProgressState::Normal,
        percent: Some(2),
    }));
    tracker.report(Some(ProgressReport {
        state: ProgressState::Normal,
        percent: Some(100),
    }));
    assert!(
        tracker
            .wait_after(first.revision, Duration::from_millis(2))
            .is_none()
    );
    let completed = tracker
        .wait_after(first.revision, Duration::from_millis(50))
        .expect("latest report publishes on the next progress frame");
    assert_eq!(completed.progress.unwrap().percent, Some(100));

    let cleared = tracker
        .wait_after(completed.revision, Duration::from_millis(250))
        .expect("completed progress clears after its timeout");
    assert_eq!(cleared.progress, None);
}

#[test]
fn pty_osc_progress_reaches_daemon_and_ris_clears_it() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize::default(),
        TerminalAppearance::default(),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));

    session
        .input(b"printf '\\033]9;4;1;42\\a'\r".to_vec(), 1)
        .unwrap();
    let progress = session
        .wait_for_progress(0, Duration::from_secs(2))
        .expect("OSC 9;4 report reaches daemon state");
    assert_eq!(
        progress
            .progress
            .map(|progress| (progress.state, progress.percent)),
        Some((TerminalProgressState::Normal, Some(42)))
    );

    session.input(b"printf '\\033c'\r".to_vec(), 2).unwrap();
    let cleared = session
        .wait_for_progress(progress.revision, Duration::from_secs(2))
        .expect("RIS clears daemon progress state");
    assert_eq!(cleared.progress, None);
    session.terminate();
}

#[test]
fn terminal_input_to_snapshot_is_event_driven() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    thread::sleep(Duration::from_millis(100));
    let mut snapshot = session.snapshot();
    let mut revision = snapshot.revision;
    let mut samples = Vec::new();
    let mut serialization_samples = Vec::new();
    let mut response_bytes = 0;
    for sequence in 1..=32 {
        let started = Instant::now();
        session.input(b"x".to_vec(), sequence).unwrap();
        let update = session
            .wait_for_snapshot(revision, Duration::from_secs(1))
            .expect("terminal input did not produce a snapshot");
        snapshot = match update {
            TerminalSnapshotUpdate::Full(snapshot) => snapshot,
            TerminalSnapshotUpdate::Delta(delta) => Arc::new(
                snapshot
                    .apply_delta(&delta)
                    .expect("input snapshot delta applies to its requested base"),
            ),
        };
        samples.push(started.elapsed());
        revision = snapshot.revision;
        let serialization_started = Instant::now();
        response_bytes = encode_line(&DaemonResponse::Snapshot {
            snapshot: snapshot.clone(),
        })
        .unwrap()
        .len();
        serialization_samples.push(serialization_started.elapsed());
    }
    samples.sort_unstable();
    serialization_samples.sort_unstable();
    let p50 = samples[(samples.len() - 1) * 50 / 100];
    let p95 = samples[(samples.len() - 1) * 95 / 100];
    let serialization_p50 = serialization_samples[(serialization_samples.len() - 1) * 50 / 100];
    let serialization_p95 = serialization_samples[(serialization_samples.len() - 1) * 95 / 100];
    eprintln!(
        "daemon input→snapshot latency: p50={:.2}ms p95={:.2}ms; snapshot JSON: {} bytes p50={:.2}ms p95={:.2}ms",
        p50.as_secs_f64() * 1_000.,
        p95.as_secs_f64() * 1_000.,
        response_bytes,
        serialization_p50.as_secs_f64() * 1_000.,
        serialization_p95.as_secs_f64() * 1_000.,
    );
    assert!(p95 < Duration::from_millis(250));
    session.input(vec![0x03], 33).unwrap();
    session.terminate();
}

#[test]
#[ignore = "performance benchmark requiring the locally installed flappy-tui binary"]
fn sustained_full_screen_snapshot_transport_benchmark() {
    let flappy = Path::new("/Users/bytedance/.cargo/bin/flappy-tui");
    if !flappy.exists() {
        eprintln!("skipping benchmark because {} is missing", flappy.display());
        return;
    }
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = Arc::new(
        TerminalSession::spawn_default(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 229,
                rows: 74,
                cell_width: 8,
                cell_height: 18,
            },
            TerminalAppearance::default(),
        )
        .unwrap(),
    );
    let session_id = session.id;
    let state = Arc::new(DaemonState {
        sessions: RwLock::new(HashMap::from([(session_id, session.clone())])),
        build_id: Arc::from("benchmark"),
    });
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let server_state = state.clone();
    let server = thread::spawn(move || serve_connection(server_stream, &server_state));
    let mut connection = DaemonConnection {
        stream: BufReader::new(client_stream),
        request: Vec::with_capacity(512),
        response: Vec::with_capacity(1024 * 1024),
    };

    thread::sleep(Duration::from_millis(800));
    session
        .input(format!("{}\r", flappy.display()).into_bytes(), 1)
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    session.input(b" ".to_vec(), 2).unwrap();
    thread::sleep(Duration::from_millis(700));

    let mut snapshot = session.snapshot();
    let mut revision = snapshot.revision;
    let mut samples = Vec::with_capacity(60);
    let mut payload_cells = 0;
    let mut payload_bytes = 0;
    for _ in 0..60 {
        let started = Instant::now();
        let response = connection
            .request(ClientRequest::WaitForSnapshot {
                session_id,
                after_revision: revision,
                timeout_ms: 1000,
            })
            .unwrap();
        snapshot = match response {
            DaemonResponse::Snapshot { snapshot } => snapshot,
            DaemonResponse::SnapshotDelta { delta } => Arc::new(
                snapshot
                    .apply_delta(&delta)
                    .expect("benchmark delta applies to its requested base"),
            ),
            response => panic!(
                "unexpected benchmark response: {response:?}; screen={:?}",
                session.snapshot().plain_lines()
            ),
        };
        samples.push(started.elapsed());
        revision = snapshot.revision;
        payload_cells = snapshot.cells.len();
        payload_bytes = connection.response.len();
    }
    samples.sort_unstable();
    let p50 = samples[(samples.len() - 1) * 50 / 100];
    let p95 = samples[(samples.len() - 1) * 95 / 100];
    eprintln!(
        "229x74 flappy snapshot transport: cells={payload_cells} wire={:.1}KiB p50={:.2}ms p95={:.2}ms ({:.1} fps)",
        payload_bytes as f64 / 1024.,
        p50.as_secs_f64() * 1_000.,
        p95.as_secs_f64() * 1_000.,
        1. / p50.as_secs_f64(),
    );

    session.terminate();
    drop(connection);
    server.join().unwrap().unwrap();
    assert!(p95 < Duration::from_millis(50));
}

// Dimensions of the synthetic image the two per-generation image benchmarks below transmit.
// A retina-ish full-screen frame at ~20 MiB reproduces the notcurses `xray` workload where
// every frame bumps a new image generation, defeating the `{id, generation}`-keyed caches and
// forcing a full re-transfer. RGBA (4 bytes/px): 2560 * 2048 * 4 = 20 MiB exactly. Both are
// well under `MAX_IMAGE_DIMENSION` (10_000) and `MAX_IMAGE_BYTES` (400 MiB).
const BENCH_IMAGE_WIDTH: u32 = 2560;
const BENCH_IMAGE_HEIGHT: u32 = 2048;
const BENCH_IMAGE_ID: u32 = 42;

/// Build the base64 payload once (the ~27 MiB encode is the expensive part and must stay out of
/// the timed region — `xray`'s cache misses come from generation bumps, not pixel changes, so a
/// constant image is representative for a *transport* benchmark). A single Kitty APC command is
/// capped at `MAX_COMMAND_BYTES` (16 MiB), so the payload is split into `m=1` continuation
/// chunks with the metadata carried on the first, mirroring `chunked_transmission_keeps_metadata_from_first_chunk`.
fn bench_transmit_commands() -> Vec<Vec<u8>> {
    let expected = (BENCH_IMAGE_WIDTH as usize) * (BENCH_IMAGE_HEIGHT as usize) * 4;
    // Content is irrelevant to a transport benchmark; a fixed byte keeps the encode deterministic.
    let encoded = BASE64.encode(vec![0x7f_u8; expected]);
    // Stay comfortably below the 16 MiB per-command ceiling; base64 must not split mid-quantum.
    const CHUNK: usize = 8 * 1024 * 1024;
    let chunk = CHUNK - (CHUNK % 4);
    let bytes = encoded.as_bytes();
    let mut commands = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let end = (offset + chunk).min(bytes.len());
        let more = if end < bytes.len() { 1 } else { 0 };
        let segment = &bytes[offset..end];
        let command = if offset == 0 {
            format!(
                "a=T,f=32,s={},v={},i={},m={};",
                BENCH_IMAGE_WIDTH, BENCH_IMAGE_HEIGHT, BENCH_IMAGE_ID, more
            )
        } else {
            format!("m={more};")
        };
        let mut payload = command.into_bytes();
        payload.extend_from_slice(segment);
        commands.push(payload);
        offset = end;
    }
    commands
}

/// Delete the previous image and re-transmit it, minting a fresh `{id, generation}` (see
/// `transmit`, which does `generation = wrapping_add(1).max(1)`), then publish. Returns the new
/// key the daemon now serves. Mirrors the delete+retransmit dance in
/// `kitty_graphics_crosses_the_real_pty_snapshot_and_resource_paths`.
fn bench_bump_image_generation(
    session: &TerminalSession,
    commands: &[Vec<u8>],
) -> TerminalImageKey {
    {
        let mut terminal = session.terminal.lock();
        terminal.kitty_graphics_command(format!("a=d,d=I,i={BENCH_IMAGE_ID},q=2;").as_bytes());
        for command in commands {
            terminal.kitty_graphics_command(command);
        }
        session.events.publish_terminal(&terminal);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        if let Some(descriptor) = snapshot
            .images
            .iter()
            .find(|image| image.key.id == BENCH_IMAGE_ID)
        {
            assert_eq!(
                (descriptor.width, descriptor.height),
                (BENCH_IMAGE_WIDTH, BENCH_IMAGE_HEIGHT),
                "published image geometry drifted from the transmitted APC"
            );
            return descriptor.key;
        }
        assert!(
            Instant::now() < deadline,
            "re-transmitted benchmark image never reached the published snapshot"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "performance benchmark: full-screen per-generation Kitty image transport over the wire"]
fn image_generation_wire_transport_benchmark() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = Arc::new(
        TerminalSession::spawn_default(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 229,
                rows: 74,
                cell_width: 8,
                cell_height: 18,
            },
            TerminalAppearance::default(),
        )
        .unwrap(),
    );
    let session_id = session.id;
    let state = Arc::new(DaemonState {
        sessions: RwLock::new(HashMap::from([(session_id, session.clone())])),
        build_id: Arc::from("benchmark"),
    });
    let (client_stream, server_stream) = UnixStream::pair().unwrap();
    let server_state = state.clone();
    let server = thread::spawn(move || serve_connection(server_stream, &server_state));
    let mut connection = DaemonConnection {
        stream: BufReader::new(client_stream),
        request: Vec::with_capacity(512),
        response: Vec::with_capacity(1024 * 1024),
    };

    thread::sleep(Duration::from_millis(100));
    let commands = bench_transmit_commands();
    let expected_length = (BENCH_IMAGE_WIDTH as usize) * (BENCH_IMAGE_HEIGHT as usize) * 4;

    let iterations = 60;
    let mut samples = Vec::with_capacity(iterations);
    let mut pixels = Vec::with_capacity(expected_length);
    let mut chunks_per_frame = 0;
    for _ in 0..iterations {
        // Minting the new generation, injecting the APC and publishing are all excluded from the
        // timed region — only the wire fetch of one complete generation is measured.
        let key = bench_bump_image_generation(&session, &commands);
        pixels.clear();
        let started = Instant::now();
        let mut chunks = 0;
        while pixels.len() < expected_length {
            let offset = pixels.len() as u32;
            let metadata = connection
                .append_terminal_image_chunk(
                    session_id,
                    key,
                    offset,
                    16 * 1024 * 1024,
                    &mut pixels,
                )
                .unwrap();
            chunks += 1;
            assert_eq!(metadata.key, key);
            assert_eq!(metadata.total_length as usize, expected_length);
            assert_eq!(metadata.offset, offset);
            assert!(metadata.chunk_length > 0, "wire transfer stalled mid-frame");
        }
        samples.push(started.elapsed());
        chunks_per_frame = chunks;
        assert_eq!(pixels.len(), expected_length, "frame arrived incomplete");
    }

    samples.sort_unstable();
    let p50 = samples[(samples.len() - 1) * 50 / 100];
    let p95 = samples[(samples.len() - 1) * 95 / 100];
    // With the inline wire path each frame crosses the socket as pixels + per-chunk metadata,
    // and is copied twice (daemon write_all + client read_exact). With the shm transport the
    // socket carries only metadata + the segment name, and the pixels are copied via shared
    // memory (daemon page-write + client page-read) instead of the socket. Report both the
    // pixel payload and the resulting per-frame chunk count so before/after stays legible; the
    // per-frame latency above is the headline number.
    let pixel_bytes_per_frame = expected_length;
    eprintln!(
        "{}x{} image transport: pixels/frame={:.2}MiB chunks/frame={} p50={:.2}ms p95={:.2}ms ({:.1} fps)",
        BENCH_IMAGE_WIDTH,
        BENCH_IMAGE_HEIGHT,
        pixel_bytes_per_frame as f64 / (1024. * 1024.),
        chunks_per_frame,
        p50.as_secs_f64() * 1_000.,
        p95.as_secs_f64() * 1_000.,
        1. / p50.as_secs_f64(),
    );

    session.terminate();
    drop(connection);
    server.join().unwrap().unwrap();
    // Loose smoke guard only: absolute latency is memcpy-bandwidth and scheduler dependent, so a
    // tight threshold would flap across machines. The value of this benchmark is the printed
    // numbers, compared before/after Route A.
    assert!(p95 < Duration::from_millis(500));
}

#[test]
#[ignore = "performance benchmark: daemon-side per-generation chunk borrow stays zero-copy"]
fn image_generation_chunk_ref_stays_zero_copy() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 229,
            rows: 74,
            cell_width: 8,
            cell_height: 18,
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(100));
    let commands = bench_transmit_commands();
    let expected_length = (BENCH_IMAGE_WIDTH as usize) * (BENCH_IMAGE_HEIGHT as usize) * 4;

    let iterations = 60;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let key = bench_bump_image_generation(&session, &commands);
        // `image_chunk_ref` only clones the immutable `Arc<PixelBuffer>` and computes slice
        // bounds — no pixel bytes are copied. A single call is capped at `MAX_IMAGE_CHUNK_SIZE`
        // (16 MiB), so walk the whole generation in chunks exactly as the wire path does, but
        // daemon-side only. Timing this confirms the daemon fetch stays O(1) per chunk
        // regardless of image size, and guards that property against regressions.
        let started = Instant::now();
        let mut fetched = 0usize;
        while fetched < expected_length {
            let chunk = session
                .image_chunk_ref(key, fetched as u32, expected_length as u32)
                .unwrap();
            assert_eq!(chunk.total_length as usize, expected_length);
            let chunk_len = chunk.bytes().len();
            assert!(chunk_len > 0, "chunk borrow stalled mid-frame");
            fetched += chunk_len;
        }
        let elapsed = started.elapsed();
        assert_eq!(fetched, expected_length, "chunk borrows did not cover the frame");
        samples.push(elapsed);
    }

    samples.sort_unstable();
    let p50 = samples[(samples.len() - 1) * 50 / 100];
    let p95 = samples[(samples.len() - 1) * 95 / 100];
    eprintln!(
        "{}x{} daemon chunk_ref borrow: p50={:.1}us p95={:.1}us (zero-copy Arc clone)",
        BENCH_IMAGE_WIDTH,
        BENCH_IMAGE_HEIGHT,
        p50.as_secs_f64() * 1_000_000.,
        p95.as_secs_f64() * 1_000_000.,
    );

    session.terminate();
    // A zero-copy Arc clone + bounds math must stay far below a millisecond even for a 20 MiB
    // image; a regression to per-fetch copying would blow past this.
    assert!(p95 < Duration::from_millis(5));
}

#[test]
fn adjacent_input_messages_are_coalesced_without_losing_order() {
    let session_id = SessionId::new_v4();
    let mut first = QueuedTerminalInput::Input {
        session_id,
        bytes: b"a".to_vec(),
        sequence: 1,
    };
    first
        .merge(QueuedTerminalInput::Input {
            session_id,
            bytes: b"b".to_vec(),
            sequence: 2,
        })
        .unwrap();
    let ClientRequest::Input {
        bytes, sequence, ..
    } = first.request()
    else {
        panic!("coalesced input changed request kind")
    };
    assert_eq!(bytes, b"ab");
    assert_eq!(sequence, 2);
}

#[test]
fn continuous_input_is_dispatched_in_latency_bounded_batches() {
    let session_id = SessionId::new_v4();
    let (sender, receiver) = mpsc::channel();
    for sequence in 1..=(MAX_INPUT_BATCH_MESSAGES as u64 * 3) {
        sender
            .send(QueuedTerminalInput::Input {
                session_id,
                bytes: vec![b'x'],
                sequence,
            })
            .unwrap();
    }
    let first = receiver.recv().unwrap();
    let batch = receive_input_batch(&receiver, first);
    assert_eq!(batch.len(), 1, "adjacent key input should still coalesce");
    let ClientRequest::Input {
        bytes, sequence, ..
    } = batch.into_iter().next().unwrap().request()
    else {
        panic!("input batch changed request kind")
    };
    assert_eq!(bytes.len(), MAX_INPUT_BATCH_MESSAGES);
    assert_eq!(sequence, MAX_INPUT_BATCH_MESSAGES as u64);
    assert_eq!(receiver.try_iter().count(), MAX_INPUT_BATCH_MESSAGES * 2);
}

#[test]
fn unresolved_terminal_colors_remain_semantic() {
    assert_eq!(
        snapshot_color(Color::Named(
            alacritty_terminal::vte::ansi::NamedColor::Foreground
        )),
        eggie_protocol::TerminalColor::Named(
            alacritty_terminal::vte::ansi::NamedColor::Foreground as u16
        )
    );
}

#[test]
fn every_alacritty_cursor_shape_is_preserved_in_the_snapshot_protocol() {
    assert_eq!(
        snapshot_cursor_shape(CursorShape::Block),
        TerminalCursorShape::Block
    );
    assert_eq!(
        snapshot_cursor_shape(CursorShape::Underline),
        TerminalCursorShape::Underline
    );
    assert_eq!(
        snapshot_cursor_shape(CursorShape::Beam),
        TerminalCursorShape::Beam
    );
    assert_eq!(
        snapshot_cursor_shape(CursorShape::HollowBlock),
        TerminalCursorShape::HollowBlock
    );
    assert_eq!(
        snapshot_cursor_shape(CursorShape::Hidden),
        TerminalCursorShape::Hidden
    );
}

#[test]
fn default_cursor_shape_config_is_applied_and_overridable_by_the_program() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    let size = TerminalSize {
        columns: 20,
        rows: 5,
        ..TerminalSize::default()
    };
    // Build a terminal with a Beam default shape (as set_default_cursor_shape would).
    let mut term = Term::new(
        terminal_config(
            kernel_cursor_shape(TerminalCursorShape::Beam),
            TERMINAL_SCROLLBACK_LIMIT,
        ),
        &GridSize(size),
        VoidListener,
    );
    assert_eq!(term.cursor_style().shape, CursorShape::Beam);

    // A program issuing DECSCUSR (CSI 2 SP q = steady block) overrides the configured default.
    let mut processor: Processor = Processor::new();
    processor.advance(&mut term, b"\x1b[2 q");
    assert_eq!(term.cursor_style().shape, CursorShape::Block);

    // Reapplying the default via set_options must not clobber the program's runtime override.
    term.set_options(terminal_config(
        kernel_cursor_shape(TerminalCursorShape::Underline),
        TERMINAL_SCROLLBACK_LIMIT,
    ));
    assert_eq!(
        term.cursor_style().shape,
        CursorShape::Block,
        "program's DECSCUSR override should survive a default-shape config change"
    );

    // DECSCUSR 0 resets to the (new) configured default.
    processor.advance(&mut term, b"\x1b[0 q");
    assert_eq!(term.cursor_style().shape, CursorShape::Underline);
}

#[test]
fn hidden_is_not_usable_as_a_default_cursor_shape() {
    // Hidden has no meaning as a *default* (it would make the cursor permanently invisible), so
    // the protocol->kernel mapping falls back to Block.
    assert_eq!(
        kernel_cursor_shape(TerminalCursorShape::Hidden),
        CursorShape::Block
    );
}

#[test]
fn scrollback_limit_config_caps_history_and_survives_a_cursor_change() {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    let size = TerminalSize {
        columns: 20,
        rows: 5,
        ..TerminalSize::default()
    };
    // A 50-line scrollback caps history no matter how much scrolls past.
    let mut term = Term::new(
        terminal_config(CursorShape::Block, 50),
        &GridSize(size),
        VoidListener,
    );
    let mut processor: Processor = Processor::new();
    processor.advance(&mut term, "\r\n".repeat(200).as_bytes());
    assert_eq!(
        term.grid().history_size(),
        50,
        "scrollback history should cap at the configured limit"
    );

    // Reapplying the config with a *different cursor shape* but the same scrollback must not
    // reset the history cap — the clobber risk `config_state` guards against.
    term.set_options(terminal_config(CursorShape::Beam, 50));
    processor.advance(&mut term, "\r\n".repeat(50).as_bytes());
    assert_eq!(term.grid().history_size(), 50);
    assert_eq!(term.cursor_style().shape, CursorShape::Beam);

    // Shrinking the scrollback (cursor shape unchanged) evicts down to the new limit at once.
    term.set_options(terminal_config(CursorShape::Beam, 20));
    assert_eq!(term.grid().history_size(), 20);
    assert_eq!(term.cursor_style().shape, CursorShape::Beam);
}

#[test]
fn runtime_scrollback_and_cursor_changes_do_not_clobber_each_other() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn(SessionSpawnConfig {
        project_id: ProjectId::new_v4(),
        window_id: WindowId::new_v4(),
        cwd: std::env::current_dir().unwrap(),
        size: TerminalSize::default(),
        appearance: TerminalAppearance::default(),
        scrollback_limit: 500,
        shell_program: None,
        shell_args: None,
        shell_features: "path".to_owned(),
    })
    .unwrap();

    // The spawn-time scrollback threads into the session's live config state.
    {
        let state = session.config_state.lock();
        assert_eq!(state.scrollback_limit, 500);
        assert_eq!(state.cursor_shape, CursorShape::Block);
    }

    // Changing the cursor default preserves the custom scrollback.
    session.set_default_cursor_shape(TerminalCursorShape::Beam);
    {
        let state = session.config_state.lock();
        assert_eq!(state.cursor_shape, CursorShape::Beam);
        assert_eq!(
            state.scrollback_limit, 500,
            "a cursor change must not reset the scrollback depth"
        );
    }

    // Changing the scrollback preserves the cursor default.
    session.set_scrollback_limit(50);
    {
        let state = session.config_state.lock();
        assert_eq!(state.scrollback_limit, 50);
        assert_eq!(
            state.cursor_shape,
            CursorShape::Beam,
            "a scrollback change must not reset the default cursor shape"
        );
    }

    session.terminate();
}

#[test]
fn set_scrollback_limit_shrinks_live_history() {
    use alacritty_terminal::vte::ansi::Processor;

    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 20,
            rows: 5,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    // Drive the shared grid directly so the test is deterministic (no PTY output timing).
    {
        let mut term = session.terminal.lock();
        let mut processor: Processor = Processor::new();
        processor.advance(&mut *term, "\r\n".repeat(200).as_bytes());
        assert!(term.grid().history_size() >= 50);
    }

    session.set_scrollback_limit(50);
    assert_eq!(
        session.terminal.lock().grid().history_size(),
        50,
        "shrinking the scrollback limit should evict the oldest lines immediately"
    );

    session.terminate();
}

#[test]
fn custom_shell_program_drives_the_session_shell() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn(SessionSpawnConfig {
        project_id: ProjectId::new_v4(),
        window_id: WindowId::new_v4(),
        cwd: std::env::current_dir().unwrap(),
        size: TerminalSize::default(),
        appearance: TerminalAppearance::default(),
        scrollback_limit: TERMINAL_SCROLLBACK_LIMIT,
        shell_program: Some("/bin/sh".to_owned()),
        shell_args: None,
        shell_features: "path".to_owned(),
    })
    .unwrap();
    // The configured shell's basename becomes the session's initial process name.
    assert_eq!(session.runtime_metadata.lock().current_process.name, "sh");
    session.terminate();
}

#[test]
fn process_filter_keeps_only_the_terminal_process_tree() {
    let processes = vec![
        ProcessInfo {
            pid: 10,
            parent_pid: Some(1),
            name: "shell".to_owned(),
            cpu_usage_tenths_percent: None,
            memory_bytes: None,
        },
        ProcessInfo {
            pid: 11,
            parent_pid: Some(10),
            name: "node".to_owned(),
            cpu_usage_tenths_percent: None,
            memory_bytes: None,
        },
        ProcessInfo {
            pid: 12,
            parent_pid: Some(11),
            name: "worker".to_owned(),
            cpu_usage_tenths_percent: None,
            memory_bytes: None,
        },
        ProcessInfo {
            pid: 20,
            parent_pid: Some(1),
            name: "unrelated".to_owned(),
            cpu_usage_tenths_percent: None,
            memory_bytes: None,
        },
    ];

    assert_eq!(
        filter_descendant_processes(10, processes)
            .into_iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        [10, 11, 12]
    );
}

#[test]
fn cpu_usage_is_stored_at_one_decimal_percent_precision() {
    assert_eq!(cpu_usage_tenths_percent(12.34), 123);
    assert_eq!(cpu_usage_tenths_percent(12.36), 124);
    assert_eq!(cpu_usage_tenths_percent(f32::NAN), 0);
    assert_eq!(cpu_usage_tenths_percent(-1.), 0);
}

#[test]
fn lsof_machine_output_is_parsed_into_listening_ports() {
    let output = "p42\ncpython\nf5u\nPTCP\nn127.0.0.1:3000\nf6u\nPUDP\nn*:5353\n";

    assert_eq!(
        parse_lsof_ports(output),
        vec![
            ListeningPort {
                pid: 42,
                protocol: "TCP".to_owned(),
                address: "127.0.0.1".to_owned(),
                port: 3000,
            },
            ListeningPort {
                pid: 42,
                protocol: "UDP".to_owned(),
                address: "*".to_owned(),
                port: 5353,
            },
        ]
    );
}

#[test]
fn open_tcp_listener_is_reported_for_its_process() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let pid = std::process::id();
    let ports = listening_ports(&[ProcessInfo {
        pid,
        parent_pid: None,
        name: "test".to_owned(),
        cpu_usage_tenths_percent: None,
        memory_bytes: None,
    }]);

    assert!(
        ports
            .iter()
            .any(|entry| entry.pid == pid && entry.port == port && entry.protocol == "TCP")
    );
}

#[test]
fn primary_screen_resize_reflows_completed_output_without_losing_columns() {
    let initial_size = TerminalSize {
        columns: 12,
        rows: 4,
        ..TerminalSize::default()
    };
    let state = Arc::new(ListenerState::new(
        SessionId::new_v4(),
        initial_size,
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let listener = DaemonEventListener(state);
    let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
    for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
        terminal.grid_mut()[Line(0)][Column(column)].c = character;
    }
    terminal.grid_mut()[Line(0)][Column(11)]
        .flags
        .insert(Flags::WRAPLINE);
    for (column, character) in "MNOP".chars().enumerate() {
        terminal.grid_mut()[Line(1)][Column(column)].c = character;
    }
    // The cursor is on the active shell line below this completed wrapped output.
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(3), Column(0));

    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 8,
            ..initial_size
        },
        TerminalSemanticPhase::Output,
        None,
    );

    assert_eq!(terminal.grid()[Line(0)][Column(0)].c, 'A');
    assert_eq!(terminal.grid()[Line(0)][Column(7)].c, 'H');
    assert_eq!(terminal.grid()[Line(1)][Column(0)].c, 'I');
    assert_eq!(terminal.grid()[Line(1)][Column(7)].c, 'P');
    assert!(
        terminal.grid()[Line(0)][Column(7)]
            .flags
            .contains(Flags::WRAPLINE)
    );

    resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);

    assert_eq!(terminal.grid()[Line(0)][Column(0)].c, 'A');
    assert_eq!(terminal.grid()[Line(0)][Column(11)].c, 'L');
    assert_eq!(terminal.grid()[Line(1)][Column(0)].c, 'M');
}

#[test]
fn primary_screen_resize_clears_active_wrapped_input_for_shell_redraw() {
    let initial_size = TerminalSize {
        columns: 12,
        rows: 3,
        ..TerminalSize::default()
    };
    let state = Arc::new(ListenerState::new(
        SessionId::new_v4(),
        initial_size,
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let listener = DaemonEventListener(state);
    let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
    for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
        terminal.grid_mut()[Line(0)][Column(column)].c = character;
    }
    terminal.grid_mut()[Line(0)][Column(11)]
        .flags
        .insert(Flags::WRAPLINE);
    for (column, character) in "MNOP".chars().enumerate() {
        terminal.grid_mut()[Line(1)][Column(column)].c = character;
    }
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(1), Column(4));

    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 8,
            ..initial_size
        },
        TerminalSemanticPhase::Input,
        Some(0),
    );

    // The active prompt/input line is on a prompt phase, so it is cleared entirely: none of
    // its glyphs survive and no stale WRAPLINE continuation is left for the shell's redraw to
    // stack a duplicate on top of.
    let has_prompt_glyph = (-(terminal.grid().history_size() as i32)
        ..terminal.grid().screen_lines() as i32)
        .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
        .any(|(line, column)| {
            let cell = &terminal.grid()[Line(line)][Column(column)];
            "ABCDEFGHIJKLMNOP".contains(cell.c)
        });
    assert!(
        !has_prompt_glyph,
        "the active prompt/input line must be cleared for the shell to redraw"
    );
    let has_wrapline = (-(terminal.grid().history_size() as i32)
        ..terminal.grid().screen_lines() as i32)
        .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
        .any(|(line, column)| {
            terminal.grid()[Line(line)][Column(column)]
                .flags
                .contains(Flags::WRAPLINE)
        });
    assert!(
        !has_wrapline,
        "no stale WRAPLINE continuation may survive on the cleared prompt line"
    );
}

/// Build a bare primary-screen terminal with a two-row wrapped active line ("ABCDEFGH" on
/// row 0 continuing into "IJKL" on row 1), the cursor parked on the continuation row. Returns
/// the terminal ready for a resize.
fn terminal_with_wrapped_active_line(size: TerminalSize) -> Term<DaemonEventListener> {
    let state = Arc::new(ListenerState::new(
        SessionId::new_v4(),
        size,
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let listener = DaemonEventListener(state);
    let mut terminal = Term::new(Config::default(), &GridSize(size), listener);
    for (column, character) in "ABCDEFGH".chars().enumerate() {
        terminal.grid_mut()[Line(0)][Column(column)].c = character;
    }
    terminal.grid_mut()[Line(0)][Column(size.columns as usize - 1)]
        .flags
        .insert(Flags::WRAPLINE);
    for (column, character) in "IJKL".chars().enumerate() {
        terminal.grid_mut()[Line(1)][Column(column)].c = character;
    }
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(1), Column(4));
    terminal
}

fn grid_has_glyph(terminal: &Term<DaemonEventListener>, glyphs: &str) -> bool {
    (-(terminal.grid().history_size() as i32)..terminal.grid().screen_lines() as i32)
        .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
        .any(|(line, column)| glyphs.contains(terminal.grid()[Line(line)][Column(column)].c))
}

fn grid_has_wrapline(terminal: &Term<DaemonEventListener>) -> bool {
    (-(terminal.grid().history_size() as i32)..terminal.grid().screen_lines() as i32)
        .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
        .any(|(line, column)| {
            terminal.grid()[Line(line)][Column(column)]
                .flags
                .contains(Flags::WRAPLINE)
        })
}

#[test]
fn prompt_phase_shrink_clears_active_line_and_removes_wrapline() {
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);
    // Put a completed-output row above the active line to make sure it is NOT cleared.
    terminal.grid_mut()[Line(3)][Column(0)].c = '#';

    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 4,
            ..initial_size
        },
        TerminalSemanticPhase::Prompt,
        Some(0),
    );

    assert!(
        !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "the wrapped prompt line must be fully cleared on shrink"
    );
    assert!(
        !grid_has_wrapline(&terminal),
        "no WRAPLINE may survive on the cleared prompt region"
    );
}

#[test]
fn prompt_phase_grow_clears_active_line_no_orphan_rows() {
    // Regression test for the duplicate-fragment bug: on WIDEN, the old code stripped the
    // WRAPLINE marker but left the continuation cells in place, so alacritty's grow_columns
    // failed to merge them and left an orphan row that stacked under the shell's redraw.
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);

    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 16,
            ..initial_size
        },
        TerminalSemanticPhase::Input,
        Some(0),
    );

    assert!(
        !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "no orphan continuation row may survive a widen on a prompt line"
    );
    assert!(
        !grid_has_wrapline(&terminal),
        "no stale WRAPLINE may survive a widen on a prompt line"
    );
}

#[test]
fn output_phase_reflows_natively_without_clearing() {
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);

    // Output phase: the wrapped content is completed command output and must reflow, not be
    // cleared. Shrink then grow and confirm the glyphs survive the round trip.
    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 4,
            ..initial_size
        },
        TerminalSemanticPhase::Output,
        None,
    );
    assert!(
        grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "output content must be preserved (reflowed), not cleared"
    );
    resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);
    assert!(
        grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "output content must survive the reflow round trip"
    );
}

#[test]
fn none_phase_uses_native_reflow_without_clearing() {
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);

    // No shell integration: phase stays None and we must fall back to native reflow, never
    // clearing content.
    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 4,
            ..initial_size
        },
        TerminalSemanticPhase::None,
        None,
    );
    assert!(
        grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "without shell integration, content must reflow, not be cleared"
    );
}

#[test]
fn alt_screen_resize_skips_prompt_clearing() {
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);
    // Enter the alternate screen; the wrapped active line above is on the primary screen, but
    // the alt-screen guard must short-circuit before any clearing regardless of phase.
    let mut parser: alacritty_terminal::vte::ansi::Processor =
        alacritty_terminal::vte::ansi::Processor::new();
    parser.advance(&mut terminal, b"\x1b[?1049h");
    assert!(terminal.mode().contains(TermMode::ALT_SCREEN));

    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 4,
            ..initial_size
        },
        TerminalSemanticPhase::Prompt,
        Some(0),
    );
    // The alt screen was blank, so there is nothing to assert on its contents; the test simply
    // verifies the guard path runs without touching the primary grid's clear logic (no panic,
    // resize applied).
    assert_eq!(terminal.columns(), 4);
}

#[test]
fn row_only_resize_does_not_clear_prompt() {
    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);

    // Only rows change (columns stay 8): no wrap-reflow happens, so the prompt line must be
    // left untouched even on a prompt phase.
    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            rows: 6,
            ..initial_size
        },
        TerminalSemanticPhase::Prompt,
        Some(0),
    );
    assert!(
        grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "a row-only resize must not clear the prompt line"
    );
}

#[test]
fn resize_reads_tracker_phase_to_gate_clearing() {
    // Drive the tracker the way the OSC 133 event path does, then confirm the phase it exposes
    // routes the resize into the prompt-clearing branch.
    let mut tracker = ShellIntegrationTracker::default();
    tracker.update(
        SemanticPrompt {
            action: SemanticPromptAction::PromptStart,
            options: String::new(),
        },
        0,
        0,
    );
    tracker.update(
        SemanticPrompt {
            action: SemanticPromptAction::InputStart,
            options: String::new(),
        },
        1,
        0,
    );
    assert_eq!(tracker.phase, TerminalSemanticPhase::Input);
    // The prompt start was recorded from the first (PromptStart) marker, not moved down to the
    // input line.
    assert_eq!(tracker.prompt_start_line, Some(0));

    let initial_size = TerminalSize {
        columns: 8,
        rows: 4,
        ..TerminalSize::default()
    };
    let mut terminal = terminal_with_wrapped_active_line(initial_size);
    resize_terminal_with_history_reflow(
        &mut terminal,
        TerminalSize {
            columns: 4,
            ..initial_size
        },
        tracker.phase,
        tracker.prompt_start_line,
    );
    assert!(
        !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
        "the tracker's Input phase must route the resize into the prompt-clearing branch"
    );
}

fn prompt_start(tracker: &mut ShellIntegrationTracker, cursor_line: i32, history_size: usize) {
    tracker.update(
        SemanticPrompt {
            action: SemanticPromptAction::PromptStart,
            options: String::new(),
        },
        cursor_line,
        history_size,
    );
}

fn output_start(tracker: &mut ShellIntegrationTracker, history_size: usize) {
    tracker.update(
        SemanticPrompt {
            action: SemanticPromptAction::OutputStart,
            options: String::new(),
        },
        0,
        history_size,
    );
}

#[test]
fn prompt_jump_points_dedupe_repeated_marks() {
    let mut tracker = ShellIntegrationTracker::default();
    // A prompt re-emitting PromptStart while already on the prompt (zle redraw) must not add a
    // second jump point.
    prompt_start(&mut tracker, 0, 0);
    prompt_start(&mut tracker, 0, 0);
    prompt_start(&mut tracker, 0, 0);
    assert_eq!(tracker.prompt_jump_points.len(), 1);
    assert_eq!(tracker.prompt_jump_points.front(), Some(&0));
}

#[test]
fn prompt_jump_points_use_global_line_from_total_scrolled() {
    let mut tracker = ShellIntegrationTracker::default();
    // First prompt at screen line 0, nothing scrolled yet -> global 0.
    prompt_start(&mut tracker, 0, 0);
    output_start(&mut tracker, 0);
    // 5 lines of output scrolled off (history_size captured with the next marker); next prompt
    // at screen line 3 -> global 5 + 3 = 8. This proves the capture uses the marker's own
    // history_size, without relying on a separate observe_scroll call.
    prompt_start(&mut tracker, 3, 5);
    assert_eq!(
        tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
        vec![0, 8]
    );
}

#[test]
fn prompt_capture_uses_marker_history_not_stale_observe_scroll() {
    // Regression: a burst like `cat ~/.zshrc` scrolls many lines in one parser batch, while the
    // throttled observe_scroll lags behind. The new prompt marker must still record an accurate
    // global coordinate from its own (fresh) history_size, so a later Up jump lands on the
    // previous prompt rather than in the middle of the output.
    let mut tracker = ShellIntegrationTracker::default();
    prompt_start(&mut tracker, 0, 0); // prompt #0 at global 0
    output_start(&mut tracker, 0);
    // 50 lines of output scrolled off. observe_scroll has NOT run yet (still stale at 0), but
    // the next prompt marker carries the true history_size = 50.
    prompt_start(&mut tracker, 2, 50); // prompt #1 at global 50 + 2 = 52
    assert_eq!(
        tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
        vec![0, 52],
        "the second prompt must be recorded at its true post-scroll coordinate"
    );
    // Viewport sitting at the live bottom: top line global == total_scrolled (50).
    assert_eq!(tracker.total_scrolled_lines, 50);
    // Jumping Up from the bottom selects prompt #0 (global 0), never a mid-output line.
    assert_eq!(tracker.jump_target(50, TerminalJumpDirection::Up), Some(0));
}

#[test]
fn observe_scroll_tracks_history_below_saturation() {
    let mut tracker = ShellIntegrationTracker::default();
    prompt_start(&mut tracker, 0, 0); // global 0
    output_start(&mut tracker, 0);
    // Below saturation, total_scrolled == history_size and nothing is evicted yet (the whole
    // buffer still fits in scrollback), so the point survives.
    tracker.observe_scroll(500, TERMINAL_SCROLLBACK_LIMIT);
    assert_eq!(tracker.total_scrolled_lines, 500);
    assert_eq!(tracker.prompt_jump_points.front(), Some(&0));
}

#[test]
fn observe_scroll_prunes_points_that_fall_out_of_a_shrunk_buffer() {
    let mut tracker = ShellIntegrationTracker::default();
    // Simulate a saturated buffer: many lines scrolled, points spread across the window.
    tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
    tracker.prompt_jump_points.extend([5, 9_000]);
    // The active-screen top is at global == total_scrolled; a point at global 5 sits
    // `10000 - 5` lines up, still inside the scrollback window (oldest live == 0), so it stays.
    tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
    assert_eq!(
        tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
        vec![5, 9_000]
    );
}

#[test]
fn command_history_cap_bounds_the_jump_index() {
    let mut tracker = ShellIntegrationTracker::default();
    // Push more distinct prompts than the cap; the oldest are dropped, newest kept.
    for i in 0..(COMMAND_HISTORY + 5) {
        prompt_start(&mut tracker, 0, i);
        output_start(&mut tracker, i);
    }
    assert!(tracker.prompt_jump_points.len() <= COMMAND_HISTORY);
}

#[test]
fn observe_scroll_advances_after_saturation() {
    let mut tracker = ShellIntegrationTracker::default();
    tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
    assert_eq!(tracker.total_scrolled_lines, TERMINAL_SCROLLBACK_LIMIT as u64);
    // history_size stays pinned at the limit, but observe_scroll must not stall: the delta is 0
    // here, so total stays; a later call with the same size keeps it monotonic.
    tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
    assert_eq!(tracker.total_scrolled_lines, TERMINAL_SCROLLBACK_LIMIT as u64);
}

#[test]
fn clear_jump_points_resets_index_and_base() {
    let mut tracker = ShellIntegrationTracker::default();
    prompt_start(&mut tracker, 2, 0);
    tracker.observe_scroll(10, TERMINAL_SCROLLBACK_LIMIT);
    tracker.clear_jump_points();
    assert!(tracker.prompt_jump_points.is_empty());
    assert_eq!(tracker.total_scrolled_lines, 0);
    assert_eq!(tracker.last_history_size, 0);
}

#[test]
fn jump_target_selects_strictly_nearer_prompt_in_each_direction() {
    let mut tracker = ShellIntegrationTracker::default();
    // Prompts at global lines 0, 10, 20.
    tracker.prompt_jump_points.extend([0, 10, 20]);
    // Viewport top at global 15: Up -> nearest below 15 is 10; Down -> nearest above 15 is 20.
    assert_eq!(tracker.jump_target(15, TerminalJumpDirection::Up), Some(10));
    assert_eq!(tracker.jump_target(15, TerminalJumpDirection::Down), Some(20));
    // At the oldest prompt (0): Up has nothing strictly earlier.
    assert_eq!(tracker.jump_target(0, TerminalJumpDirection::Up), None);
    // At the newest prompt (20): Down has nothing strictly later.
    assert_eq!(tracker.jump_target(20, TerminalJumpDirection::Down), None);
    // Exactly on a prompt line (10): Up -> 0, Down -> 20 (strict inequality).
    assert_eq!(tracker.jump_target(10, TerminalJumpDirection::Up), Some(0));
    assert_eq!(tracker.jump_target(10, TerminalJumpDirection::Down), Some(20));
}

#[test]
fn zsh_env_sets_zdotdir_and_preserves_original() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env(
        "zsh",
        "/bin/zsh",
        &terminfo,
        Some(&root),
        Some("/home/user/.zsh".to_owned()),
        None,
        None,
        None,
        "path",
    );
    assert_eq!(
        launch.env.get("ZDOTDIR").map(String::as_str),
        Some("/tmp/eggie-integration/zsh")
    );
    assert_eq!(
        launch.env.get("EGGIE_ZDOTDIR_ORIG").map(String::as_str),
        Some("/home/user/.zsh")
    );
    // Base environment still present, and the login arg is preserved for zsh.
    assert_eq!(launch.env.get("TERM").map(String::as_str), Some("alacritty"));
    assert!(launch.env.contains_key("TERMINFO"));
    assert_eq!(launch.args, vec!["-l".to_owned()]);
}

#[test]
fn zsh_env_without_user_zdotdir_sets_no_marker() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env("zsh", "/bin/zsh", &terminfo, Some(&root), None, None, None, None, "path");
    assert!(launch.env.contains_key("ZDOTDIR"));
    assert!(!launch.env.contains_key("EGGIE_ZDOTDIR_ORIG"));
}

#[test]
fn bash_env_sets_env_var_and_inject() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    // A non-Apple bash path (e.g. Homebrew) gets full integration.
    let launch = build_shell_env(
        "bash",
        "/opt/homebrew/bin/bash",
        &terminfo,
        Some(&root),
        None,
        Some("/home/user/env.sh".to_owned()),
        None,
        None,
        "path",
    );
    assert_eq!(
        launch.env.get("ENV").map(String::as_str),
        Some("/tmp/eggie-integration/bash/eggie.bash")
    );
    assert_eq!(
        launch.env.get("EGGIE_BASH_ENV").map(String::as_str),
        Some("/home/user/env.sh")
    );
    assert_eq!(
        launch.env.get("EGGIE_BASH_INJECT").map(String::as_str),
        Some("1")
    );
    assert_eq!(launch.args, vec!["--posix".to_owned()]);
}

#[test]
#[cfg(target_os = "macos")]
fn apple_bin_bash_skips_integration() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env("bash", "/bin/bash", &terminfo, Some(&root), None, None, None, None, "path");
    // Apple's /bin/bash cannot use the ENV-based POSIX startup path, so no injection.
    assert!(!launch.env.contains_key("ENV"));
    assert!(!launch.env.contains_key("EGGIE_BASH_INJECT"));
    assert!(launch.env.contains_key("TERM"));
}

#[test]
fn non_integrated_shell_has_no_injection() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch =
        build_shell_env("fish", "/usr/bin/fish", &terminfo, Some(&root), None, None, None, None, "path");
    assert!(!launch.env.contains_key("ZDOTDIR"));
    assert!(!launch.env.contains_key("ENV"));
    assert!(!launch.env.contains_key("EGGIE_BASH_INJECT"));
    // Base environment is still populated.
    assert_eq!(launch.env.get("TERM").map(String::as_str), Some("alacritty"));
}

#[test]
fn no_integration_root_skips_injection() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    // Installation failed -> integration_root is None -> zsh gets no ZDOTDIR override.
    let launch = build_shell_env(
        "zsh",
        "/bin/zsh",
        &terminfo,
        None,
        Some("/home/user/.zsh".to_owned()),
        None,
        None,
        None,
        "path",
    );
    assert!(!launch.env.contains_key("ZDOTDIR"));
    assert!(!launch.env.contains_key("EGGIE_ZDOTDIR_ORIG"));
    assert_eq!(launch.args, vec!["-l".to_owned()]);
}

#[test]
fn injects_bin_dir_and_features() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let bin_dir = PathBuf::from("/opt/eggie/bin");
    let launch = build_shell_env(
        "zsh",
        "/bin/zsh",
        &terminfo,
        Some(&root),
        None,
        None,
        None,
        Some(&bin_dir),
        "path",
    );
    assert_eq!(
        launch.env.get("EGGIE_BIN_DIR").map(String::as_str),
        Some("/opt/eggie/bin")
    );
    assert_eq!(
        launch.env.get("EGGIE_SHELL_FEATURES").map(String::as_str),
        Some("path")
    );
}

#[test]
fn without_bin_dir_only_features_is_injected() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    // current_exe() failure -> bin_dir None -> EGGIE_BIN_DIR omitted, but the feature list still
    // ships so a shell that resolves the binary another way still knows what to enable.
    let launch = build_shell_env(
        "zsh",
        "/bin/zsh",
        &terminfo,
        Some(&root),
        None,
        None,
        None,
        None,
        "path",
    );
    assert!(!launch.env.contains_key("EGGIE_BIN_DIR"));
    assert_eq!(
        launch.env.get("EGGIE_SHELL_FEATURES").map(String::as_str),
        Some("path")
    );
}

#[test]
fn features_injected_even_for_non_integrated_shell() {
    // fish has no integration script yet, but the env vars are injected unconditionally so its
    // future script (or a manually-sourced one) can act on them.
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let bin_dir = PathBuf::from("/opt/eggie/bin");
    let launch = build_shell_env(
        "fish",
        "/usr/bin/fish",
        &terminfo,
        None,
        None,
        None,
        None,
        Some(&bin_dir),
        "path",
    );
    assert_eq!(
        launch.env.get("EGGIE_BIN_DIR").map(String::as_str),
        Some("/opt/eggie/bin")
    );
    assert_eq!(
        launch.env.get("EGGIE_SHELL_FEATURES").map(String::as_str),
        Some("path")
    );
}

#[test]
fn custom_args_replace_the_default_login_arg_for_zsh() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env(
        "zsh",
        "/bin/zsh",
        &terminfo,
        Some(&root),
        None,
        None,
        Some(vec!["-i".to_owned(), "-c".to_owned(), "echo hi".to_owned()]),
        None,
        "path",
    );
    // Custom args used verbatim (no implicit `-l`), integration env still applied.
    assert_eq!(
        launch.args,
        vec!["-i".to_owned(), "-c".to_owned(), "echo hi".to_owned()]
    );
    assert!(launch.env.contains_key("ZDOTDIR"));
}

#[test]
fn bash_forces_posix_ahead_of_custom_args() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env(
        "bash",
        "/opt/homebrew/bin/bash",
        &terminfo,
        Some(&root),
        None,
        None,
        Some(vec!["-i".to_owned()]),
        None,
        "path",
    );
    // `--posix` is mandatory for the ENV-based hook, so it is prepended before the user's args.
    assert_eq!(launch.args, vec!["--posix".to_owned(), "-i".to_owned()]);
    assert!(launch.env.contains_key("ENV"));
    assert_eq!(
        launch.env.get("EGGIE_BASH_INJECT").map(String::as_str),
        Some("1")
    );
}

#[test]
fn bash_does_not_duplicate_a_user_supplied_posix_flag() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env(
        "bash",
        "/opt/homebrew/bin/bash",
        &terminfo,
        Some(&root),
        None,
        None,
        Some(vec!["--posix".to_owned(), "-c".to_owned(), "true".to_owned()]),
        None,
        "path",
    );
    assert_eq!(
        launch.args,
        vec!["--posix".to_owned(), "-c".to_owned(), "true".to_owned()]
    );
}

#[test]
fn custom_args_pass_through_for_non_integrated_shell() {
    let terminfo = PathBuf::from("/tmp/eggie-terminfo");
    let root = PathBuf::from("/tmp/eggie-integration");
    let launch = build_shell_env(
        "fish",
        "/usr/bin/fish",
        &terminfo,
        Some(&root),
        None,
        None,
        Some(vec!["-C".to_owned(), "echo hi".to_owned()]),
        None,
        "path",
    );
    // fish gets no injection; custom args used verbatim.
    assert_eq!(launch.args, vec!["-C".to_owned(), "echo hi".to_owned()]);
    assert!(!launch.env.contains_key("ENV"));
    assert!(!launch.env.contains_key("ZDOTDIR"));
}

#[test]
fn repeated_column_reflow_keeps_images_attached_to_content_and_blank_cells() {
    let initial_size = TerminalSize {
        columns: 12,
        rows: 6,
        cell_width: 8,
        cell_height: 18,
    };
    let state = Arc::new(ListenerState::new(
        SessionId::new_v4(),
        initial_size,
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let listener = DaemonEventListener(state);
    let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
    terminal.set_kitty_graphics_cell_size(initial_size.cell_width, initial_size.cell_height);

    for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
        terminal.grid_mut()[Line(0)][Column(column)].c = character;
    }
    terminal.grid_mut()[Line(0)][Column(11)]
        .flags
        .insert(Flags::WRAPLINE);
    for (column, character) in "MNOP@RSTUVWX".chars().enumerate() {
        terminal.grid_mut()[Line(1)][Column(column)].c = character;
    }
    terminal.grid_mut()[Line(2)][Column(0)].c = '#';

    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(1), Column(4));
    terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=2,r=2,C=1;AQIDBA==");
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(2), Column(10));
    terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=8,c=1,r=1,C=1;AQIDBA==");
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(5), Column(0));

    let compact_size = TerminalSize {
        columns: 8,
        ..initial_size
    };

    for _ in 0..3 {
        resize_terminal_with_history_reflow(&mut terminal, compact_size, TerminalSemanticPhase::Output, None);
        let marker = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '@')
            .expect("the marked text cell survives reflow");
        let line_start = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '#')
            .expect("the blank-anchor line survives reflow");
        let snapshot = terminal.kitty_graphics_snapshot();
        let content = snapshot
            .placements
            .iter()
            .find(|placement| placement.image.id == 7)
            .unwrap();
        let blank = snapshot
            .placements
            .iter()
            .find(|placement| placement.image.id == 8)
            .unwrap();
        assert_eq!((content.line, content.column), (marker.0, marker.1 as u32));
        assert_eq!((blank.line, blank.column), (line_start.0 + 1, 2));

        resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);
        let marker = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '@')
            .expect("the marked text cell unwraps");
        let line_start = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '#')
            .expect("the blank-anchor line unwraps");
        let snapshot = terminal.kitty_graphics_snapshot();
        let content = snapshot
            .placements
            .iter()
            .find(|placement| placement.image.id == 7)
            .unwrap();
        let blank = snapshot
            .placements
            .iter()
            .find(|placement| placement.image.id == 8)
            .unwrap();
        assert_eq!((content.line, content.column), (marker.0, marker.1 as u32));
        assert_eq!((blank.line, blank.column), (line_start.0, 10));
    }
}

#[test]
fn scrollback_round_trip_keeps_image_attached_to_its_text_row() {
    let size = TerminalSize {
        columns: 20,
        rows: 5,
        cell_width: 8,
        cell_height: 18,
    };
    let session_id = SessionId::new_v4();
    let state = Arc::new(ListenerState::new(
        session_id,
        size,
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    ));
    let listener = DaemonEventListener(state);
    let mut terminal = Term::new(Config::default(), &GridSize(size), listener);
    terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
    terminal.grid_mut()[Line(1)][Column(8)].c = 'X';
    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(1), Column(8));
    terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=1,r=2,C=1;AQIDBA==");

    terminal.grid_mut().cursor.point =
        alacritty_terminal::index::Point::new(Line(4), Column(0));
    let mut parser: alacritty_terminal::vte::ansi::Processor =
        alacritty_terminal::vte::ansi::Processor::new();
    parser.advance(&mut terminal, b"\n\n\n\n\n\n\n\n");

    let bottom = snapshot_terminal(&terminal, session_id, size, String::new(), 1, 0);
    assert!(
        bottom.image_placements.is_empty(),
        "an image in scrollback must not stay pinned to the top of the live viewport"
    );

    terminal.scroll_display(Scroll::Delta(8));
    let history = snapshot_terminal(&terminal, session_id, size, String::new(), 2, 0);
    let marker_line = history
        .cells
        .iter()
        .find(|cell| cell.character == 'X')
        .map(|cell| i32::from(cell.line))
        .expect("the anchor text must be visible in scrollback");
    assert_eq!(history.image_placements.len(), 1);
    assert_eq!(history.image_placements[0].line, marker_line);

    terminal.scroll_display(Scroll::Bottom);
    assert!(
        snapshot_terminal(&terminal, session_id, size, String::new(), 3, 0)
            .image_placements
            .is_empty()
    );
    terminal.scroll_display(Scroll::Delta(8));
    let history_again = snapshot_terminal(&terminal, session_id, size, String::new(), 4, 0);
    assert_eq!(history_again.image_placements[0].line, marker_line);

    terminal.scroll_display(Scroll::Bottom);
    let compact_size = TerminalSize { columns: 6, ..size };
    resize_terminal_with_history_reflow(&mut terminal, compact_size, TerminalSemanticPhase::Output, None);
    assert!(
        snapshot_terminal(&terminal, session_id, compact_size, String::new(), 5, 0,)
            .image_placements
            .is_empty(),
        "resizing must not pull a historical image into the live viewport",
    );
    terminal.scroll_display(Scroll::Top);
    let compact_history =
        snapshot_terminal(&terminal, session_id, compact_size, String::new(), 6, 0);
    let marker = compact_history
        .cells
        .iter()
        .find(|cell| cell.character == 'X')
        .expect("the reflowed anchor cell must remain in scrollback");
    assert_eq!(compact_history.image_placements.len(), 1);
    assert_eq!(
        compact_history.image_placements[0].line,
        i32::from(marker.line)
    );
    assert_eq!(
        compact_history.image_placements[0].column,
        u32::from(marker.column)
    );

    resize_terminal_with_history_reflow(&mut terminal, size, TerminalSemanticPhase::Output, None);
    terminal.scroll_display(Scroll::Top);
    let restored_history = snapshot_terminal(&terminal, session_id, size, String::new(), 7, 0);
    let marker = restored_history
        .cells
        .iter()
        .find(|cell| cell.character == 'X')
        .expect("the unwrapped anchor cell must remain in scrollback");
    assert_eq!(restored_history.image_placements.len(), 1);
    assert_eq!(
        restored_history.image_placements[0].line,
        i32::from(marker.line)
    );
    assert_eq!(
        restored_history.image_placements[0].column,
        u32::from(marker.column)
    );
}

#[test]
fn resizing_a_session_updates_its_snapshot_grid_and_revision() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize::default(),
        TerminalAppearance::default(),
    )
    .unwrap();
    let previous_revision = session.snapshot().revision;
    let size = TerminalSize {
        columns: 72,
        rows: 18,
        cell_width: 9,
        cell_height: 19,
    };

    session.resize(size).unwrap();

    let snapshot = session.snapshot();
    assert_eq!(snapshot.size, size);
    assert!(snapshot.revision > previous_revision);
    session.terminate();
}

#[test]
fn pty_output_is_parsed_into_an_alacritty_snapshot() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let cwd = std::env::current_dir().unwrap();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        cwd,
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(
            b"printf '\\033]4;1;#123456\\007\\033[1;3;4;9;31;58;2;17;34;51mEGGIE_PTY_OK\\033[0m\\n'\r"
                .to_vec(),
            1,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        let snapshot = session.snapshot();
        let dynamic_color_ready = snapshot.color_overrides.contains(&TerminalColorOverride {
            index: 1,
            color: 0x123456ff,
        });
        let styled_output_ready = snapshot.cells.iter().any(|cell| {
            cell.character == 'E'
                && Flags::from_bits_retain(cell.flags)
                    .contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::STRIKEOUT)
        });
        if dynamic_color_ready && styled_output_ready {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "terminal output did not arrive");
        thread::sleep(Duration::from_millis(20));
    };

    assert!(snapshot.revision > 0);
    assert!(snapshot.color_overrides.contains(&TerminalColorOverride {
        index: 1,
        color: 0x123456ff,
    }));
    let styled_cell = snapshot
        .cells
        .iter()
        .find(|cell| {
            cell.character == 'E'
                && Flags::from_bits_retain(cell.flags)
                    .contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::STRIKEOUT)
        })
        .expect("styled terminal cell was not captured");
    assert_eq!(
        styled_cell.foreground,
        eggie_protocol::TerminalColor::Named(1)
    );
    assert_eq!(
        styled_cell.underline_color,
        Some(eggie_protocol::TerminalColor::Rgb(0x112233ff))
    );
    let flags = Flags::from_bits_retain(styled_cell.flags);
    assert!(flags.contains(Flags::BOLD));
    assert!(flags.contains(Flags::ITALIC));
    assert!(flags.contains(Flags::UNDERLINE));
    assert!(flags.contains(Flags::STRIKEOUT));
    session.terminate();
}

#[test]
fn terminal_search_finds_counts_and_navigates_matches() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    // Print three lines that each contain the needle so the search has multiple matches.
    session
        .input(
            b"printf 'needle one\\nneedle two\\nneedle three\\n'\r".to_vec(),
            1,
        )
        .unwrap();

    // Wait until all three needle lines have been parsed into the snapshot.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        let needle_rows = snapshot
            .cells
            .iter()
            .filter(|cell| cell.character == 'n' && cell.column == 0)
            .count();
        if needle_rows >= 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "needle output did not arrive in the snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    }

    // A fresh forward search should find a match and count every occurrence. The exact count
    // is not asserted because the shell echoes the command line (which also contains the
    // needle); instead we verify the invariants that must hold regardless of echo.
    let first = session
        .search(TerminalSearchRequest {
            query: "needle".to_owned(),
            regex: false,
            direction: TerminalSearchDirection::Forward,
            fresh: true,
        })
        .unwrap();
    assert!(
        first.total >= 3,
        "expected at least three matches for 'needle', got {}",
        first.total
    );
    assert_eq!(first.index, 0, "fresh search should start at the first match");
    let first_active = first.active.expect("active match should be present");
    assert!(
        first.matches.iter().any(|m| *m == first_active),
        "the active match should be among the visible highlights"
    );
    let total = first.total;

    // Advancing forward moves to the next match without changing the total.
    let second = session
        .search(TerminalSearchRequest {
            query: "needle".to_owned(),
            regex: false,
            direction: TerminalSearchDirection::Forward,
            fresh: false,
        })
        .unwrap();
    assert_eq!(second.total, total);
    assert_eq!(second.index, 1, "forward navigation should advance the index");

    // A query with no matches reports nothing.
    let missing = session
        .search(TerminalSearchRequest {
            query: "this-string-does-not-exist".to_owned(),
            regex: false,
            direction: TerminalSearchDirection::Forward,
            fresh: true,
        })
        .unwrap();
    assert_eq!(missing.total, 0);
    assert!(missing.active.is_none());

    // A regex search matches the same needles as the literal search.
    let regex_result = session
        .search(TerminalSearchRequest {
            query: "n..dle".to_owned(),
            regex: true,
            direction: TerminalSearchDirection::Forward,
            fresh: true,
        })
        .unwrap();
    assert_eq!(
        regex_result.total, total,
        "regex 'n..dle' should match the same cells as literal 'needle'"
    );

    session.terminate();
}

#[test]
fn select_all_and_selection_text_span_the_whole_scrollback() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    // Print more lines than the viewport so the earliest ones scroll into history.
    session
        .input(
            b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
            1,
        )
        .unwrap();

    // Wait until LASTLINE has been parsed into the snapshot.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        let has_last = snapshot
            .plain_lines()
            .iter()
            .any(|line| line.contains("LASTLINE"));
        if has_last {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "LASTLINE did not arrive in the snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    }

    // FIRSTLINE is now in scrollback (not visible). Select-all must reach it and LASTLINE.
    session.select_all().unwrap();
    let text = session
        .selection_text()
        .unwrap()
        .expect("select all should produce text");
    assert!(
        text.contains("FIRSTLINE"),
        "select all should include the scrollback top; got:\n{text}"
    );
    assert!(
        text.contains("LASTLINE"),
        "select all should include the buffer bottom; got:\n{text}"
    );

    // Clearing drops the selection text entirely.
    session.selection_clear().unwrap();
    assert!(session.selection_text().unwrap().is_none());

    session.terminate();
}

#[test]
fn scroll_to_commands_move_the_viewport_across_scrollback() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(
            b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
            1,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        if snapshot
            .plain_lines()
            .iter()
            .any(|line| line.contains("LASTLINE"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "LASTLINE did not arrive");
        thread::sleep(Duration::from_millis(20));
    }

    // FIRSTLINE is in scrollback and not visible at the live bottom.
    assert!(
        !session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("FIRSTLINE")),
        "FIRSTLINE should start off-screen in scrollback"
    );

    // Jump to the top: the earliest scrollback line becomes visible.
    session.scroll_to(TerminalScrollCommand::Top).unwrap();
    assert!(
        session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("FIRSTLINE")),
        "scroll-to-top should reveal the oldest scrollback line"
    );

    // Jump back to the bottom: the live tail is visible again.
    session.scroll_to(TerminalScrollCommand::Bottom).unwrap();
    assert!(
        session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("LASTLINE")),
        "scroll-to-bottom should return to the live viewport"
    );
    assert!(
        !session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("FIRSTLINE")),
        "scroll-to-bottom should leave scrollback again"
    );

    session.terminate();
}

/// Fills the scrollback with numbered filler lines and returns the session once `marker` is
/// visible at the live bottom. Shared setup for the scroll-position tests below.
#[cfg(test)]
fn session_with_scrollback(marker: &str) -> TerminalSession {
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    // Split the marker across a shell string concatenation so the echoed command line reads
    // `"BO""TTOMMARK"` while only the printed OUTPUT line reads `BOTTOMMARK`. Otherwise the wait
    // loop would match the echoed command before any filler ran, breaking out with empty
    // scrollback (the source of a nasty flake).
    let (head, tail) = marker.split_at(2);
    session
        .input(
            format!(
                "for i in $(seq 1 60); do echo filler $i; done; printf '%s\\n' \"{head}\"\"{tail}\"\r"
            )
            .into_bytes(),
            1,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains(marker))
        {
            break;
        }
        assert!(Instant::now() < deadline, "{marker} did not arrive");
        thread::sleep(Duration::from_millis(20));
    }
    // Wait for the shell to go quiescent (prompt drawn, no more output) so `history_size` is
    // stable. Otherwise trailing output keeps growing history and, while scrolled, the kernel
    // advances display_offset to stay anchored — which would make the positioning asserts flaky.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = session.snapshot().history_size;
    let mut stable_since = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(50));
        let now = session.snapshot().history_size;
        if now == last {
            if stable_since.elapsed() >= Duration::from_millis(300) {
                break;
            }
        } else {
            last = now;
            stable_since = Instant::now();
        }
        assert!(Instant::now() < deadline, "scrollback never went quiescent");
    }
    session
}

#[test]
fn snapshot_reports_display_offset_and_history_size() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = session_with_scrollback("BOTTOMMARK");

    // At the live bottom there is scrollback above but the offset is zero.
    let bottom = session.snapshot();
    assert_eq!(bottom.display_offset, 0, "live bottom has zero offset");
    assert!(bottom.history_size > 0, "filler lines built scrollback");

    // Scrolling to the very top puts the offset at the full history size.
    session.scroll_to(TerminalScrollCommand::Top).unwrap();
    let top = session.snapshot();
    assert_eq!(
        top.display_offset, top.history_size,
        "scroll-to-top offset equals history size"
    );

    session.terminate();
}

#[test]
fn scroll_to_offset_positions_viewport_and_clamps() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = session_with_scrollback("TAILMARK");
    let history = session.snapshot().history_size;
    assert!(history >= 4, "need enough scrollback to position within");

    // An interior offset lands exactly there.
    let target = history / 2;
    session.scroll_to_offset(target).unwrap();
    assert_eq!(session.snapshot().display_offset, target);

    // Offsets past the history clamp to the top rather than panicking or overshooting.
    session.scroll_to_offset(u32::MAX).unwrap();
    assert_eq!(session.snapshot().display_offset, history);

    // Offset zero returns to the live bottom.
    session.scroll_to_offset(0).unwrap();
    assert_eq!(session.snapshot().display_offset, 0);

    session.terminate();
}

#[test]
fn output_while_scrolled_keeps_the_thumb_anchor() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = session_with_scrollback("ANCHORMARK");
    let history = session.snapshot().history_size;

    // Queue delayed output BEFORE scrolling. `input()` snaps the viewport to the live bottom
    // (scroll-on-keystroke), so we must issue no further keystrokes after scrolling — the
    // `sleep` lets the burst arrive on its own while we sit scrolled back.
    session
        .input(
            b"sleep 0.6; for i in $(seq 1 10); do echo more $i; done\r".to_vec(),
            2,
        )
        .unwrap();

    // Scroll up into the middle of the scrollback and record the top-of-viewport index.
    let offset = history / 2;
    session.scroll_to_offset(offset).unwrap();
    let scrolled = session.snapshot();
    let anchor = scrolled.history_size - scrolled.display_offset;

    // The delayed burst lands with no keystroke: the kernel advances display_offset in lockstep
    // with history growth so the visible content (and thus the thumb) stays anchored —
    // history_size - display_offset holds constant.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = session.snapshot();
        if now.history_size > history {
            assert_eq!(
                now.history_size - now.display_offset,
                anchor,
                "top-of-viewport index must stay constant while output arrives"
            );
            break;
        }
        assert!(Instant::now() < deadline, "additional output did not arrive");
        thread::sleep(Duration::from_millis(20));
    }

    session.terminate();
}

#[test]
fn keystroke_snaps_the_viewport_back_to_the_live_bottom() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(
            b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
            1,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("LASTLINE"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "LASTLINE did not arrive");
        thread::sleep(Duration::from_millis(20));
    }

    // Scroll up into history so the live bottom is off-screen.
    session.scroll_to(TerminalScrollCommand::Top).unwrap();
    assert!(
        session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("FIRSTLINE")),
        "precondition: scrolled into scrollback"
    );

    // Typing must snap the viewport back to the bottom, even before the echo arrives.
    session.input(b"x".to_vec(), 2).unwrap();
    assert!(
        session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("LASTLINE")),
        "a keystroke should scroll the viewport back to the live bottom"
    );

    session.terminate();
}

#[test]
fn interactive_selection_projects_into_the_visible_viewport() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(b"printf 'ALPHA BRAVO CHARLIE\\n'\r".to_vec(), 1)
        .unwrap();

    // Wait until the printed line has arrived, then wait for the terminal to go quiescent (the
    // trailing shell prompt can scroll the grid). Take the target row and its expected text from
    // the SAME settled snapshot so the assertion tests the viewport→absolute mapping, not timing.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        let present = snapshot.plain_lines().iter().any(|line| {
            line.contains("ALPHA BRAVO CHARLIE") && !line.contains("printf")
        });
        if present {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "printed line did not arrive in the snapshot"
        );
        thread::sleep(Duration::from_millis(20));
    }
    // Settle: require the revision to hold steady across a short window.
    let mut last_revision = session.snapshot().revision;
    loop {
        thread::sleep(Duration::from_millis(120));
        let revision = session.snapshot().revision;
        if revision == last_revision {
            break;
        }
        last_revision = revision;
        assert!(
            Instant::now() < deadline,
            "terminal did not go quiescent for the selection test"
        );
    }

    let settled = session.snapshot();
    let lines = settled.plain_lines();
    let target_line = lines
        .iter()
        .position(|line| line.contains("ALPHA BRAVO CHARLIE") && !line.contains("printf"))
        .expect("printed line should be present in the settled snapshot")
        as u16;
    let expected: String = lines[target_line as usize].chars().take(5).collect();

    // Select the first five columns of that row.
    session
        .selection_start(
            TerminalCellPosition {
                line: target_line,
                column: 0,
            },
            TerminalSelectionSide::Left,
            TerminalSelectionKind::Simple,
        )
        .unwrap();
    session
        .selection_update(
            TerminalCellPosition {
                line: target_line,
                column: 4,
            },
            TerminalSelectionSide::Right,
        )
        .unwrap();

    let text = session
        .selection_text()
        .unwrap()
        .expect("interactive selection should produce text");
    assert_eq!(text, expected);
    assert_eq!(expected, "ALPHA");

    // The selection projects into the visible viewport on the same row.
    let projected = session
        .snapshot()
        .selection
        .expect("visible selection should project into the snapshot");
    assert_eq!(projected.start.line, target_line);
    assert_eq!(projected.start.column, 0);
    assert_eq!(projected.end.line, target_line);

    session.terminate();
}

#[test]
fn detects_bare_url_and_trims_trailing_punctuation() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(b"printf 'see https://example.com. now\\n'\r".to_vec(), 1)
        .unwrap();

    // Wait until a detected link appears for the printed URL (ignoring the echoed command line).
    let deadline = Instant::now() + Duration::from_secs(5);
    let link = loop {
        let snapshot = session.snapshot();
        let link = snapshot
            .detected_links
            .iter()
            .find(|link| link.url == "https://example.com")
            .cloned();
        if let Some(link) = link {
            break link;
        }
        assert!(
            Instant::now() < deadline,
            "detected link did not arrive; links = {:?}",
            session.snapshot().detected_links
        );
        thread::sleep(Duration::from_millis(20));
    };

    // The trailing period must not be part of the URL, so the range ends before it.
    let snapshot = session.snapshot();
    let line = snapshot
        .plain_lines()
        .into_iter()
        .find(|line| line.contains("see https://example.com. now"))
        .expect("printed line should be present");
    let dot_column = line.find("https://example.com.").unwrap() + "https://example.com".len();
    assert!(
        (link.end.column as usize) < dot_column,
        "range should stop before the trailing period at column {dot_column}, got end {}",
        link.end.column
    );

    session.terminate();
}

#[test]
fn refine_url_range_strips_trailing_and_unbalanced_punctuation() {
    // A pure-logic check of the trimming rules independent of the grid: build a tiny term,
    // print candidates, and assert the cleaned text. Uses the same trimming that runs on real
    // matches, exercised through short strings.
    for (raw, expected) in [
        ("https://example.com.", "https://example.com"),
        ("http://a.com,", "http://a.com"),
        ("https://a.com/x)", "https://a.com/x"),
        ("https://a.com/wiki_(foo)", "https://a.com/wiki_(foo)"),
    ] {
        let cleaned = trim_url_trailing(raw);
        assert_eq!(cleaned, expected, "trimming {raw}");
    }
}

#[test]
fn explicit_osc8_hyperlink_stays_out_of_detected_links() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    // OSC 8 hyperlink whose visible text is NOT itself a URL, so a match in detected_links could
    // only come from the auto-detector (which must ignore explicit links).
    session
        .input(
            b"printf '\\033]8;;https://osc8.example\\033\\\\clickme\\033]8;;\\033\\\\\\n'\r"
                .to_vec(),
            1,
        )
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        // Find the cell that carries the explicit OSC 8 link (its visible text is "clickme").
        let osc8_cell = snapshot
            .cells
            .iter()
            .find(|cell| cell.hyperlink.as_deref() == Some("https://osc8.example"));
        if let Some(cell) = osc8_cell {
            // That cell must not be covered by any auto-detected link range: explicit OSC 8
            // links are carried on `hyperlink`, never duplicated into `detected_links`. (The
            // echoed command line may separately contain the literal URL text as a real bare
            // URL; that is a different cell and legitimately detectable.)
            let covered = snapshot.detected_links.iter().any(|link| {
                link.start.line == cell.line
                    && cell.column >= link.start.column
                    && cell.column <= link.end.column
            });
            assert!(
                !covered,
                "the OSC 8 link cell must not be covered by a detected bare URL"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "OSC 8 hyperlink cell did not arrive"
        );
        thread::sleep(Duration::from_millis(20));
    }

    session.terminate();
}

#[test]
fn terminal_search_overlapping_regex_navigation_advances_without_repeat() {
    // Regression for overlapping matches: navigation must advance past a match's END, so a
    // pattern like "aa" over "aaaa" does not re-find the same overlapping match, and the
    // reported index moves forward instead of sticking.
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input(b"printf 'zzzz aaaa zzzz\\n'\r".to_vec(), 1)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        let has_aaaa = snapshot
            .cells
            .iter()
            .any(|cell| cell.character == 'a' && cell.column > 0);
        if has_aaaa {
            break;
        }
        assert!(Instant::now() < deadline, "aaaa output did not arrive");
        thread::sleep(Duration::from_millis(20));
    }

    let first = session
        .search(TerminalSearchRequest {
            query: "aa".to_owned(),
            regex: true,
            direction: TerminalSearchDirection::Forward,
            fresh: true,
        })
        .unwrap();
    let first_active = first.active.expect("first overlapping match should be found");
    assert_eq!(first.index, 0, "fresh search starts at the first match");

    // Advancing forward must land on a different, later match (no overlap re-find).
    let second = session
        .search(TerminalSearchRequest {
            query: "aa".to_owned(),
            regex: true,
            direction: TerminalSearchDirection::Forward,
            fresh: false,
        })
        .unwrap();
    let second_active = second.active.expect("a second match should exist");
    assert_ne!(
        (first_active.start.line, first_active.start.column),
        (second_active.start.line, second_active.start.column),
        "forward navigation must not re-find the same overlapping match"
    );
    assert_eq!(
        second.total, first.total,
        "the total match count is stable across navigation"
    );
    assert!(
        second.index < second.total,
        "the active index stays within range"
    );

    session.terminate();
}

#[test]
fn non_bmp_utf8_input_round_trips_through_the_pty_and_alacritty() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();

    session
        .input("printf 'EGGIE🙂OK\\n'\r".as_bytes().to_vec(), 1)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        if snapshot.cells.iter().any(|cell| cell.character == '🙂') {
            let emoji = snapshot
                .cells
                .iter()
                .find(|cell| cell.character == '🙂')
                .unwrap();
            assert!(Flags::from_bits_retain(emoji.flags).contains(Flags::WIDE_CHAR));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "emoji input did not round-trip through the terminal"
        );
        thread::sleep(Duration::from_millis(20));
    }

    session.terminate();
}

#[test]
fn session_summary_tracks_foreground_process_and_working_directory() {
    let _pty_guard = PTY_TEST_LOCK.lock();
    let cwd = std::env::current_dir().unwrap();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        cwd,
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    let expected_directory = fs::canonicalize(std::env::temp_dir()).unwrap();
    session
        .input(
            format!(
                "printf 'EGGIE_METADATA_READY\\n'; cd '{}'\r",
                expected_directory.display()
            )
            .into_bytes(),
            1,
        )
        .unwrap();

    let shell_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if session
            .snapshot()
            .plain_lines()
            .iter()
            .any(|line| line.contains("EGGIE_METADATA_READY"))
        {
            break;
        }
        assert!(
            Instant::now() < shell_deadline,
            "shell did not execute metadata test setup"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let directory_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let summary = session.summary();
        if summary.current_directory == expected_directory {
            break;
        }
        assert!(
            Instant::now() < directory_deadline,
            "terminal working directory did not update: {}; process={} pid={} shell_pid={}",
            summary.current_directory.display(),
            summary.current_process.name,
            summary.current_process.pid,
            summary.shell_pid,
        );
        thread::sleep(Duration::from_millis(20));
    }

    session.input(b"sleep 5\r".to_vec(), 2).unwrap();
    let process_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let summary = session.summary();
        if summary.current_process.name == "sleep" {
            assert_ne!(summary.current_process.pid, 0);
            break;
        }
        assert!(
            Instant::now() < process_deadline,
            "foreground process did not update: {}",
            summary.current_process.name
        );
        thread::sleep(Duration::from_millis(20));
    }

    session.input(vec![0x03], 3).unwrap();
    session.terminate();
}

#[test]
fn terminal_input_modes_follow_alacritty_mode_precedence() {
    let mode = TermMode::MOUSE_DRAG
        | TermMode::SGR_MOUSE
        | TermMode::FOCUS_IN_OUT
        | TermMode::ALT_SCREEN
        | TermMode::ALTERNATE_SCROLL;
    assert_eq!(
        terminal_input_modes(mode),
        TerminalInputModes {
            mouse_tracking: TerminalMouseTracking::Drag,
            mouse_encoding: TerminalMouseEncoding::Sgr,
            focus_reporting: true,
            alternate_screen: true,
            alternate_scroll: true,
            paste_events: false,
            kitty_keyboard_flags: 0,
        }
    );
    assert_eq!(
        terminal_input_modes(mode | TermMode::MOUSE_MOTION).mouse_tracking,
        TerminalMouseTracking::Motion
    );
    assert_eq!(
        terminal_input_modes(mode | TermMode::VI).mouse_tracking,
        TerminalMouseTracking::Disabled
    );
}

#[test]
fn legacy_mouse_reports_press_release_modifiers_and_coordinate_limits() {
    let position = TerminalMousePosition {
        column: 4,
        row: 2,
        pixel_x: 0,
        pixel_y: 0,
    };
    let press = TerminalMouseEvent {
        action: TerminalMouseAction::Press,
        button: Some(TerminalMouseButton::Left),
        position: TerminalMousePosition {
            column: 0,
            row: 0,
            pixel_x: 0,
            pixel_y: 0,
        },
        modifiers: TerminalModifiers::default(),
    };
    assert_eq!(
        mouse_report_bytes(TermMode::MOUSE_REPORT_CLICK, 0, press),
        Some(vec![0x1b, b'[', b'M', 32, 33, 33])
    );

    let release = TerminalMouseEvent {
        action: TerminalMouseAction::Release,
        button: Some(TerminalMouseButton::Right),
        position,
        modifiers: TerminalModifiers {
            control: true,
            ..TerminalModifiers::default()
        },
    };
    assert_eq!(
        mouse_report_bytes(TermMode::MOUSE_REPORT_CLICK, 0, release),
        Some(vec![0x1b, b'[', b'M', 51, 37, 35])
    );

    assert!(
        mouse_report_from_code(
            TermMode::MOUSE_REPORT_CLICK,
            0,
            TerminalMousePosition {
                column: 222,
                row: 222,
                pixel_x: 0,
                pixel_y: 0,
            },
            0,
            false,
            TerminalModifiers::default(),
        )
        .is_some()
    );
    assert!(
        mouse_report_from_code(
            TermMode::MOUSE_REPORT_CLICK,
            0,
            TerminalMousePosition {
                column: 223,
                row: 0,
                pixel_x: 0,
                pixel_y: 0,
            },
            0,
            false,
            TerminalModifiers::default(),
        )
        .is_none()
    );
}

#[test]
fn utf8_and_sgr_mouse_reports_preserve_extended_coordinates() {
    let utf8_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
    assert_eq!(
        mouse_report_from_code(
            utf8_mode,
            0,
            TerminalMousePosition {
                column: 95,
                row: 0,
                pixel_x: 0,
                pixel_y: 0
            },
            0,
            false,
            TerminalModifiers::default(),
        ),
        Some(vec![0x1b, b'[', b'M', 32, 0xc2, 0x80, 33])
    );
    assert!(
        mouse_report_from_code(
            utf8_mode,
            0,
            TerminalMousePosition {
                column: 2014,
                row: 2014,
                pixel_x: 0,
                pixel_y: 0,
            },
            0,
            false,
            TerminalModifiers::default(),
        )
        .is_some()
    );
    assert!(
        mouse_report_from_code(
            utf8_mode,
            0,
            TerminalMousePosition {
                column: 2015,
                row: 0,
                pixel_x: 0,
                pixel_y: 0,
            },
            0,
            false,
            TerminalModifiers::default(),
        )
        .is_none()
    );

    let sgr_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
    let release = TerminalMouseEvent {
        action: TerminalMouseAction::Release,
        button: Some(TerminalMouseButton::Right),
        position: TerminalMousePosition {
            column: 4,
            row: 2,
            pixel_x: 0,
            pixel_y: 0,
        },
        modifiers: TerminalModifiers {
            alt: true,
            ..TerminalModifiers::default()
        },
    };
    assert_eq!(
        mouse_report_bytes(sgr_mode, 0, release),
        Some(b"\x1b[<10;5;3m".to_vec())
    );
    assert!(mouse_report_bytes(sgr_mode, 3, release).is_none());
}

#[test]
fn sgr_pixel_mouse_reports_viewport_pixels_instead_of_cells() {
    let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE | TermMode::SGR_PIXEL_MOUSE;
    let press = TerminalMouseEvent {
        action: TerminalMouseAction::Press,
        button: Some(TerminalMouseButton::Left),
        position: TerminalMousePosition {
            column: 4,
            row: 2,
            pixel_x: 35,
            pixel_y: 47,
        },
        modifiers: TerminalModifiers::default(),
    };
    assert_eq!(
        mouse_report_bytes(mode, 0, press),
        Some(b"\x1b[<0;36;48M".to_vec())
    );
}

#[test]
fn mouse_motion_respects_drag_any_motion_shift_and_vi_modes() {
    let motion = |button, modifiers| TerminalMouseEvent {
        action: TerminalMouseAction::Move,
        button,
        position: TerminalMousePosition {
            column: 1,
            row: 1,
            pixel_x: 0,
            pixel_y: 0,
        },
        modifiers,
    };
    let drag_mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
    assert!(
        mouse_report_bytes(drag_mode, 0, motion(None, TerminalModifiers::default())).is_none()
    );
    assert_eq!(
        mouse_report_bytes(
            drag_mode,
            0,
            motion(
                Some(TerminalMouseButton::Middle),
                TerminalModifiers::default()
            ),
        ),
        Some(b"\x1b[<33;2;2M".to_vec())
    );
    assert!(
        mouse_report_bytes(
            drag_mode,
            0,
            motion(
                Some(TerminalMouseButton::Left),
                TerminalModifiers {
                    shift: true,
                    ..TerminalModifiers::default()
                },
            ),
        )
        .is_none()
    );
    assert_eq!(
        mouse_report_bytes(
            TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE,
            0,
            motion(
                None,
                TerminalModifiers {
                    control: true,
                    ..TerminalModifiers::default()
                },
            ),
        ),
        Some(b"\x1b[<51;2;2M".to_vec())
    );
    assert!(
        mouse_report_bytes(
            TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE | TermMode::VI,
            0,
            motion(None, TerminalModifiers::default()),
        )
        .is_none()
    );
}

#[test]
fn input_queue_coalesces_motion_and_scroll_without_crossing_event_barriers() {
    let session_id = SessionId::new_v4();
    let mut motion = QueuedTerminalInput::Mouse {
        session_id,
        event: TerminalMouseEvent {
            action: TerminalMouseAction::Move,
            button: None,
            position: TerminalMousePosition {
                column: 1,
                row: 1,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers: TerminalModifiers::default(),
        },
    };
    assert!(
        motion
            .merge(QueuedTerminalInput::Mouse {
                session_id,
                event: TerminalMouseEvent {
                    action: TerminalMouseAction::Move,
                    button: None,
                    position: TerminalMousePosition {
                        column: 3,
                        row: 2,
                        pixel_x: 0,
                        pixel_y: 0
                    },
                    modifiers: TerminalModifiers::default(),
                },
            })
            .is_ok()
    );
    let ClientRequest::Mouse { event, .. } = motion.request() else {
        panic!("coalesced motion changed request kind");
    };
    assert_eq!(
        event.position,
        TerminalMousePosition {
            column: 3,
            row: 2,
            pixel_x: 0,
            pixel_y: 0
        }
    );

    let scroll_event = |y| TerminalScrollEvent {
        delta: eggie_protocol::TerminalScrollDelta {
            x: 0,
            y,
            unit: TerminalScrollUnit::Pixels,
        },
        phase: TerminalScrollPhase::Moved,
        position: TerminalMousePosition {
            column: 0,
            row: 0,
            pixel_x: 0,
            pixel_y: 0,
        },
        modifiers: TerminalModifiers::default(),
    };
    let mut scroll = QueuedTerminalInput::Scroll {
        session_id,
        event: scroll_event(100),
    };
    assert!(
        scroll
            .merge(QueuedTerminalInput::Scroll {
                session_id,
                event: scroll_event(250),
            })
            .is_ok()
    );
    let ClientRequest::Scroll { event, .. } = scroll.request() else {
        panic!("coalesced scroll changed request kind");
    };
    assert_eq!(event.delta.y, 350);
}

#[test]
fn mouse_and_focus_reports_round_trip_through_the_real_pty() {
    if !Command::new("sh")
        .args(["-c", "command -v python3 >/dev/null 2>&1"])
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }

    let _pty_guard = PTY_TEST_LOCK.lock();
    let session = TerminalSession::spawn_default(
        ProjectId::new_v4(),
        std::env::current_dir().unwrap(),
        TerminalSize {
            columns: 80,
            rows: 24,
            ..TerminalSize::default()
        },
        TerminalAppearance::default(),
    )
    .unwrap();
    let script = "import os,sys,termios,tty; old=termios.tcgetattr(0); tty.setraw(0); sys.stdout.write('\\x1b[?1002h\\x1b[?1006h\\x1b[?1004h'); sys.stdout.flush(); data=b''; exec(\"while len(data)<12:\\n data+=os.read(0,12-len(data))\"); sys.stdout.write('\\x1b[?1002l\\x1b[?1006l\\x1b[?1004l\\x1b[?1049h\\x1b[?1007h'); sys.stdout.flush(); scroll=b''; exec(\"while len(scroll)<9:\\n scroll+=os.read(0,9-len(scroll))\"); sys.stdout.write('\\x1b[?1049l'); sys.stdout.flush(); termios.tcsetattr(0,termios.TCSADRAIN,old); print('\\r\\nEGGIE_POINTER:'+data.hex()+':'+scroll.hex())";
    session
        .input(
            format!("python3 -c \"{}\"\r", script.replace('"', "\\\"")).into_bytes(),
            1,
        )
        .unwrap();

    let mode_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let modes = session.snapshot().input_modes;
        if modes.mouse_tracking == TerminalMouseTracking::Drag
            && modes.mouse_encoding == TerminalMouseEncoding::Sgr
            && modes.focus_reporting
        {
            break;
        }
        assert!(
            Instant::now() < mode_deadline,
            "child application did not enable pointer protocols: {modes:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }

    session
        .mouse(TerminalMouseEvent {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::Left),
            position: TerminalMousePosition {
                column: 4,
                row: 2,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers: TerminalModifiers::default(),
        })
        .unwrap();
    session.focus(false).unwrap();

    let alternate_scroll_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let modes = session.snapshot().input_modes;
        if modes.alternate_screen && modes.alternate_scroll && !modes.captures_mouse() {
            break;
        }
        assert!(
            Instant::now() < alternate_scroll_deadline,
            "child application did not enable alternate scroll: {modes:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    session
        .scroll(TerminalScrollEvent {
            delta: eggie_protocol::TerminalScrollDelta {
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
        })
        .unwrap();

    let output_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let lines = session.snapshot().plain_lines();
        if lines.iter().any(|line| {
            line.contains("EGGIE_POINTER:1b5b3c303b353b334d1b5b4f:1b4f411b4f411b4f41")
        }) {
            break;
        }
        assert!(
            Instant::now() < output_deadline,
            "mouse/focus reports did not round-trip through PTY: {lines:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }

    session
        .input(
            b"for i in {1..40}; do echo EGGIE_SCROLL_$i; done\r".to_vec(),
            2,
        )
        .unwrap();
    let history_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let lines = session.snapshot().plain_lines();
        if lines.iter().any(|line| line.contains("EGGIE_SCROLL_40")) {
            break;
        }
        assert!(
            Instant::now() < history_deadline,
            "scrollback fixture did not reach the terminal: {lines:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(session.terminal.lock().grid().display_offset(), 0);
    let bottom_snapshot = session.snapshot();
    session
        .scroll(TerminalScrollEvent {
            delta: eggie_protocol::TerminalScrollDelta {
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
        })
        .unwrap();
    assert_eq!(session.terminal.lock().grid().display_offset(), 3);
    let history_snapshot = session.snapshot();
    assert!(history_snapshot.revision > bottom_snapshot.revision);
    assert_ne!(
        history_snapshot.plain_lines(),
        bottom_snapshot.plain_lines()
    );
    assert!(!history_snapshot.cells.is_empty());
    assert!(
        history_snapshot
            .cells
            .iter()
            .all(|cell| cell.line < history_snapshot.size.rows)
    );
    assert_eq!(history_snapshot.cursor_shape, TerminalCursorShape::Hidden);

    session
        .scroll(TerminalScrollEvent {
            delta: eggie_protocol::TerminalScrollDelta {
                x: 0,
                y: -TERMINAL_SCROLL_DELTA_SCALE,
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
        })
        .unwrap();
    assert_eq!(session.terminal.lock().grid().display_offset(), 0);
    let returned_snapshot = session.snapshot();
    assert!(returned_snapshot.revision > history_snapshot.revision);
    assert_ne!(
        returned_snapshot.plain_lines(),
        history_snapshot.plain_lines()
    );
    assert!(
        returned_snapshot
            .plain_lines()
            .iter()
            .any(|line| line.contains("EGGIE_SCROLL_40")),
        "scrolling back to offset zero must restore the latest output"
    );
    session.terminate();
}

fn osc_listener_state() -> ListenerState {
    ListenerState::new(
        SessionId::new_v4(),
        TerminalSize::default(),
        TerminalAppearance::default(),
        Arc::new(AtomicU64::new(0)),
    )
}

#[test]
fn bell_is_forwarded_once_and_throttles_a_burst() {
    let state = osc_listener_state();
    // First bell is forwarded and publishes an OSC event.
    assert!(state.ring_bell());
    let update = state
        .osc_events
        .wait_after(0, Duration::ZERO)
        .expect("first bell publishes an OSC event");
    assert_eq!(update.events.len(), 1);
    assert_eq!(update.events[0].payload, TerminalOscEventPayload::Bell);

    // A second bell inside the throttle window is dropped, so the revision does not advance.
    let revision_after_first = state.osc_events.revision();
    assert!(!state.ring_bell());
    assert_eq!(state.osc_events.revision(), revision_after_first);
}

#[test]
fn osc_reported_locations_distinguish_local_and_remote_hosts() {
    let local = parse_reported_location("file:///Users/test/My%20Project").unwrap();
    assert_eq!(local.path, "/Users/test/My Project");
    assert!(local.local);
    assert_eq!(local.user, None);

    let remote =
        parse_reported_location("file://alice@example.invalid/home/alice/repo").unwrap();
    assert_eq!(remote.user.as_deref(), Some("alice"));
    assert_eq!(remote.host.as_deref(), Some("example.invalid"));
    assert_eq!(remote.path, "/home/alice/repo");
    assert!(!remote.local);

    let state = osc_listener_state();
    state.report_working_directory("file:///tmp/working");
    state.update_remote_host("bob@remote.invalid");
    state.report_working_directory("/home/bob/repo");
    assert_eq!(
        *state.reported_location.read(),
        Some(TerminalReportedLocation {
            user: Some("bob".to_owned()),
            host: Some("remote.invalid".to_owned()),
            path: "/home/bob/repo".to_owned(),
            local: false,
        })
    );
}

#[test]
fn osc_133_tracks_prompt_command_output_and_aborted_input() {
    let mut tracker = ShellIntegrationTracker::default();
    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::PromptStart,
        options: String::new(),
    }, 0, 0);
    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::InputStart,
        options: "cmdline=echo hello".to_owned(),
    }, 0, 0);
    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::CommandFinished,
        options: "130".to_owned(),
    }, 0, 0);
    assert_eq!(tracker.snapshot().phase, TerminalSemanticPhase::None);
    assert!(tracker.snapshot().history.is_empty());

    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::InputStart,
        options: "cmdline=printf ok".to_owned(),
    }, 0, 0);
    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::OutputStart,
        options: String::new(),
    }, 0, 0);
    tracker.update(SemanticPrompt {
        action: SemanticPromptAction::CommandFinished,
        options: "7".to_owned(),
    }, 0, 0);
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.phase, TerminalSemanticPhase::None);
    assert_eq!(snapshot.history.len(), 1);
    assert_eq!(
        snapshot.history[0].command_line.as_deref(),
        Some("printf ok")
    );
    assert_eq!(snapshot.history[0].exit_code, Some(7));
}

#[test]
fn kitty_notification_multipart_is_bounded_sanitized_and_published_once() {
    let state = osc_listener_state();
    state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
        code: 99,
        payload: "i=build\u{1b}:p=title:d=0;Build".to_owned(),
        terminator: "\x1b\\".to_owned(),
    });
    assert_eq!(state.osc_events.revision(), 0);
    state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
        code: 99,
        payload: "i=build\u{1b}:p=body;Finished".to_owned(),
        terminator: "\x1b\\".to_owned(),
    });

    let update = state
        .osc_events
        .wait_after(0, Duration::ZERO)
        .expect("completed notification publishes");
    assert_eq!(update.events.len(), 1);
    let TerminalOscEventPayload::Notification { notification } = &update.events[0].payload
    else {
        panic!("unexpected OSC event: {:?}", update.events[0].payload);
    };
    assert_eq!(notification.id, "build");
    assert_eq!(notification.title, "Build");
    assert_eq!(notification.body, "Finished");
    assert!(state.live_notifications.lock().contains("build"));

    state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
        code: 99,
        payload: "p=close;".to_owned(),
        terminator: "\x1b\\".to_owned(),
    });
    assert_eq!(
        state.osc_events.revision(),
        1,
        "close without an id is a no-op"
    );
    state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
        code: 99,
        payload: "i=build:p=close;".to_owned(),
        terminator: "\x1b\\".to_owned(),
    });
    assert!(!state.live_notifications.lock().contains("build"));
    assert_eq!(state.osc_events.revision(), 2);
}

#[test]
fn kitty_rich_clipboard_commits_multipart_data_and_aliases_once() {
    let state = osc_listener_state();
    let text_mime = BASE64.encode("text/plain;charset=utf-8");
    let alias_mime = BASE64.encode("text/plain");
    state.handle_kitty_clipboard("type=write:id=clip", "\x1b\\");
    state.handle_kitty_clipboard(
        &format!("type=walias:id=clip:mime={text_mime};{alias_mime}"),
        "\x1b\\",
    );
    state.handle_kitty_clipboard(
        &format!(
            "type=wdata:id=clip:mime={text_mime};{}",
            BASE64.encode("hello")
        ),
        "\x1b\\",
    );
    state.handle_kitty_clipboard("type=wdata:id=clip", "\x1b\\");

    let update = state
        .osc_events
        .wait_after(0, Duration::ZERO)
        .expect("clipboard write publishes");
    let TerminalOscEventPayload::ClipboardWrite { contents, .. } = &update.events[0].payload
    else {
        panic!("unexpected OSC event: {:?}", update.events[0].payload);
    };
    assert_eq!(contents.len(), 2);
    assert!(contents.iter().all(|content| content.data == b"hello"));
    assert!(
        contents
            .iter()
            .any(|content| content.mime_type == "text/plain")
    );
    assert!(
        contents
            .iter()
            .any(|content| content.mime_type == "text/plain;charset=utf-8")
    );
}

#[test]
fn iterm2_variables_and_direct_copy_use_terminal_state() {
    let state = osc_listener_state();
    *state.title.write() = "build tab".to_owned();
    state.report_working_directory("file://alice@remote.invalid/work/repo");
    state.handle_iterm2_command(
        &format!("SetUserVar=branch={}", BASE64.encode("feature/osc")),
        "\x1b\\",
    );

    assert_eq!(
        state.iterm2_variable("session.name").as_deref(),
        Some("build tab")
    );
    assert_eq!(
        state.iterm2_variable("session.path").as_deref(),
        Some("/work/repo")
    );
    assert_eq!(
        state.iterm2_variable("session.hostname").as_deref(),
        Some("remote.invalid")
    );
    assert_eq!(
        state.iterm2_variable("user.branch").as_deref(),
        Some("feature/osc")
    );
    assert_eq!(state.iterm2_variable("unknown"), None);

    state.handle_iterm2_command(&format!("Copy=:{}", BASE64.encode("copy me")), "\x1b\\");
    let update = state
        .osc_events
        .wait_after(0, Duration::ZERO)
        .expect("iTerm2 direct copy publishes a clipboard event");
    let TerminalOscEventPayload::ClipboardWrite { contents, .. } = &update.events[0].payload
    else {
        panic!("unexpected OSC event: {:?}", update.events[0].payload);
    };
    assert_eq!(contents[0].data, b"copy me");
}

#[test]
fn kitty_file_transfer_preserves_safe_hierarchy_and_streams_zlib() {
    use flate2::{Compression, write::ZlibEncoder};

    let state = osc_listener_state();
    state.handle_kitty_file_transfer("ac=send;id=transfer-1", "\x1b\\");
    let update = state
        .osc_events
        .wait_after(0, Duration::ZERO)
        .expect("incoming transfer asks for authorization");
    let TerminalOscEventPayload::FileTransfer { offer } = &update.events[0].payload else {
        panic!("unexpected OSC event: {:?}", update.events[0].payload);
    };
    let destination = std::env::temp_dir().join(format!(
        "eggie-osc-file-transfer-test-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&destination).unwrap();
    state
        .complete_file_transfer(offer.request_id, Some(destination.clone()))
        .unwrap();

    let contents = b"streamed zlib file contents";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(contents).unwrap();
    let compressed = encoder.finish().unwrap();
    let encoded_name = BASE64.encode("project/src/main.txt");
    state.handle_kitty_file_transfer(
        &format!(
            "ac=file;id=transfer-1;fid=file-1;ft=regular;n={encoded_name};zip=zlib;sz={}",
            contents.len()
        ),
        "\x1b\\",
    );
    state.handle_kitty_file_transfer(
        &format!(
            "ac=end_data;id=transfer-1;fid=file-1;d={}",
            BASE64.encode(compressed)
        ),
        "\x1b\\",
    );
    state.handle_kitty_file_transfer("ac=finish;id=transfer-1", "\x1b\\");

    assert_eq!(
        fs::read(destination.join("project/src/main.txt")).unwrap(),
        contents
    );
    assert_eq!(
        safe_kitty_transfer_path(Some(&BASE64.encode("../../escape")), "fallback"),
        PathBuf::from("fallback")
    );
    fs::remove_dir_all(destination).unwrap();
}

#[test]
fn handshake_acceptance_rules() {
    // Protocol mismatch is always rejected.
    assert!(!handshake_accepted(2, "any", 1, "any"));

    // Same protocol, same build id is always accepted.
    assert!(handshake_accepted(1, "build-a", 1, "build-a"));

    // Same protocol but different build id: rejected in debug builds
    // (so a recompile swaps the daemon), accepted in release builds
    // (so in-place updates keep a compatible daemon).
    assert_eq!(
        handshake_accepted(1, "build-a", 1, "build-b"),
        !cfg!(debug_assertions)
    );
}
