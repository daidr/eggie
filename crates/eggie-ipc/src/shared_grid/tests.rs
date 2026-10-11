use super::*;
use crate::wire::{
    WireFrameKind, classify_wire_header, read_daemon_response_with_grid, read_message_body,
    read_wire_header,
};
use eggie_protocol::{
    DaemonResponse, TerminalCell, TerminalCursorShape, TerminalInputModes, TerminalSize,
};
use std::io::Cursor;

fn snapshot(revision: u64, columns: u16, rows: u16) -> Arc<TerminalSnapshot> {
    let rows = (0..rows)
        .map(|line| {
            TerminalRow::owned(
                (0..columns)
                    .map(|column| TerminalCell {
                        line,
                        column,
                        character: 'a',
                        zerowidth: vec![],
                        foreground: TerminalColor::Named(256),
                        background: TerminalColor::Named(257),
                        underline_color: None,
                        hyperlink: None,
                        flags: 0,
                    })
                    .collect(),
            )
        })
        .collect();
    Arc::new(TerminalSnapshot {
        prompt_editable: false,
        prompt_input: None,
        session_id: uuid::Uuid::nil(),
        size: TerminalSize {
            columns,
            rows: rows_len(&rows),
            cell_width: 8,
            cell_height: 18,
        },
        cells: TerminalCells::from_rows(rows),
        color_overrides: vec![],
        cursor_line: 0,
        cursor_column: 0,
        cursor_shape: TerminalCursorShape::Block,
        cursor_width: 1,
        cursor_blinking: true,
        title: "grid test".into(),
        revision,
        last_input_sequence: revision,
        input_modes: TerminalInputModes::default(),
        images: vec![],
        image_placements: vec![],
        selection: None,
        detected_links: vec![],
        display_offset: 0,
        history_size: 0,
    })
}
fn rows_len(rows: &Vec<TerminalRow>) -> u16 {
    rows.len() as u16
}
fn change(base: &Arc<TerminalSnapshot>, line: usize, character: char) -> Arc<TerminalSnapshot> {
    let mut frame = (**base).clone();
    frame.revision += 1;
    let mut rows = frame.cells.rows().to_vec();
    let mut cells = rows[line]
        .iter()
        .map(TerminalCellView::to_owned)
        .collect::<Vec<_>>();
    cells[0].character = character;
    rows[line] = TerminalRow::owned(cells);
    frame.cells = TerminalCells::from_rows(rows);
    Arc::new(frame)
}
fn send(
    writer: &mut GridWriter,
    reader: &mut GridReader,
    frame: Arc<TerminalSnapshot>,
    base: u64,
) -> Arc<TerminalSnapshot> {
    let mut wire = vec![];
    assert!(
        writer
            .send(&mut wire, &mut vec![], frame.clone(), base)
            .unwrap()
    );
    let response = read_daemon_response_with_grid(&mut Cursor::new(wire), &mut vec![], reader)
        .unwrap()
        .unwrap();
    let DaemonResponse::Snapshot { snapshot } = response else {
        panic!("expected snapshot");
    };
    assert_eq!(*snapshot, *frame);
    snapshot
}
fn update(writer: &mut GridWriter, frame: Arc<TerminalSnapshot>, base: u64) -> GridUpdate {
    let mut wire = vec![];
    assert!(writer.send(&mut wire, &mut vec![], frame, base).unwrap());
    let mut cursor = Cursor::new(wire);
    let (kind, len) = classify_wire_header(read_wire_header(&mut cursor).unwrap().unwrap());
    assert_eq!(kind, WireFrameKind::ShmGrid);
    read_message_body(&mut cursor, &mut vec![], len).unwrap()
}

