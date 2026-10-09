//! End-to-end parser/grid tests. These exercise the VTE APC hook used by the PTY loop.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<String>>>);
impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(reply) = event {
            self.0.lock().unwrap().push(reply);
        }
    }
}

struct Size(usize, usize);
impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.0
    }

    fn screen_lines(&self) -> usize {
        self.1
    }

    fn total_lines(&self) -> usize {
        self.1
    }
}

fn terminal(history: usize) -> (Term<Replies>, Processor, Replies) {
    let replies = Replies::default();
    let config = Config { scrolling_history: history, ..Config::default() };
    let mut term = Term::new(config, &Size(10, 4), replies.clone());
    term.set_graphics_cell_size(10., 20.);
    (term, Processor::new(), replies)
}
fn image(id: u32, extra: &str) -> Vec<u8> {
    format!("\x1b_Ga=T,i={id},f=32,s=1,v=1,C=1{extra};/wAA/w==\x1b\\").into_bytes()
}

#[test]
fn query_reply_precedes_device_attributes_and_does_not_store_an_image() {
    let (mut term, mut parser, replies) = terminal(10);
    parser.advance(&mut term, b"\x1b_Ga=q,i=31,f=24,s=1,v=1;AAAA\x1b\\\x1b[c");
    let replies = replies.0.lock().unwrap();
    assert_eq!(replies[0], "\x1b_Gi=31;OK\x1b\\");
    assert_eq!(replies[1], "\x1b[?62;22c");
    assert!(term.graphics_placements().is_empty());
}

#[test]
fn apc_survives_every_read_boundary_and_does_not_leak_payload_as_text() {
    let bytes = [b"a".as_slice(), &image(7, ",q=2"), b"b"].concat();
    for split in 0..=bytes.len() {
        let (mut term, mut parser, replies) = terminal(10);
        parser.advance(&mut term, &bytes[..split]);
        parser.advance(&mut term, &bytes[split..]);
        assert!(replies.0.lock().unwrap().is_empty());
        assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, 'a');
        assert_eq!(term.grid()[Point::new(Line(0), Column(1))].c, 'b');
        let images = term.graphics_placements();
        assert_eq!(images.len(), 1);
        assert_eq!((images[0].col, images[0].viewport_row), (1, 0));
    }
}

#[test]
fn synchronized_output_commits_graphics_and_text_together_in_order() {
    let (mut term, mut parser, replies) = terminal(10);
    parser.advance(&mut term, b"\x1b[?2026hTEXT");
    parser.advance(&mut term, &image(1, ""));
    assert!(term.graphics_placements().is_empty());
    assert!(replies.0.lock().unwrap().is_empty());
    assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, ' ');
    parser.advance(&mut term, b"\x1b[?2026l");
    assert_eq!(term.graphics_placements()[0].col, 4);
    assert!(replies.0.lock().unwrap()[0].contains("i=1;OK"));
    assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, 'T');
}

#[test]
fn c1_apc_and_utf8_continuations_are_distinct() {
    let (mut term, mut parser, _) = terminal(10);
    for byte in "🟢Ghello".bytes() {
        parser.advance(&mut term, &[byte]);
    }
    assert!(term.graphics_placements().is_empty());
    parser.advance(&mut term, b"\x9fGa=T,i=1,f=32,s=1,v=1,C=1;/wAA/w==\x9c");
    assert_eq!(term.graphics_placements().len(), 1);
}

#[test]
fn cancelled_apc_and_non_graphics_apc_do_not_create_images() {
    let (mut term, mut parser, _) = terminal(10);
    parser.advance(&mut term, b"\x1b_Ga=T,i=1,f=32,s=1,v=1;/wAA/w==\x18a\x1b_other\x1b\\b");
    assert!(term.graphics_placements().is_empty());
    assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, 'a');
    assert_eq!(term.grid()[Point::new(Line(0), Column(1))].c, 'b');
}

