use std::sync::Arc;

use alacritty_ffi::alacritty_terminal::clipboard::{
    TerminalClipboardContent, TerminalClipboardLocation,
};
use alacritty_ffi::alacritty_terminal::index::{Direction, Side};
use alacritty_ffi::alacritty_terminal::protocols::progress::ProgressState;
use alacritty_ffi::alacritty_terminal::protocols::shell::Phase;
use alacritty_ffi::alacritty_terminal::selection::SelectionType;
use alacritty_ffi::alacritty_terminal::term::{Osc52, TermMode};
use alacritty_ffi::alacritty_terminal::vi_mode::ViMotion;
use alacritty_ffi::keyboard::{Key, KeyEvent, Modifiers, NamedKey};
use alacritty_ffi::mouse::MouseButton;
use alacritty_ffi::snapshot::{CELL_DEFAULT_BACKGROUND, CELL_SELECTED, Cell, Snapshot};
use alacritty_ffi::{ClipboardProvider, Engine, HostEvent, Options, WheelAction};
use parking_lot::Mutex;

fn headless() -> Engine {
    Engine::headless(Options::default(), None)
}

fn events(engine: &mut Engine) -> Vec<HostEvent> {
    std::iter::from_fn(|| engine.poll()).collect()
}

fn output(engine: &mut Engine) -> Vec<u8> {
    events(engine)
        .into_iter()
        .filter_map(|event| match event {
            HostEvent::Output(bytes) => Some(bytes),
            _ => None,
        })
        .flatten()
        .collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[derive(Clone, Default)]
struct TestClipboard(Arc<Mutex<Vec<TerminalClipboardContent>>>);

impl ClipboardProvider for TestClipboard {
    fn formats(&mut self, _location: TerminalClipboardLocation) -> Option<Vec<String>> {
        Some(self.0.lock().iter().map(|content| content.mime_type.clone()).collect())
    }

    fn read(&mut self, _location: TerminalClipboardLocation, mime_type: &str) -> Option<Vec<u8>> {
        let contents = self.0.lock();
        contents.iter().find(|content| content.mime_type == mime_type).map(|c| c.data.clone())
    }

    fn write(
        &mut self,
        _location: TerminalClipboardLocation,
        contents: &[TerminalClipboardContent],
    ) -> bool {
        *self.0.lock() = contents.to_vec();
        true
    }
}

#[test]
fn feed_renders_text_with_resolved_colors() {
    let mut engine = headless();
    engine.feed(b"hello \x1b[31mred\x1b[0m");
    assert!(matches!(events(&mut engine).as_slice(), [HostEvent::Wakeup]));

    let snapshot = engine.snapshot();
    assert_eq!((snapshot.columns, snapshot.rows), (80, 24));
    assert!(snapshot.full_damage);
    let h = snapshot.cell(0, 0).unwrap();
    assert_eq!(h.codepoint, 'h' as u32);
    assert_eq!(h.fg, 0xd8d8d8);
    assert_ne!(h.flags & CELL_DEFAULT_BACKGROUND, 0);
    assert_eq!(snapshot.cell(6, 0).unwrap().fg, 0xac4242);
    assert_eq!((snapshot.cursor.column, snapshot.cursor.row), (9, 0));
    assert!(engine.screen_text().starts_with("hello red"));

    // Only the edited line is damaged afterwards.
    engine.feed(b"\r\nx");
    let snapshot = engine.snapshot();
    assert!(!snapshot.full_damage);
    assert!(snapshot.damage.iter().any(|line| line.row == 1));
}

#[test]
fn title_and_bell_events() {
    let mut engine = headless();
    engine.feed(b"\x1b]2;my title\x07\x07");
    let events = events(&mut engine);
    assert!(events.iter().any(|e| matches!(e, HostEvent::Title(Some(t)) if t == "my title")));
    assert!(events.iter().any(|e| matches!(e, HostEvent::Bell)));
}

#[test]
fn protocol_queries_are_answered_by_the_engine() {
    let mut engine = headless();
    engine.feed(b"\x1b[c");
    assert!(output(&mut engine).starts_with(b"\x1b[?"));

    engine.feed(b"\x1b]10;?\x07");
    assert!(contains(&output(&mut engine), b"10;rgb:d8d8/d8d8/d8d8"));

    engine.feed(b"\x1b[14t");
    assert!(contains(&output(&mut engine), b"\x1b[4;384;640t"));
}

#[test]
fn progress_and_osc9_notifications() {
    let mut engine = headless();
    engine.feed(b"\x1b]9;4;1;42\x07");
    let progress = events(&mut engine).into_iter().find_map(|event| match event {
        HostEvent::ProgressChanged(progress) => Some(progress),
        _ => None,
    });
    let progress = progress.unwrap();
    assert_eq!((progress.state, progress.percent), (ProgressState::Normal, 42));
    assert!(engine.next_deadline().is_some());

    engine.feed(b"\x1b]9;Build finished\x07");
    let notification = events(&mut engine).into_iter().find_map(|event| match event {
        HostEvent::NotificationShow(notification) => Some(notification),
        _ => None,
    });
    assert_eq!(notification.unwrap().title, "Build finished");
}

#[test]
fn osc99_notification_activation_reports_back() {
    let mut engine = headless();
    engine.feed(b"\x1b]99;i=job:a=report;Done\x1b\\");
    let serial = events(&mut engine)
        .into_iter()
        .find_map(|event| match event {
            HostEvent::NotificationShow(notification) => Some(notification.serial),
            _ => None,
        })
        .unwrap();
    engine.notification_activated(serial, None);
    assert!(contains(&output(&mut engine), b"\x1b]99;i=job;"));
}

#[test]
fn working_directory_and_shell_integration() {
    let mut engine = headless();
    engine.feed(b"\x1b]7;file://box/tmp/a%20b\x07");
    assert_eq!(engine.working_directory(), Some(("box".into(), "/tmp/a b".into())));

    engine.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;3\x07");
    let shell = engine.shell_info();
    assert_eq!(shell.phase, Phase::Unknown);
    assert_eq!(shell.last_command.as_deref(), Some("ls"));
    assert_eq!(shell.last_exit_status, Some(3));
    assert_eq!(engine.last_command_output().as_deref(), Some("out"));
}

#[test]
fn program_status_records() {
    let mut engine = headless();
    engine.feed(b"\x1b]7501;state=working:app=build:progress=40\x1b\\");
    assert!(events(&mut engine).iter().any(|e| matches!(e, HostEvent::ProgramStatusChanged)));
    let statuses = engine.program_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].app.as_deref(), Some("build"));
    assert_eq!(statuses[0].record.progress, Some(40));
}

