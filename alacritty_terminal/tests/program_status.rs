use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::program_status::{BlockedKind, State};
use alacritty_terminal::term::Config;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::vte::ansi::Processor;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}

fn terminal() -> (Term<Listener>, Processor, Listener) {
    let listener = Listener::default();
    let term = Term::new(Config::default(), &TermSize::new(80, 24), listener.clone());
    (term, Processor::new(), listener)
}

fn report(term: &mut Term<Listener>, parser: &mut Processor, body: &str) {
    parser.advance(term, format!("\x1b]7501;{body}\x1b\\").as_bytes());
}

#[test]
fn query_replies_before_device_attributes_with_both_terminators() {
    for terminator in ["\x07", "\x1b\\"] {
        let (mut term, mut parser, listener) = terminal();
        parser.advance(&mut term, format!("\x1b]7501;?{terminator}\x1b[c").as_bytes());
        let events = listener.0.borrow();
        let replies: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Event::PtyWrite(reply) => Some(reply.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0], format!("\x1b]7501;?{terminator}"));
        assert!(term.program_status().records().is_empty());
    }
}

#[test]
fn reports_decode_and_replace_records_and_update_title() {
    let (mut term, mut parser, listener) = terminal();
    report(
        &mut term,
        &mut parser,
        "state=blocked:kind=permission:app=terraform:title=RGVwbG95:msg=QXBwcm92ZT8:progress=40",
    );
    let record = &term.program_status().records()[0];
    assert_eq!(record.state, State::Blocked);
    assert_eq!(record.kind, Some(BlockedKind::Permission));
    assert_eq!(record.progress, Some(40));
    assert_eq!(record.title.as_deref(), Some("Deploy"));
    assert_eq!(record.msg.as_deref(), Some("Approve?"));
    assert!(matches!(listener.0.borrow().last(), Some(Event::Title(title))
        if title == "[blocked 40%] Deploy: Approve?"));

    report(&mut term, &mut parser, "state=done");
    let record = &term.program_status().records()[0];
    assert_eq!(record.state, State::Done);
    assert_eq!(record.app, None);
    assert_eq!(record.msg, None);
    assert_eq!(record.title, None);
    assert_eq!(record.kind, None);
    assert_eq!(record.progress, None);
}

#[test]
fn malformed_pairs_are_skipped_and_last_value_wins() {
    let (mut term, mut parser, _) = terminal();
    report(
        &mut term,
        &mut parser,
        " state = idle \
         :junk:=empty:Bad=key:x=a;b:other=!:state=working:progress=2:progress=99:unknown=yes",
    );
    let record = &term.program_status().records()[0];
    assert_eq!(record.state, State::Working);
    assert_eq!(record.progress, Some(99));
    report(&mut term, &mut parser, "state=blocked:kind=unknown:progress=101:app=bad/name");
    let record = &term.program_status().records()[0];
    assert_eq!(record.kind, None);
    assert_eq!(record.progress, None);
    assert_eq!(record.app, None);
}

#[test]
fn invalid_reports_leave_existing_records_untouched() {
    let (mut term, mut parser, listener) = terminal();
    report(&mut term, &mut parser, "state=working:app=original");
    let original = term.program_status().records().to_vec();
    listener.0.borrow_mut().clear();
    for body in [
        "app=missing-state",
        "state=unknown",
        "state=done:id=",
        "state=done:id=/child",
        "state=done:id=child/",
        "state=done:id=a//b",
        "state=done:id=bad,id",
        "state=done:msg=A",
        "state=done:title=/w==",
        "state=clear:msg=AA==",
        "state=done:msg=woc=",
        "state=done:msg=fw==",
        "state=done:msg=Cg==:msg=T0s=",
        "state=done:abcdefghijklmnopq=x",
    ] {
        report(&mut term, &mut parser, body);
        assert_eq!(term.program_status().records(), original, "{body}");
    }
    assert!(listener.0.borrow().is_empty());
}

#[test]
fn limits_apply_before_any_record_changes() {
    let (mut term, mut parser, _) = terminal();
    report(&mut term, &mut parser, "state=working");
    let original = term.program_status().records().to_vec();
    for body in [
        format!("state=done:app={}", "a".repeat(33)),
        format!("state=done:id={}", "a".repeat(33)),
        format!("state=done:id={}", ["a"; 9].join("/")),
        format!("state=done:id={}", vec!["a".repeat(32); 4].join("/") + "/x"),
        format!("state=done:msg={}", STANDARD.encode("x".repeat(2049))),
        format!("state=done:title={}", STANDARD.encode("x".repeat(193))),
        format!("state=done:msg={}:msg=T0s=", "a".repeat(2733)),
        format!("state=done:unknown={}", "a".repeat(4096)),
    ] {
        report(&mut term, &mut parser, &body);
        assert_eq!(term.program_status().records(), original);
    }
    let msg = "x".repeat(2048);
    let title = "y".repeat(192);
    report(
        &mut term,
        &mut parser,
        &format!("state=done:msg={}:title={}", STANDARD.encode(&msg), STANDARD.encode(&title)),
    );
    let record = &term.program_status().records()[0];
    assert_eq!(record.msg.as_deref(), Some(msg.as_str()));
    assert_eq!(record.title.as_deref(), Some(title.as_str()));
}