#[test]
fn chunks_use_final_cursor_and_alternate_screen_without_partial_display() {
    let (mut term, mut parser, _) = terminal(10);
    parser.advance(&mut term, b"\x1b_Ga=T,i=1,f=32,s=1,v=1,C=1,m=1;/wAA\x1b\\");
    assert!(term.graphics_placements().is_empty());
    parser.advance(&mut term, b"\x1b[?1049h\x1b[2;3H\x1b_Gm=0;/w==\x1b\\");
    let images = term.graphics_placements();
    assert_eq!((images[0].col, images[0].viewport_row), (2, 1));
    parser.advance(&mut term, b"\x1b[?1049l");
    assert!(term.graphics_placements().is_empty());
}

#[test]
fn primary_scrollback_images_and_alternate_images_are_isolated() {
    let (mut term, mut parser, _) = terminal(8);
    parser.advance(&mut term, &image(1, ""));
    parser.advance(&mut term, b"\x1b[4;1H\n");
    assert_eq!(term.history_size(), 1);
    assert!(term.graphics_placements().is_empty());
    term.scroll_display(Scroll::Top);
    assert_eq!(term.graphics_placements()[0].viewport_row, 0);
    term.scroll_display(Scroll::Bottom);
    parser.advance(&mut term, b"\x1b[?1049h");
    parser.advance(&mut term, &image(2, ""));
    assert_eq!(term.graphics_placements()[0].image_id, 2);
    parser.advance(&mut term, b"\x1b[?1049l\x1b[3J");
    term.scroll_display(Scroll::Top);
    assert!(term.graphics_placements().is_empty());
    parser.advance(&mut term, b"\x1b[?1049h");
    assert!(term.graphics_placements().is_empty());
}

#[test]
fn full_history_and_scrolling_margins_move_and_clip_images() {
    for history in [0, 1] {
        let (mut term, mut parser, _) = terminal(history);
        parser.advance(&mut term, b"\x1b[2;1H");
        parser.advance(&mut term, &image(1, ",c=1,r=2"));
        parser.advance(&mut term, b"\x1b[4;1H\n\n");
        let images = term.graphics_placements();
        assert_eq!(images[0].viewport_row, -1);
        parser.advance(&mut term, b"\n");
        assert!(term.graphics_placements().is_empty());
    }
    let (mut term, mut parser, _) = terminal(8);
    parser.advance(&mut term, b"\x1b[2;3r\x1b[2;1H");
    parser.advance(&mut term, &image(1, ",c=1,r=2"));
    parser.advance(&mut term, b"\x1b[3;1H\n");
    let images = term.graphics_placements();
    assert_eq!(images[0].clip_top_rows, 1);
    parser.advance(&mut term, b"\n");
    assert!(term.graphics_placements().is_empty());
}

#[test]
fn placeholders_move_with_text_and_relative_children_follow_and_delete() {
    let (mut term, mut parser, _) = terminal(8);
    parser.advance(&mut term, &image(42, ",U=1,c=2,r=1,p=1"));
    parser.advance(&mut term, "\x1b[38;5;42m\u{10eeee}\u{0305}\u{0305}\u{10eeee}".as_bytes());
    let images = term.graphics_placements();
    assert_eq!(images.len(), 2);
    assert_eq!(images[1].virtual_cell, Some((1, 0)));
    parser.advance(&mut term, &image(2, ",p=2,P=42,Q=1,H=3,V=1"));
    let child = term.graphics_placements().into_iter().find(|p| p.image_id == 2).unwrap();
    assert_eq!((child.col, child.viewport_row), (3, 1));
    parser.advance(&mut term, b"\x1b_Ga=d,d=I,i=42\x1b\\");
    assert!(term.graphics_placements().is_empty());
}