#[test]
fn osc52_uses_clipboard_provider() {
    let mut engine =
        Engine::headless(Options { osc52: Osc52::CopyPaste, ..Options::default() }, None);
    let clipboard = TestClipboard::default();
    engine.set_clipboard(Some(Box::new(clipboard.clone())));

    engine.feed(b"\x1b]52;c;aGVsbG8=\x07");
    events(&mut engine);
    assert_eq!(clipboard.0.lock()[0].data, b"hello");

    engine.feed(b"\x1b]52;c;?\x07");
    assert!(contains(&output(&mut engine), b"aGVsbG8="));

    // Unfocused terminals can't touch the clipboard.
    engine.set_focused(false);
    engine.feed(b"\x1b]52;c;Ynll\x07");
    events(&mut engine);
    assert_eq!(clipboard.0.lock()[0].data, b"hello");
}

#[test]
fn kitty_clipboard_lists_formats() {
    let mut engine = headless();
    let clipboard = TestClipboard::default();
    clipboard
        .0
        .lock()
        .push(TerminalClipboardContent { mime_type: "text/plain".into(), data: b"x".to_vec() });
    engine.set_clipboard(Some(Box::new(clipboard)));
    engine.feed(b"\x1b]5522;type=read;Lg==\x1b\\");
    let output = output(&mut engine);
    assert!(
        output.starts_with(b"\x1b]5522;type=read:status=OK"),
        "{:?}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn kitty_graphics_placements_are_snapshotted() {
    let mut engine = headless();
    // A 2x1 RGBA image, transmitted and displayed at the cursor.
    engine.feed(b"\x1b_Ga=T,f=32,s=2,v=1,i=7;/wAA/wD/AP8=\x1b\\");
    let reply = output(&mut engine);
    assert!(contains(&reply, b"\x1b_Gi=7;OK"), "{:?}", String::from_utf8_lossy(&reply));

    let snapshot = engine.snapshot();
    assert_eq!(snapshot.images.len(), 1);
    let image = snapshot.images[0];
    assert_eq!((image.image_id, image.width, image.height, image.rgba_len), (7, 2, 1, 8));
    assert_eq!((image.dest_x, image.dest_y, image.dest_width, image.dest_height), (0., 0., 2., 1.));
    let pixels = unsafe { std::slice::from_raw_parts(image.rgba, image.rgba_len) };
    assert_eq!(pixels, [0xff, 0, 0, 0xff, 0, 0xff, 0, 0xff]);
}

#[test]
fn keys_follow_terminal_modes() {
    let mut engine = headless();
    engine.key(&KeyEvent::new(Key::Character('c'), Modifiers::CONTROL));
    assert_eq!(output(&mut engine), b"\x03");

    engine.feed(b"\x1b[?1h");
    engine.key(&KeyEvent::new(Key::Named(NamedKey::ArrowUp), Modifiers::empty()));
    assert_eq!(output(&mut engine), b"\x1bOA");

    // Applications opt into the Kitty keyboard protocol.
    engine.feed(b"\x1b[>1u");
    engine.key(&KeyEvent::new(Key::Character('c'), Modifiers::CONTROL));
    assert_eq!(output(&mut engine), b"\x1b[99;5u");
}

#[test]
fn bracketed_paste_strips_escapes() {
    let mut engine = headless();
    engine.paste("a\nb");
    assert_eq!(output(&mut engine), b"a\rb");
    engine.feed(b"\x1b[?2004h");
    engine.paste("a\x1bb");
    assert_eq!(output(&mut engine), b"\x1b[200~ab\x1b[201~");
}

#[test]
fn mouse_reporting_and_scrolling() {
    let mut engine = headless();
    assert!(!engine.mouse_button(MouseButton::Left, true, 2, 3, Modifiers::empty()));

    engine.feed(b"\x1b[?1002h\x1b[?1006h");
    assert!(engine.mouse_button(MouseButton::Left, true, 2, 3, Modifiers::empty()));
    assert!(engine.mouse_move(3, 3, Modifiers::empty()));
    assert!(!engine.mouse_move(3, 3, Modifiers::empty()));
    assert!(engine.mouse_button(MouseButton::Left, false, 3, 3, Modifiers::empty()));
    assert_eq!(output(&mut engine), b"\x1b[<0;3;4M\x1b[<32;4;4M\x1b[<0;4;4m");
    // Shift bypasses mouse reporting for host selection.
    assert!(!engine.mouse_button(MouseButton::Left, true, 0, 0, Modifiers::SHIFT));

    let mut engine = headless();
    for i in 0..100 {
        engine.feed(format!("line {i}\r\n").as_bytes());
    }
    assert_eq!(engine.mouse_wheel(3, 0, 0, 0, Modifiers::empty()), WheelAction::Scrolled);
    assert_eq!(engine.snapshot().display_offset, 3);
    // Typing returns to the bottom.
    engine.key(&KeyEvent::new(Key::Character('x'), Modifiers::empty()));
    assert_eq!(engine.snapshot().display_offset, 0);
}

#[test]
fn search_selects_matches() {
    let mut engine = headless();
    engine.feed(b"foo bar baz");
    let found = engine.search("ba.", Direction::Right).unwrap().unwrap();
    assert_eq!((found.start().column.0, found.end().column.0), (4, 6));
    assert_eq!(engine.selection_text().as_deref(), Some("bar"));
    engine.search("ba.", Direction::Right).unwrap();
    assert_eq!(engine.selection_text().as_deref(), Some("baz"));
    assert_ne!(engine.snapshot().cell(8, 0).unwrap().flags & CELL_SELECTED, 0);
    assert!(engine.search("(", Direction::Right).is_err());
}

#[test]
fn focus_events_are_reported_when_requested() {
    let mut engine = headless();
    engine.set_focused(false);
    assert!(output(&mut engine).is_empty());
    engine.feed(b"\x1b[?1004h");
    engine.set_focused(true);
    assert_eq!(output(&mut engine), b"\x1b[I");
}

#[test]
fn synchronized_updates_time_out() {
    let mut engine = headless();
    engine.feed(b"\x1b[?2026hhidden");
    events(&mut engine);
    assert!(engine.next_deadline().is_some());
    assert!(!engine.screen_text().contains("hidden"));
    engine.feed(b"\x1b[?2026l");
    assert!(engine.screen_text().contains("hidden"));
}

#[cfg(unix)]
#[test]
fn pty_child_output_and_exit() {
    use std::time::{Duration, Instant};

    use alacritty_ffi::PtyOptions;

    let pty = PtyOptions {
        program: Some("/bin/sh".into()),
        args: vec!["-c".into(), "printf 'hi from pty'; exit 3".into()],
        drain_on_exit: true,
        ..PtyOptions::default()
    };
    let mut engine = Engine::spawn(Options::default(), pty, None).unwrap();
    assert!(engine.child_pid().is_some());

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exit = None;
    while exit.is_none() && Instant::now() < deadline {
        while let Some(event) = engine.poll() {
            if let HostEvent::ChildExit(code) = event {
                exit = Some(code);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(exit, Some(Some(3)));
    assert!(engine.screen_text().contains("hi from pty"));
}

#[test]
fn wakeup_fires_for_queued_events() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let wakeups = Arc::new(AtomicUsize::new(0));
    let counter = wakeups.clone();
    let mut engine = Engine::headless(
        Options::default(),
        Some(Box::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })),
    );
    engine.feed(b"text\x1b[c");
    assert!(wakeups.load(Ordering::Relaxed) > 0);
    let before = wakeups.load(Ordering::Relaxed);
    assert!(!output(&mut engine).is_empty());
    // Replies written while polling signal the host again.
    assert!(wakeups.load(Ordering::Relaxed) > before);
}

/// Cells compared by content, since combining character indices depend on update history.
fn assert_same_cells(left: &Snapshot, right: &Snapshot) {
    assert_eq!((left.columns, left.rows), (right.columns, right.rows));
    for (index, (a, b)) in left.cells.iter().zip(&right.cells).enumerate() {
        let position = (index % left.columns, index / left.columns);
        assert_eq!(left.cell_text(a), right.cell_text(b), "text at {position:?}");
        assert_eq!(
            (a.fg, a.bg, a.underline_color, a.flags),
            (b.fg, b.bg, b.underline_color, b.flags),
            "cell at {position:?}"
        );
        let uri = |snapshot: &Snapshot, cell: &Cell| {
            (cell.hyperlink as usize)
                .checked_sub(1)
                .map(|index| snapshot.hyperlinks[index].uri.clone())
        };
        assert_eq!(uri(left, a), uri(right, b), "hyperlink at {position:?}");
    }
    assert_eq!(left.cursor, right.cursor);
}

#[test]
fn incremental_snapshots_match_full_snapshots() {
    let mut engine = headless();
    let mut reference = headless();
    let mut snapshot = Snapshot::default();
    engine.update_snapshot(&mut snapshot);
    assert!(snapshot.full_damage);

    let chunks: [&[u8]; 6] = [
        b"plain \x1b[1;31mbold red\x1b[0m e\xcc\x81\r\n",
        b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07 \x1b[4:3;58:2::1:2:3mcurly\x1b[0m\r\n",
        b"\x1b[5;10Hmoved\x1b[2;1H\x1b[2Kreplaced",
        b"\x1b[7minverse\x1b[0m wide \xe4\xb8\xad",
        b"\x1b[10;1H\x1b[1K\x1b[3;3H",
        b"\x1b]8;;https://example.org\x07other\x1b]8;;\x07",
    ];
    for chunk in chunks {
        engine.feed(chunk);
        reference.feed(chunk);
        engine.update_snapshot(&mut snapshot);
        assert!(!snapshot.full_damage);
        assert!(snapshot.damage.len() < snapshot.rows);
        assert_same_cells(&snapshot, &reference.snapshot());
    }
}

#[test]
fn snapshot_damage_includes_cursor_rows() {
    let mut engine = headless();
    let mut snapshot = Snapshot::default();
    engine.feed(b"\x1b[5;1Hx");
    engine.update_snapshot(&mut snapshot);
    engine.feed(b"\x1b[10;1H");
    engine.update_snapshot(&mut snapshot);
    let rows: Vec<_> = snapshot.damage.iter().map(|damage| damage.row).collect();
    assert!(rows.contains(&4) && rows.contains(&9), "{rows:?}");
    assert_eq!(snapshot.cursor.row, 9);

    // A snapshot that missed updates is fully damaged.
    let mut stale = Snapshot::default();
    engine.update_snapshot(&mut stale);
    engine.feed(b"y");
    engine.update_snapshot(&mut snapshot);
    assert!(snapshot.full_damage);
    assert_same_cells(&snapshot, &engine.snapshot());
}

#[test]
fn cursor_text_color_contrasts_with_cursor() {
    let mut engine = headless();
    engine.feed(b"a\x1b[D");
    let cursor = engine.snapshot().cursor;
    assert_eq!(cursor.color, 0xd8d8d8);
    assert_eq!(cursor.text_color, 0x181818);

    // Fall back to the foreground when the background matches the cursor.
    engine.feed(b"\x1b[38;2;1;2;3;48;2;216;216;216mb\x1b[D");
    let cursor = engine.snapshot().cursor;
    assert_eq!(cursor.text_color, 0x010203);
}

#[test]
fn title_and_working_directory_changes() {
    let mut engine = headless();
    assert_eq!(engine.title(), None);
    engine.feed(b"\x1b]0;shell\x07");
    assert_eq!(engine.title().as_deref(), Some("shell"));

    let changed = |engine: &mut Engine| {
        events(engine).iter().filter(|e| matches!(e, HostEvent::WorkingDirectoryChanged)).count()
    };
    engine.feed(b"\x1b]7;file://host/tmp\x07");
    assert_eq!(changed(&mut engine), 1);
    engine.feed(b"\x1b]7;file://host/tmp\x07");
    assert_eq!(changed(&mut engine), 0);
    engine.feed(b"\x1b]7;file://host/var\x07");
    assert_eq!(changed(&mut engine), 1);
    assert_eq!(engine.working_directory(), Some(("host".into(), "/var".into())));
}

#[test]
fn writer_sends_from_other_threads() {
    let mut engine = headless();
    let writer = engine.writer();
    std::thread::spawn(move || assert!(writer.write(b"from thread".to_vec()))).join().unwrap();
    assert_eq!(output(&mut engine), b"from thread");

    let writer = engine.writer();
    drop(engine);
    assert!(!writer.write(b"gone".to_vec()));
}

#[test]
fn click_selection_and_copy() {
    let mut engine = headless();
    let clipboard = TestClipboard::default();
    engine.set_clipboard(Some(Box::new(clipboard.clone())));
    engine.feed(b"alpha beta-gamma\r\nsecond line");

    assert_eq!(engine.cell_at(8. * 7. + 1., 1.), (7, 0, Side::Left));
    assert_eq!(engine.cell_at(8. * 7. + 7., 17.), (7, 1, Side::Right));
    assert_eq!(engine.cell_at(1e6, 1e6), (79, 23, Side::Right));

    engine.select_click(7, 0, Side::Left, 2, false);
    assert_eq!(engine.selection_text().as_deref(), Some("beta-gamma"));
    engine.select_click(2, 1, Side::Left, 3, false);
    assert_eq!(engine.selection_text().as_deref(), Some("second line\n"));
    engine.select_click(0, 0, Side::Left, 1, true);
    engine.select_update(1, 1, Side::Right);
    assert_eq!(engine.selection_text().as_deref(), Some("al\nse"));

    assert!(engine.copy_selection(TerminalClipboardLocation::Clipboard));
    assert_eq!(clipboard.0.lock()[0].data, b"al\nse");

    engine.select_all();
    let text = engine.selection_text().unwrap();
    assert_eq!(text.trim_end(), "alpha beta-gamma\nsecond line");
    engine.select_clear();
    assert!(!engine.copy_selection(TerminalClipboardLocation::Clipboard));
}

#[test]
fn links_are_detected() {
    let mut engine = headless();
    engine.feed(b"see https://example.com/a(b) and \x1b]8;;https://osc8.dev\x07here\x1b]8;;\x07");

    let link = engine.link_at(10, 0).unwrap();
    assert_eq!(link.uri, "https://example.com/a(b)");
    assert!(!link.hyperlink);
    assert_eq!((link.range.start().column.0, link.range.end().column.0), (4, 27));
    let link = engine.link_at(34, 0).unwrap();
    assert_eq!((link.uri.as_str(), link.hyperlink), ("https://osc8.dev", true));
    assert!(engine.link_at(1, 0).is_none());

    let uris: Vec<_> = engine.visible_links().into_iter().map(|link| link.uri).collect();
    assert_eq!(uris, ["https://example.com/a(b)", "https://osc8.dev"]);

    engine.set_url_pattern(Some("")).unwrap();
    assert!(engine.link_at(10, 0).is_none());
    assert_eq!(engine.visible_links().len(), 1);
    engine.set_url_pattern(Some("ex[a-z]+")).unwrap();
    assert_eq!(engine.link_at(14, 0).unwrap().uri, "example");
    assert!(engine.set_url_pattern(Some("(")).is_err());
    engine.set_url_pattern(None).unwrap();
    assert_eq!(engine.link_at(10, 0).unwrap().uri, "https://example.com/a(b)");
}

fn press(engine: &mut Engine, c: char) -> bool {
    engine.key(&KeyEvent::new(Key::Character(c), Modifiers::empty()))
}

#[test]
fn vi_mode_moves_selects_and_copies() {
    let mut engine = headless();
    let clipboard = TestClipboard::default();
    engine.set_clipboard(Some(Box::new(clipboard.clone())));
    engine.feed(b"first word\r\nsecond words here\r\n$ ");

    engine.vi_toggle();
    assert!(engine.mode().contains(TermMode::VI));
    let mut snapshot = Snapshot::default();
    engine.update_snapshot(&mut snapshot);

    // Keys move the vi cursor and are never sent to the child.
    assert!(press(&mut engine, 'k'));
    assert!(press(&mut engine, '0'));
    engine.update_snapshot(&mut snapshot);
    assert_eq!((snapshot.cursor.column, snapshot.cursor.row), (0, 1));
    assert!(snapshot.damage.iter().any(|damage| damage.row == 1));
    assert!(press(&mut engine, 'w'));
    assert!(press(&mut engine, 'v'));
    assert!(press(&mut engine, 'e'));
    assert_eq!(engine.selection_text().as_deref(), Some("words"));
    assert!(press(&mut engine, 'y'));
    assert_eq!(clipboard.0.lock()[0].data, b"words");
    assert_eq!(engine.selection_text(), None);

    assert!(press(&mut engine, 'V'));
    assert_eq!(engine.selection_text().as_deref(), Some("second words here\n"));
    engine.key(&KeyEvent::new(Key::Named(NamedKey::Escape), Modifiers::empty()));
    assert_eq!(engine.selection_text(), None);

    // Search UI is left to the host and moves the vi cursor.
    assert!(!press(&mut engine, '/'));
    engine.search("first", Direction::Left).unwrap().unwrap();
    engine.update_snapshot(&mut snapshot);
    assert_eq!((snapshot.cursor.column, snapshot.cursor.row), (0, 0));
    assert!(press(&mut engine, 'n'));

    assert!(output(&mut engine).is_empty());
    assert!(press(&mut engine, 'i'));
    assert!(!engine.mode().contains(TermMode::VI));
    assert!(press(&mut engine, 'x'));
    assert_eq!(output(&mut engine), b"x");
}

#[test]
fn vi_mode_scrolls_history() {
    let mut engine = headless();
    for i in 0..100 {
        engine.feed(format!("line {i}\r\n").as_bytes());
    }
    engine.vi_toggle();
    engine.key(&KeyEvent::new(Key::Character('u'), Modifiers::CONTROL));
    assert_eq!(engine.snapshot().display_offset, 12);
    assert!(press(&mut engine, 'g'));
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.display_offset, engine.history_size());
    assert_eq!((snapshot.cursor.column, snapshot.cursor.row), (0, 0));
    assert!(press(&mut engine, 'G'));
    assert_eq!(engine.snapshot().display_offset, 0);
    engine.vi_motion(ViMotion::Up);
    engine.vi_select(SelectionType::Lines);
    assert_eq!(engine.selection_text().as_deref(), Some("line 99\n"));
}

#[test]
fn composed_alt_text_is_sent_without_escape() {
    let mut engine = headless();
    let mut event = KeyEvent::new(Key::Character('f'), Modifiers::ALT);
    event.text = Some("ƒ");
    event.alt_composes = true;
    engine.key(&event);
    assert_eq!(output(&mut engine), "ƒ".as_bytes());
    event.text = Some("f");
    event.alt_composes = false;
    engine.key(&event);
    assert_eq!(output(&mut engine), b"\x1bf");
}