#[test]
fn mapped_unicode_styles_and_links_round_trip_without_owned_cells() {
    let mut frame = (*snapshot(1, 4, 3)).clone();
    let mut cells = frame.cells.to_vec();
    cells[0].character = '界';
    cells[0].zerowidth = vec!['\u{301}', '\u{fe0f}'];
    cells[0].foreground = TerminalColor::Rgb(0x123456ff);
    cells[0].background = TerminalColor::Indexed(255);
    cells[0].underline_color = Some(TerminalColor::Named(268));
    cells[0].hyperlink = Some("https://例子.com/🙂".into());
    cells[0].flags = 0xffff;
    cells[1].hyperlink = Some("".into());
    frame.cells = cells.into();
    let frame = Arc::new(frame);
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let mapped = send(&mut writer, &mut reader, frame.clone(), 0);
    assert!(!mapped.cells.rows()[0].same_storage(&frame.cells.rows()[0]));
    // Producer teardown cannot invalidate cells referenced by the renderer.
    let name = writer.region.as_ref().unwrap().name.clone();
    drop(writer);
    drop(reader);
    assert_eq!(*mapped, *frame);
    assert_eq!(
        unsafe { libc::shm_open(name.as_ptr(), libc::O_RDWR, 0) },
        -1
    );
}

#[test]
fn unchanged_rows_share_leases_and_retained_frames_never_change() {
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let first = snapshot(1, 6, 4);
    let mapped_first = send(&mut writer, &mut reader, first.clone(), 0);
    let next = change(&first, 2, 'b');
    let mapped_next = send(&mut writer, &mut reader, next.clone(), 1);
    assert!(mapped_next.cells.rows()[0].same_storage(&mapped_first.cells.rows()[0]));
    assert!(!mapped_next.cells.rows()[2].same_storage(&mapped_first.cells.rows()[2]));
    assert_eq!(*mapped_first, *first);
    // Resize creates a pool; old leases remain valid after the producer unmaps its old pool.
    let resized = snapshot(3, 20, 2);
    send(&mut writer, &mut reader, resized, 2);
    assert_eq!(*mapped_next, *next);
    assert_eq!(*mapped_first, *first);
}

#[test]
fn exhausted_slots_fall_back_and_recover_after_last_arc_drops() {
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let mut frame = snapshot(1, 4, 2);
    let mut held = vec![];
    for base in 0..3 {
        held.push(send(&mut writer, &mut reader, frame.clone(), base));
        frame = change(&frame, 0, char::from_u32('b' as u32 + base as u32).unwrap());
    }
    assert!(
        !writer
            .send(&mut vec![], &mut vec![], frame.clone(), 3)
            .unwrap()
    );
    assert_eq!(held[0].cells.get(0).unwrap().character, 'a');
    reader.accept_owned(frame.clone());
    drop(held);
    let next = change(&frame, 0, 'z');
    send(&mut writer, &mut reader, next, 4);
}

#[test]
fn new_subscriber_and_revision_gap_receive_complete_frames() {
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let frame = snapshot(1, 4, 3);
    send(&mut writer, &mut reader, frame.clone(), 0);
    let next = change(&frame, 1, 'x');
    let update = update(&mut writer, next.clone(), 0);
    assert_eq!(update.base_revision, 0);
    assert_eq!(update.changed.len(), 3);
    reader.receive(update).unwrap();
    let mut other_writer = GridWriter::default();
    let mut other_reader = GridReader::default();
    let other = send(&mut other_writer, &mut other_reader, next, 0);
    assert!(
        !other.cells.rows()[0].same_storage(&reader.previous.as_ref().unwrap().cells.rows()[0])
    );
}

#[test]
fn invalid_shape_base_generation_and_duplicate_row_are_rejected() {
    for fault in 0..5 {
        let mut writer = GridWriter::default();
        let mut reader = GridReader::default();
        let frame = snapshot(1, 4, 3);
        let mut update = update(&mut writer, frame, 0);
        match fault {
            0 => update.changed[0].generation += 1,
            1 => update.base_revision = 12,
            2 => update.changed[1].line = 0,
            3 => {
                update.changed.pop();
            }
            _ => update.capacity = u32::MAX,
        }
        assert!(reader.receive(update).is_err(), "fault {fault}");
    }
}

