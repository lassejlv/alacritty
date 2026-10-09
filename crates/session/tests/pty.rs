#![cfg(unix)]

use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use alacritty_session::Session;
use alacritty_session::pty::{Options, Shell};
use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::Config;
use alacritty_terminal::term::test::TermSize;

#[derive(Clone)]
struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

#[test]
fn pty_output_reaches_terminal_before_exit() {
    let (tx, rx) = mpsc::channel();
    let listener = Listener(tx);
    let term = Term::new(Config::default(), &TermSize::new(80, 24), listener.clone());
    let options = Options {
        shell: Some(Shell::new("/bin/sh".into(), vec![
            "-c".into(),
            "printf 'session-ready'".into(),
        ])),
        drain_on_exit: true,
        ..Options::default()
    };
    let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
    let session = Session::new(term, &options, size, 0, listener, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        if matches!(event, Event::Exit) {
            break;
        }
    }
    let mut term = session.terminal.lock();
    let mut selection =
        Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
    selection.update(Point::new(Line(0), Column(12)), Side::Right);
    term.selection = Some(selection);
    assert_eq!(term.selection_to_string().as_deref(), Some("session-ready"));
}

#[test]
fn resize_and_input_are_delivered_in_order() {
    let (tx, rx) = mpsc::channel();
    let listener = Listener(tx);
    let term = Term::new(Config::default(), &TermSize::new(80, 24), listener.clone());
    let options = Options {
        shell: Some(Shell::new("/bin/sh".into(), vec!["-c".into(), "read line; stty size".into()])),
        drain_on_exit: true,
        ..Options::default()
    };
    let size = WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 };
    let mut session = Session::new(term, &options, size, 0, listener, false).unwrap();
    session.notifier.on_resize(WindowSize { num_lines: 32, num_cols: 100, ..size });
    session.notifier.notify(b"go\n".to_vec());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        if matches!(event, Event::Exit) {
            break;
        }
    }
    let mut term = session.terminal.lock();
    let mut selection =
        Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
    selection.update(Point::new(Line(23), Column(79)), Side::Right);
    term.selection = Some(selection);
    assert!(term.selection_to_string().unwrap().contains("32 100"));
}

#[test]
fn closing_a_sibling_keeps_resize_signals_working() {
    let (tx, rx) = mpsc::channel();
    let listener = Listener(tx);
    let term = Term::new(Config::default(), &TermSize::new(40, 24), listener.clone());
    let options = Options {
        shell: Some(Shell::new("/bin/sh".into(), vec![
            "-c".into(),
            r"trap 'stty size' WINCH; printf '\033[?1049hready\r\n'; while :; do read line || :; done".into(),
        ])),
        ..Options::default()
    };
    let size = WindowSize { num_lines: 24, num_cols: 40, cell_width: 8, cell_height: 16 };
    let mut survivor = Session::new(term, &options, size, 0, listener, false).unwrap();
    let wait_for = |expected: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut term = survivor.terminal.lock();
            let mut selection =
                Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
            selection.update(Point::new(Line(23), Column(39)), Side::Right);
            term.selection = Some(selection);
            if term.selection_to_string().unwrap().contains(expected) {
                break;
            }
            drop(term);
            rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        }
    };
    wait_for("ready");
    let (tx, _) = mpsc::channel();
    let listener = Listener(tx);
    let term = Term::new(Config::default(), &TermSize::new(40, 24), listener.clone());
    let sibling = Session::new(term, &options, size, 0, listener, false).unwrap();
    drop(sibling);
    survivor.notifier.on_resize(WindowSize { num_cols: 80, ..size });
    wait_for("24 80");
}