#[test]
fn terminal_panes_do_not_share_image_ids_or_deletion() {
    let (mut left, mut left_parser, _) = terminal(8);
    let (mut right, mut right_parser, _) = terminal(8);
    left_parser.advance(&mut left, &image(1, ""));
    right_parser.advance(&mut right, &image(1, ""));
    left_parser.advance(&mut left, b"\x1b_Ga=d,d=I,i=1\x1b\\");
    assert!(left.graphics_placements().is_empty());
    assert_eq!(right.graphics_placements().len(), 1);
    right_parser.advance(&mut right, b"\x1bc");
    assert!(right.graphics_placements().is_empty());
}

#[test]
fn file_errors_are_indistinguishable_and_ranges_are_exact() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("image");
    std::fs::write(&file, [255, 0, 0, 255]).unwrap();
    let (mut term, mut parser, replies) = terminal(8);
    let paths = [dir.path().join("missing"), dir.path().to_path_buf(), file.clone()];
    for path in paths {
        let payload = STANDARD.encode(path.to_str().unwrap());
        parser.advance(
            &mut term,
            format!("\x1b_Ga=q,i=1,f=32,s=1,v=1,t=f,S=5;{payload}\x1b\\").as_bytes(),
        );
    }
    let errors = replies.0.lock().unwrap().clone();
    assert_eq!(errors.len(), 3);
    assert!(errors.iter().all(|reply| reply == "\x1b_Gi=1;EBADF:Failed to read image file\x1b\\"));
    let payload = STANDARD.encode(file.to_str().unwrap());
    parser.advance(
        &mut term,
        format!("\x1b_Ga=q,i=1,f=32,s=1,v=1,t=f,S=4;{payload}\x1b\\").as_bytes(),
    );
    assert_eq!(replies.0.lock().unwrap().last().unwrap(), "\x1b_Gi=1;OK\x1b\\");
}

#[test]
fn official_client_unpadded_payloads_and_image_number_animation_work() {
    let (mut term, mut parser, replies) = terminal(8);
    parser.advance(&mut term, b"\x1b_Ga=T,I=9,f=32,s=1,v=1,C=1;/wAA/w\x1b\\");
    parser.advance(&mut term, b"\x1b_Ga=f,I=9,f=32,s=1,v=1,z=100;AAD//w\x1b\\");
    parser.advance(&mut term, b"\x1b_Ga=a,I=9,c=2\x1b\\");
    let images = term.graphics_placements();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].image.rgba(), Some([0, 0, 255, 255].as_slice()));
    assert!(replies.0.lock().unwrap().iter().all(|reply| reply.contains(";OK")));
}

#[test]
fn successful_animation_controls_and_deletes_do_not_leak_replies_into_shells() {
    let (mut term, mut parser, replies) = terminal(8);
    parser.advance(&mut term, &image(1, ",q=2"));
    parser.advance(&mut term, b"\x1b_Ga=a,i=1,r=1,z=30\x1b\\\x1b_Ga=d,d=I,i=1\x1b\\");
    assert!(replies.0.lock().unwrap().is_empty());
    parser.advance(&mut term, b"\x1b_Ga=a,i=1,c=2\x1b\\");
    assert!(replies.0.lock().unwrap()[0].contains(";ENOENT"));
}

#[test]
fn anonymous_images_are_unaddressable_and_survive_colliding_explicit_ids() {
    let (mut term, mut parser, replies) = terminal(8);
    parser.advance(&mut term, b"\x1b_Ga=T,f=32,s=1,v=1,p=9,C=1;/wAA/w==\x1b\\");
    let anonymous = term.graphics_placements();
    assert_eq!((anonymous[0].image_id, anonymous[0].placement_id), (0, 0));
    parser.advance(&mut term, b"\x1b_Ga=p,i=4294967295\x1b\\");
    assert!(replies.0.lock().unwrap().last().unwrap().contains(";ENOENT"));
    parser.advance(&mut term, &image(u32::MAX, ""));
    assert_eq!(term.graphics_placements().len(), 2);
    parser.advance(&mut term, b"\x1b_Ga=d,d=R,x=1,y=4294967295\x1b\\");
    let remaining = term.graphics_placements();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].image_id, 0);
}