#[test]
fn packed_rows_reject_invalid_scalars_offsets_utf8_and_colors() {
    let mut cells = snapshot(1, 4, 1).cells.to_vec();
    cells[0].zerowidth = vec!['\u{301}'];
    cells[0].hyperlink = Some("ok".into());
    let row = TerminalRow::owned(cells);
    let mut valid = vec![];
    pack_row(Some(&row), &mut valid);
    assert_eq!(validate_row(&valid, 4).unwrap(), 4);
    for fault in 0..7 {
        let mut data = valid.clone();
        match fault {
            0 => put(&mut data, 8 + 4, 0xd800),
            1 => put(&mut data, 8 + 32, 1),
            2 => put(&mut data, 8 + 36, u32::MAX),
            3 => {
                let offset = word(&data, 8 + 40) as usize;
                data[offset] = 0xff;
            }
            4 => put(&mut data, 8 + 12, 7),
            5 => put(&mut data, 8 + 20, 1), // value 257 cannot be an indexed color
            _ => data[8 + RECORD..8 + RECORD + 2].copy_from_slice(&0_u16.to_le_bytes()),
        }
        assert!(validate_row(&data, 4).is_err(), "fault {fault}");
    }
}

fn single_cell_delta(
    frame: &TerminalSnapshot,
    base_revision: u64,
) -> eggie_protocol::TerminalSnapshotDelta {
    eggie_protocol::TerminalSnapshotDelta {
        prompt_editable: frame.prompt_editable,
        prompt_input: frame.prompt_input.clone(),
        session_id: frame.session_id,
        base_revision,
        size: frame.size,
        cells: vec![frame.cells.get(0).unwrap().to_owned()],
        cleared: vec![],
        color_overrides: frame.color_overrides.clone(),
        cursor_line: frame.cursor_line,
        cursor_column: frame.cursor_column,
        cursor_shape: frame.cursor_shape,
        cursor_width: frame.cursor_width,
        cursor_blinking: frame.cursor_blinking,
        title: frame.title.clone(),
        revision: frame.revision,
        last_input_sequence: frame.last_input_sequence,
        input_modes: frame.input_modes,
        images: None,
        image_placements: None,
        selection: frame.selection,
        detected_links: frame.detected_links.clone(),
        display_offset: frame.display_offset,
        history_size: frame.history_size,
    }
}

