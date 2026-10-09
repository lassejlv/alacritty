use std::sync::{Arc, Mutex};

use alacritty_terminal::clipboard::{
    ClipboardHost, KittyClipboardHostState, TerminalClipboardContent,
    TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
};
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Event>>>);
impl EventListener for Events {
    fn send_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

struct Size;
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        4
    }

    fn screen_lines(&self) -> usize {
        4
    }

    fn columns(&self) -> usize {
        80
    }
}

#[derive(Default)]
struct Clipboard {
    contents: Vec<TerminalClipboardContent>,
    writes: usize,
}
impl ClipboardHost for Clipboard {
    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        self.contents = request.contents;
        self.writes += 1;
        TerminalClipboardWriteResult::Success { remember_permission: false }
    }
}

fn drain(
    events: &Events,
    state: &mut KittyClipboardHostState,
    host: &mut Clipboard,
) -> Vec<Vec<u8>> {
    let mut replies = Vec::new();
    for event in events.0.lock().unwrap().drain(..) {
        match event {
            Event::KittyClipboard(osc) => replies.extend(state.handle_osc(osc, host)),
            Event::KittyClipboardMode(enabled) => state.set_paste_events_enabled(enabled),
            Event::KittyClipboardReset => state.reset(),
            Event::PtyWrite(reply) => replies.push(reply.into_bytes()),
            _ => (),
        }
    }
    replies
}

const WRITE: &[u8] = b"\x1b]5522;type=write:id=client\x1b\\\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8=\x1b\\\x1b]5522;type=wdata\x1b\\";

#[test]
fn bell_terminated_requests_preserve_ids_and_reply_terminators() {
    let events = Events::default();
    let mut term = Term::new(Config::default(), &Size, events.clone());
    let mut parser: Processor = Processor::new();
    let mut state = KittyClipboardHostState::new();
    let mut host = Clipboard::default();
    parser.advance(&mut term, b"\x1b]5522;type=read:id=a*!b;dGV4dC9wbGFpbg==\x07");
    assert_eq!(drain(&events, &mut state, &mut host), [
        b"\x1b]5522;type=read:status=EPERM:id=ab\x07".to_vec()
    ]);
}

#[test]
fn synchronized_output_keeps_clipboard_requests_ordered_with_text() {
    let events = Events::default();
    let mut term = Term::new(Config::default(), &Size, events.clone());
    let mut parser: Processor = Processor::new();
    let mut state = KittyClipboardHostState::new();
    let mut host = Clipboard::default();
    parser.advance(&mut term, b"\x1b[?2026hTEXT");
    parser.advance(&mut term, WRITE);
    assert!(drain(&events, &mut state, &mut host).is_empty());
    assert_eq!(host.writes, 0);
    parser.advance(&mut term, b"\x1b[?2026l");
    assert_eq!(drain(&events, &mut state, &mut host).len(), 1);
    assert_eq!(host.writes, 1);
    assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, 'T');
}

#[test]
fn disabling_clipboard_access_revokes_paste_mode_and_pending_transactions() {
    let events = Events::default();
    let mut term = Term::new(Config::default(), &Size, events.clone());
    let mut parser: Processor = Processor::new();
    let mut state = KittyClipboardHostState::new();
    let mut host = Clipboard::default();
    parser.advance(&mut term, b"\x1b[?5522h\x1b]5522;type=write:id=pending\x1b\\");
    drain(&events, &mut state, &mut host);
    assert!(state.paste_events_enabled());
    term.set_options(Config { osc52: Osc52::Disabled, ..Config::default() });
    drain(&events, &mut state, &mut host);
    assert!(!state.paste_events_enabled());
    parser.advance(&mut term, b"\x1b[?5522h\x1b[?5522$p\x1b]5522;type=wdata\x1b\\");
    assert_eq!(drain(&events, &mut state, &mut host), [b"\x1b[?5522;0$y".to_vec()]);
    assert_eq!(host.writes, 0);
}