#[test]
fn sequence_size_includes_the_terminator() {
    for terminator in ["\x07", "\x1b\\"] {
        let (mut term, mut parser, _) = terminal();
        let prefix = "\x1b]7501;state=working:x=";
        let packet =
            format!("{prefix}{}{terminator}", "a".repeat(4096 - prefix.len() - terminator.len()));
        parser.advance(&mut term, packet.as_bytes());
        assert_eq!(term.program_status().records().len(), 1);
        report(&mut term, &mut parser, "state=clear");
        let packet =
            format!("{prefix}{}{terminator}", "a".repeat(4097 - prefix.len() - terminator.len()));
        parser.advance(&mut term, packet.as_bytes());
        assert!(term.program_status().records().is_empty());
    }
}

#[test]
fn hierarchy_inherits_apps_and_clear_removes_only_its_subtree() {
    let (mut term, mut parser, _) = terminal();
    for body in [
        "state=idle:app=root",
        "state=working:id=build:app=cargo",
        "state=blocked:id=build/test/one",
        "state=working:id=builder",
    ] {
        report(&mut term, &mut parser, body);
    }
    let statuses = term.program_status();
    assert_eq!(statuses.app_for(&statuses.records()[2]), Some("cargo"));
    assert_eq!(statuses.app_for(&statuses.records()[3]), Some("root"));
    report(&mut term, &mut parser, "state=clear:id=build");
    let ids: Vec<_> = term.program_status().records().iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["", "builder"]);
    report(&mut term, &mut parser, "state=clear");
    assert!(term.program_status().records().is_empty());
}

#[test]
fn records_survive_screens_and_soft_reset_but_not_full_reset() {
    let (mut term, mut parser, listener) = terminal();
    report(&mut term, &mut parser, "state=working");
    parser.advance(&mut term, b"\x1b[?1049h\x1b[?1049l\x1b[!p");
    assert_eq!(term.program_status().records().len(), 1);
    parser.advance(&mut term, b"\x1bc");
    assert!(term.program_status().records().is_empty());
    assert!(listener.0.borrow().iter().any(|event| matches!(event, Event::ResetTitle)));
}

#[test]
fn prompt_and_exit_remove_transient_records_but_keep_results() {
    for exit in [false, true] {
        let (mut term, mut parser, _) = terminal();
        for state in ["idle", "working", "blocked", "done", "error"] {
            report(&mut term, &mut parser, &format!("state={state}:id={state}"));
        }
        if exit {
            term.exit();
        } else {
            parser.advance(&mut term, b"\x1b]133;A;extra=value\x07");
        }
        let states: Vec<_> = term.program_status().records().iter().map(|r| r.state).collect();
        assert_eq!(states, [State::Idle, State::Done, State::Error]);
    }
}

#[test]
fn record_cap_evicts_least_recently_updated() {
    let (mut term, mut parser, _) = terminal();
    for id in 0..256 {
        report(&mut term, &mut parser, &format!("state=working:id={id}"));
    }
    report(&mut term, &mut parser, "state=done:id=0");
    report(&mut term, &mut parser, "state=working:id=new");
    let records = term.program_status().records();
    assert_eq!(records.len(), 256);
    assert!(!records.iter().any(|record| record.id == "1"));
    assert!(records.iter().any(|record| record.id == "0" && record.state == State::Done));
}

#[test]
fn fragmented_and_cancelled_reports_do_not_commit_early() {
    let (mut term, mut parser, _) = terminal();
    for byte in b"\x1b]7501;state=working\x1b" {
        parser.advance(&mut term, &[*byte]);
        assert!(term.program_status().records().is_empty());
    }
    parser.advance(&mut term, b"\\");
    assert_eq!(term.program_status().records()[0].state, State::Working);
    for tail in ["\x18", "\x1a", "\x1b[31m", "\x1bX"] {
        parser.advance(&mut term, format!("\x1b]7501;state=done{tail}").as_bytes());
        assert_eq!(term.program_status().records()[0].state, State::Working);
    }
    report(&mut term, &mut parser, "state=done");
    assert_eq!(term.program_status().records()[0].state, State::Done);
}

#[test]
fn title_updates_preserve_status_and_clear_restores_latest_title() {
    let (mut term, mut parser, listener) = terminal();
    parser.advance(&mut term, b"\x1b]2;shell\x07");
    report(&mut term, &mut parser, "state=working");
    parser.advance(&mut term, b"\x1b]2;new shell\x07");
    assert!(matches!(listener.0.borrow().last(), Some(Event::Title(title))
        if title == "[working] new shell"));
    report(&mut term, &mut parser, "state=clear");
    assert!(
        matches!(listener.0.borrow().last(), Some(Event::Title(title)) if title == "new shell")
    );
}

#[test]
fn title_prioritizes_attention_and_disarms_invisible_controls() {
    let (mut term, mut parser, listener) = terminal();
    report(
        &mut term,
        &mut parser,
        &format!("state=blocked:id=first:msg={}", STANDARD.encode("approve\u{202e}")),
    );
    report(&mut term, &mut parser, "state=working:id=second");
    assert!(
        matches!(listener.0.borrow().last(), Some(Event::Title(title)) if title == "[blocked]: approve")
    );
    report(&mut term, &mut parser, "state=error:id=third:app=latest");
    assert!(
        matches!(listener.0.borrow().last(), Some(Event::Title(title)) if title == "[error] latest")
    );
}