#[test]
#[ignore = "repeatable local Unix socket transport benchmark; run with --release --ignored --nocapture"]
fn shared_grid_socket_ab_benchmark() {
    use crate::wire::write_wire_message;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        thread,
        time::Instant,
    };
    const WARMUP: usize = 60;
    const SAMPLES: usize = 600;
    for dense in [false, true] {
        let a = snapshot(1, 240, 80);
        let b = if dense {
            let mut b = (*a).clone();
            let mut cells = b.cells.to_vec();
            for cell in &mut cells {
                cell.character = 'b';
            }
            b.cells = cells.into();
            Arc::new(b)
        } else {
            change(&a, 0, 'b')
        };
        for shared in [false, true] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let a = a.clone();
            let b = b.clone();
            let worker = thread::spawn(move || {
                let mut writer = GridWriter::default();
                let mut buffer = vec![];
                for index in 0..WARMUP + SAMPLES {
                    server.read_exact(&mut [0; 1]).unwrap();
                    let mut frame = if index % 2 == 0 {
                        (*a).clone()
                    } else {
                        (*b).clone()
                    };
                    frame.revision = index as u64 + 1;
                    let frame = Arc::new(frame);
                    if shared {
                        assert!(
                            writer
                                .send(&mut server, &mut buffer, frame, index as u64)
                                .unwrap()
                        );
                    } else if dense || index == 0 {
                        write_wire_message(
                            &mut server,
                            &mut buffer,
                            &DaemonResponse::Snapshot { snapshot: frame },
                        )
                        .unwrap();
                    } else {
                        write_wire_message(
                            &mut server,
                            &mut buffer,
                            &DaemonResponse::SnapshotDelta {
                                delta: single_cell_delta(&frame, index as u64),
                            },
                        )
                        .unwrap();
                    }
                }
                writer.region.as_ref().map_or(0, |r| r.map.len())
            });
            let mut incoming = std::io::BufReader::new(client.try_clone().unwrap());
            let mut buffer = vec![];
            let mut grid = GridReader::default();
            let mut socket = client;
            let mut previous: Option<Arc<TerminalSnapshot>> = None;
            let mut times = vec![];
            let mut bytes = 0;
            // The one-byte wakeup avoids measuring request serialization in both modes. Receive
            // uses the production persistent wire reader, including validation and delta apply.
            for index in 0..WARMUP + SAMPLES {
                let started = Instant::now();
                socket.write_all(&[1]).unwrap();
                let response =
                    read_daemon_response_with_grid(&mut incoming, &mut buffer, &mut grid)
                        .unwrap()
                        .unwrap();
                let frame = match response {
                    DaemonResponse::Snapshot { snapshot } => snapshot,
                    DaemonResponse::SnapshotDelta { delta } => {
                        Arc::new(previous.as_ref().unwrap().apply_delta(&delta).unwrap())
                    }
                    _ => panic!("unexpected frame"),
                };
                assert_eq!(
                    frame.cells.get(0).unwrap().character,
                    if index % 2 == 0 { 'a' } else { 'b' }
                );
                previous = Some(frame);
                if index >= WARMUP {
                    times.push(started.elapsed());
                    bytes += buffer.len() + 4;
                }
            }
            let pool = worker.join().unwrap();
            times.sort_unstable();
            eprintln!(
                "grid-ab 240x80 workload={} transport={} p50_us={:.1} p95_us={:.1} socket_bytes={} pool_bytes={}",
                if dense { "full-screen" } else { "one-cell" },
                if shared { "shared-rows" } else { "messagepack" },
                times[times.len() / 2].as_secs_f64() * 1e6,
                times[times.len() * 95 / 100].as_secs_f64() * 1e6,
                bytes / SAMPLES,
                pool
            );
        }
    }
}

#[test]
fn owned_base_with_omitted_empty_tail_can_receive_a_changed_last_row() {
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let mut first = (*snapshot(1, 4, 3)).clone();
    let mut rows = first.cells.rows().to_vec();
    rows[1] = TerminalRow::owned(vec![]);
    rows[2] = TerminalRow::owned(vec![]);
    first.cells = TerminalCells::from_rows(rows);
    let first = Arc::new(first);
    send(&mut writer, &mut reader, first.clone(), 0);
    let mut owned = (*first).clone();
    owned.cells = owned.cells.to_vec().into();
    assert_eq!(owned.cells.rows().len(), 1);
    reader.accept_owned(Arc::new(owned));
    let mut next = (*first).clone();
    next.revision = 2;
    let mut rows = next.cells.rows().to_vec();
    let mut cell = rows[0].cell(0).to_owned();
    cell.line = 2;
    rows[2] = TerminalRow::owned(vec![cell]);
    next.cells = TerminalCells::from_rows(rows);
    send(&mut writer, &mut reader, Arc::new(next), 1);
}

#[test]
fn shared_grid_preserves_and_clears_editable_shell_metadata() {
    let mut writer = GridWriter::default();
    let mut reader = GridReader::default();
    let mut frame = (*snapshot(1, 80, 24)).clone();
    frame.prompt_editable = true;
    frame.prompt_input = Some(eggie_protocol::PromptInput {
        text: "git 中".into(),
        cursor: "git 中".len(),
        shell: "zsh".into(),
        native: true,
        ..Default::default()
    });
    send(&mut writer, &mut reader, Arc::new(frame.clone()), 0);
    frame.revision += 1;
    frame.prompt_input = None;
    send(&mut writer, &mut reader, Arc::new(frame.clone()), 1);
    frame.revision += 1;
    frame.prompt_editable = false;
    send(&mut writer, &mut reader, Arc::new(frame), 2);
}