#[test]
fn clipboard_write_survives_every_pty_boundary_and_commits_only_after_st() {
    for split in 0..WRITE.len() {
        let events = Events::default();
        let mut term = Term::new(Config::default(), &Size, events.clone());
        let mut parser: Processor = Processor::new();
        let mut state = KittyClipboardHostState::new();
        let mut host = Clipboard::default();
        parser.advance(&mut term, &WRITE[..split]);
        assert!(drain(&events, &mut state, &mut host).is_empty());
        assert_eq!(host.writes, 0);
        parser.advance(&mut term, &WRITE[split..]);
        assert_eq!(drain(&events, &mut state, &mut host), [
            b"\x1b]5522;type=write:status=DONE:id=client\x1b\\".to_vec()
        ]);
        assert_eq!(host.contents[0].data, b"hello");
    }
}

#[test]
fn cancelled_or_corrupt_packets_never_replace_the_clipboard() {
    for suffix in [b"\x18".as_slice(), b"\x1a", b"\x1b[0m", b"\x01\x1b\\"] {
        let events = Events::default();
        let mut term = Term::new(Config::default(), &Size, events.clone());
        let mut parser: Processor = Processor::new();
        let mut state = KittyClipboardHostState::new();
        let mut host = Clipboard::default();
        parser.advance(&mut term, &WRITE[..WRITE.len() - 2]);
        parser.advance(&mut term, suffix);
        let replies = drain(&events, &mut state, &mut host);
        assert_eq!(replies, [b"\x1b]5522;type=write:status=EINVAL:id=client\x1b\\".to_vec()]);
        assert_eq!(host.writes, 0);
        parser.advance(&mut term, b"\x1b]5522;type=wdata\x1b\\");
        assert!(drain(&events, &mut state, &mut host).is_empty());
    }
}

#[test]
fn paste_mode_queries_and_resets_are_ordered_and_pane_local() {
    let events = Events::default();
    let mut term = Term::new(Config::default(), &Size, events.clone());
    let mut other = Term::new(Config::default(), &Size, Events::default());
    let mut parser: Processor = Processor::new();
    let mut state = KittyClipboardHostState::new();
    let mut host = Clipboard::default();
    parser.advance(&mut term, b"\x1b[?5522$p\x1b[?2004;5522h\x1b[?5522$p");
    assert_eq!(drain(&events, &mut state, &mut host), [
        b"\x1b[?5522;2$y".to_vec(),
        b"\x1b[?5522;1$y".to_vec()
    ]);
    assert!(state.paste_events_enabled());
    assert!(term.mode().contains(TermMode::BRACKETED_PASTE));
    assert!(!other.mode().contains(TermMode::CLIPBOARD_PASTE_EVENTS));
    parser.advance(&mut term, b"\x1bc\x1b[?5522$p");
    assert_eq!(drain(&events, &mut state, &mut host), [b"\x1b[?5522;2$y".to_vec()]);
    assert!(!state.paste_events_enabled());
    parser.advance(&mut other, b"\x1b[?5522h");
    assert!(!term.mode().contains(TermMode::CLIPBOARD_PASTE_EVENTS));
}

#[test]
fn oversized_osc_packet_aborts_and_preserves_following_text_and_protocol() {
    let events = Events::default();
    let mut term = Term::new(Config::default(), &Size, events.clone());
    let mut parser: Processor = Processor::new();
    let mut state = KittyClipboardHostState::new();
    let mut host = Clipboard::default();
    parser.advance(&mut term, b"\x1b]5522;type=write:id=large\x1b\\");
    let mut huge = b"\x1b]5522;type=wdata:mime=dGV4dC9wbGFpbg==;".to_vec();
    huge.extend(std::iter::repeat_n(b'A', 128 * 1024));
    huge.extend_from_slice(b"\x1b\\visible");
    parser.advance(&mut term, &huge);
    assert_eq!(drain(&events, &mut state, &mut host), [
        b"\x1b]5522;type=write:status=EINVAL:id=large\x1b\\".to_vec()
    ]);
    assert_eq!(host.writes, 0);
    let text: String = (0..7).map(|col| term.grid()[Point::new(Line(0), Column(col))].c).collect();
    assert_eq!(text, "visible");
    parser.advance(&mut term, WRITE);
    assert_eq!(drain(&events, &mut state, &mut host).len(), 1);
    assert_eq!(host.writes, 1);
}
