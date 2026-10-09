use std::time::{Duration, Instant};

use alacritty_terminal::protocols::notifications::{Effect, Notifications, Occasion};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

fn apply(host: &mut Notifications, packet: &str) -> Vec<Effect> {
    host.apply(packet.as_bytes(), false, Instant::now(), false, true, true)
}

fn shown(effects: &[Effect]) -> &alacritty_terminal::protocols::notifications::Notification {
    effects
        .iter()
        .find_map(|effect| if let Effect::Show(n) = effect { Some(n.as_ref()) } else { None })
        .expect("notification was not emitted")
}

fn replies(effects: &[Effect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(
            |effect| if let Effect::Reply(text) = effect { Some(text.as_str()) } else { None },
        )
        .collect()
}

#[test]
fn multipart_notifications_preserve_metadata_and_literal_semicolons() {
    let mut host = Notifications::default();
    assert!(apply(&mut host, "i=build:d=0:a=report,-focus:c=1:u=2;Build; complete").is_empty());
    let effects = apply(&mut host, "i=build:p=body;All tests passed");
    let notification = shown(&effects);
    assert_eq!(notification.title, "Build; complete");
    assert_eq!(notification.body, "All tests passed");
    assert!(!notification.focus);
    assert!(notification.report);
    assert!(notification.report_close);
    assert_eq!(notification.urgency, 2);
    let (focus, effects) = host.activated(notification.serial, None);
    assert!(!focus);
    assert_eq!(replies(&effects), ["\x1b]99;i=build;\x1b\\", "\x1b]99;i=build:p=close;\x1b\\"]);
}

#[test]
fn base64_accepts_splits_before_and_after_encoding_and_utf8_boundaries() {
    for split in 1..12 {
        let mut host = Notifications::default();
        let encoded = STANDARD.encode("Hello æ世界\n");
        assert!(apply(&mut host, &format!("i=one:d=0:e=1;{}", &encoded[..split])).is_empty());
        let effects = apply(&mut host, &format!("i=one:e=1;{}", &encoded[split..]));
        assert_eq!(shown(&effects).title, "Hello æ世界\n");
    }
    let mut host = Notifications::default();
    assert!(apply(&mut host, &format!("i=two:d=0:e=1;{}", STANDARD.encode("a"))).is_empty());
    let effects = apply(&mut host, &format!("i=two:e=1;{}", STANDARD.encode("b")));
    assert_eq!(shown(&effects).title, "ab");
}

#[test]
fn buttons_report_one_based_indices_only_when_requested() {
    let mut host = Notifications::default();
    apply(&mut host, "i=task:d=0:a=report;Choose");
    let effects = apply(&mut host, "i=task:p=buttons;Open\u{2028}Dismiss");
    let serial = shown(&effects).serial;
    assert!(host.activated(serial, Some(3)).1.is_empty());
    let (focus, effects) = host.activated(serial, Some(2));
    assert!(focus);
    assert_eq!(replies(&effects), ["\x1b]99;i=task;2\x1b\\"]);
    let effects = apply(&mut host, ";No callback requested");
    let (_, effects) = host.activated(shown(&effects).serial, None);
    assert!(replies(&effects).is_empty());
}

#[test]
fn updating_replaces_the_same_id_and_ignores_old_callbacks() {
    let mut host = Notifications::default();
    let old = shown(&apply(&mut host, "i=same:a=report:c=1;Old")).serial;
    let effects = apply(&mut host, "i=same:a=report;New");
    let new = shown(&effects).serial;
    assert_ne!(old, new);
    assert!(effects.iter().any(|e| matches!(e, Effect::Close(id) if *id == old)));
    assert!(host.activated(old, None).1.is_empty());
    assert!(host.closed(old).is_empty());
    assert_eq!(host.alive("query", &[old, new]), "\x1b]99;i=query:p=alive;same\x1b\\");
}

#[test]
fn anonymous_notifications_are_independent_and_use_zero_in_replies() {
    let mut host = Notifications::default();
    let a = shown(&apply(&mut host, "a=report;one")).serial;
    let b = shown(&apply(&mut host, "a=report;two")).serial;
    assert_ne!(a, b);
    assert!(apply(&mut host, "p=close;").is_empty());
    assert_eq!(replies(&host.activated(a, None).1), ["\x1b]99;i=0;\x1b\\"]);
    assert_eq!(replies(&host.activated(b, None).1), ["\x1b]99;i=0;\x1b\\"]);
}

#[test]
fn explicit_close_expiry_and_reset_remove_notification_state() {
    let mut host = Notifications::default();
    apply(&mut host, "i=one:c=1;First");
    assert_eq!(replies(&apply(&mut host, "i=one:p=close;")), ["\x1b]99;i=one:p=close;\x1b\\"]);
    let now = Instant::now();
    let effects = host.apply(b"i=timed:w=10:c=1;Soon", false, now, false, false, true);
    let serial = shown(&effects).serial;
    assert_eq!(host.next_expiry(), Some(now + Duration::from_millis(10)));
    assert!(host.expire(now).is_empty());
    assert_eq!(replies(&host.expire(now + Duration::from_millis(11))), [
        "\x1b]99;i=timed:p=close;\x1b\\"
    ]);
    assert!(host.activated(serial, None).1.is_empty());
    apply(&mut host, "i=last;Last");
    assert!(matches!(host.reset().as_slice(), [Effect::Close(_)]));
    assert!(host.next_expiry().is_none());
}

#[test]
fn focus_visibility_and_user_policy_are_enforced_at_commit() {
    let now = Instant::now();
    let mut host = Notifications::default();
    assert!(host.apply(b"o=unfocused;Quiet", false, now, true, true, true).is_empty());
    assert!(host.apply(b"o=invisible;Quiet", false, now, false, true, true).is_empty());
    let effects = host.apply(b"o=invisible;Show", false, now, false, false, true);
    assert_eq!(shown(&effects).occasion, Occasion::Invisible);
    assert!(host.apply(b";Disabled", false, now, false, false, false).is_empty());
}

#[test]
fn cancelled_or_malformed_chunks_cannot_complete_previous_content() {
    let mut host = Notifications::default();
    apply(&mut host, "i=one:d=0;Old");
    host.apply(b"i=one;Truncated", true, Instant::now(), false, true, true);
    let effects = apply(&mut host, "i=one;New");
    assert_eq!(shown(&effects).title, "New");
    for packet in
        ["i=bad,id:p=?;", "i=bad\nkey:p=?;", "e=1;%%%%", "d=2;bad", ";bad\nbody", "e=1;AA=="]
    {
        assert!(apply(&mut host, packet).is_empty(), "{packet:?}");
    }
    apply(&mut host, "i=two:d=0;Old");
    assert!(apply(&mut host, &format!("i=two;{}", "x".repeat(2049))).is_empty());
    assert_eq!(shown(&apply(&mut host, "i=two;New")).title, "New");
}

#[test]
fn capabilities_queries_and_unknown_extensions_are_forward_compatible() {
    let mut host = Notifications::default();
    let effects = apply(&mut host, "i=mux:p=?;");
    let response = replies(&effects)[0];
    assert!(response.starts_with("\x1b]99;i=mux:p=?;"));
    assert!(response.contains("a=focus,report"));
    assert!(response.contains("p=title,body,buttons,close,alive,?"));
    assert_eq!(shown(&apply(&mut host, "z=future:i=ignored;Hello")).title, "Hello");
    assert!(apply(&mut host, "p=future;ignored").is_empty());
}

#[test]
fn pending_and_active_notifications_are_bounded() {
    let mut host = Notifications::default();
    for n in 0..32 {
        apply(&mut host, &format!("i=pending{n}:d=0;part"));
    }
    assert!(apply(&mut host, "i=overflow:d=0;ignored").is_empty());
    assert_eq!(shown(&apply(&mut host, "i=pending0;end")).title, "partend");
    for n in 1..32 {
        apply(&mut host, &format!("i=pending{n};end"));
    }
    assert!(apply(&mut host, "i=overflow;ignored").is_empty());
    assert_eq!(host.reset().len(), 32);
}

#[test]
fn parser_preserves_notification_payloads_and_orders_capability_replies() {
    use alacritty_terminal::Term;
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct Listener(Rc<RefCell<Vec<Event>>>);
    impl EventListener for Listener {
        fn send_event(&self, event: Event) {
            self.0.borrow_mut().push(event);
        }
    }
    let listener = Listener::default();
    let mut term = Term::new(Config::default(), &TermSize::new(20, 2), listener.clone());
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, b"\x1b]99;i=mux:p=?;\x1b\\\x1b[c\x1b]99;i=one:d=0;old\x07\x1b]99;i=one;bad\x18\x1b]99;i=one;new;title\x1b\\");
    let mut host = Notifications::default();
    let mut replies_seen = vec![];
    let mut titles = vec![];
    for event in listener.0.take() {
        match event {
            Event::DesktopNotification { body, truncated } => {
                for effect in host.apply(&body, truncated, Instant::now(), false, true, true) {
                    match effect {
                        Effect::Reply(reply) => replies_seen.push(reply),
                        Effect::Show(notification) => titles.push(notification.title),
                        _ => (),
                    }
                }
            },
            Event::PtyWrite(reply) => replies_seen.push(reply),
            _ => (),
        }
    }
    assert_eq!(titles, ["new;title"]);
    assert!(replies_seen[0].starts_with("\x1b]99;i=mux:p=?;"));
    assert!(replies_seen[1].starts_with("\x1b[?"));
}
