use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::protocols::progress::ProgressState;
use alacritty_terminal::protocols::shell::Phase;
use alacritty_terminal::term::Config;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::vte::ansi::Processor;

fn terminal(columns: usize, lines: usize) -> (Term<VoidListener>, Processor) {
    (Term::new(Config::default(), &TermSize::new(columns, lines), VoidListener), Processor::new())
}

#[test]
fn shell_markers_capture_output_and_exit_status_without_prompts() {
    let (mut term, mut parser) = terminal(40, 4);
    parser.advance(&mut term, b"\x1b]133;A\x07$ \x1b]133;B\x07printf hello\r\n\x1b]133;C\x07hello\r\n\x1b]133;D;7\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    let command = term.shell_state().last_command().unwrap();
    assert_eq!(command.text, "printf hello");
    assert_eq!(command.exit_status, Some(7));
    assert_eq!(term.last_command_output().as_deref(), Some("hello"));
    assert_eq!(term.shell_state().phase(), Phase::Input);
}

#[test]
fn output_markers_follow_reflow_and_scrollback() {
    let (mut term, mut parser) = terminal(12, 3);
    parser.advance(&mut term, b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\echo\r\n\x1b]133;C\x1b\\abcdefghijklmnopqrstuvwx\r\nlast\r\n\x1b]133;D;0\x1b\\\x1b]133;A\x1b\\$ ");
    let expected = "abcdefghijklmnopqrstuvwx\nlast";
    assert_eq!(term.last_command_output().as_deref(), Some(expected));
    term.resize(TermSize::new(7, 3));
    assert_eq!(term.last_command_output().as_deref(), Some(expected));
    term.resize(TermSize::new(24, 6));
    assert_eq!(term.last_command_output().as_deref(), Some(expected));
}

#[test]
fn output_ending_at_the_right_margin_includes_the_last_cell() {
    let (mut term, mut parser) = terminal(8, 4);
    parser.advance(
        &mut term,
        b"\x1b]133;A\x07$ \x1b]133;B\x07echo\r\n\x1b]133;C\x07abcdefgh\x1b]133;D;0\x07",
    );
    assert_eq!(term.last_command_output().as_deref(), Some("abcdefgh"));
    term.resize(TermSize::new(4, 4));
    assert_eq!(term.last_command_output().as_deref(), Some("abcdefgh"));
}

#[test]
fn aborted_input_and_secondary_prompts_do_not_finish_a_command() {
    let (mut term, mut parser) = terminal(40, 4);
    parser.advance(
        &mut term,
        b"\x1b]133;A\x07$ \x1b]133;B\x07for\r\n\x1b]133;A;k=s\x07> \x1b]133;D\x07",
    );
    assert!(term.shell_state().last_command().is_none());
    assert_eq!(term.shell_state().phase(), Phase::Unknown);
    assert!(term.last_command_output().is_none());
}

#[test]
fn prompt_navigation_moves_both_directions_through_history() {
    let (mut term, mut parser) = terminal(20, 2);
    for _ in 0..8 {
        parser.advance(
            &mut term,
            b"\x1b]133;A\x07$ \x1b]133;B\x07echo\r\n\x1b]133;C\x07output\r\n\x1b]133;D;0\x07",
        );
    }
    parser.advance(&mut term, b"\x1b]133;A\x07$ ");
    term.jump_to_prompt(Direction::Left);
    let first = term.grid().display_offset();
    assert!(first > 0);
    term.jump_to_prompt(Direction::Left);
    assert!(term.grid().display_offset() > first);
    term.jump_to_prompt(Direction::Right);
    assert_eq!(term.grid().display_offset(), first);
    term.jump_to_prompt(Direction::Right);
    assert_eq!(term.grid().display_offset(), 0);
}

#[test]
fn alternate_screen_does_not_overwrite_primary_shell_state() {
    let (mut term, mut parser) = terminal(20, 3);
    parser.advance(&mut term, b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07\x1b[?1049h\x1b]133;A\x07fake\x1b]133;D;9\x07");
    assert_eq!(term.shell_state().phase(), Phase::Output);
    assert!(term.last_command_output().is_none());
    parser.advance(&mut term, b"\x1b[?1049lreal\r\n\x1b]133;D;0\x07");
    assert_eq!(term.last_command_output().as_deref(), Some("real"));
}

#[test]
fn clearing_history_and_reset_do_not_leave_stale_output_ranges() {
    let (mut term, mut parser) = terminal(20, 2);
    parser.advance(&mut term, b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07one\r\ntwo\r\nthree\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    parser.advance(&mut term, b"\x1b[3J");
    assert!(term.last_command_output_range().is_none());
    parser.advance(&mut term, b"\x1bc");
    assert!(term.shell_state().last_command().is_none());
    term.jump_to_prompt(Direction::Left);
    assert_eq!(term.grid().display_offset(), 0);
}

#[test]
fn working_directory_decodes_file_uris_but_not_remote_local_paths() {
    let (mut term, mut parser) = terminal(20, 2);
    parser.advance(&mut term, b"\x1b]7;file://workstation/tmp/hello%20world/%C3%A6;a\x1b\\");
    let directory = term.working_directory().unwrap();
    assert_eq!(directory.path, "/tmp/hello world/æ;a");
    assert!(directory.local_path("workstation").is_some());
    assert!(directory.local_path("another-host").is_none());
    let original = directory.clone();
    for uri in [
        "http://host/tmp",
        "file://host",
        "file:///tmp/%00",
        "file:///tmp/%zz",
        "file:///tmp/%ff",
        "file://user@host/tmp",
        "file:///tmp/#fragment",
    ] {
        parser.advance(&mut term, format!("\x1b]7;{uri}\x07").as_bytes());
        assert_eq!(term.working_directory(), Some(&original));
    }
}

#[test]
fn progress_states_preserve_values_and_coexist_with_program_status() {
    let (mut term, mut parser) = terminal(20, 2);
    parser.advance(&mut term, b"\x1b]7501;state=working:app=build\x07\x1b]9;4;1;42\x07");
    assert_eq!(term.progress().state, ProgressState::Normal);
    assert_eq!(term.progress().percent, 42);
    parser.advance(&mut term, b"\x1b]9;4;2\x07");
    assert_eq!(term.progress().state, ProgressState::Error);
    assert_eq!(term.progress().percent, 42);
    parser.advance(&mut term, b"\x1b]9;4;4\x07");
    assert_eq!(term.progress().state, ProgressState::Paused);
    parser.advance(&mut term, b"\x1b]9;4;3\x07");
    assert_eq!(term.progress().state, ProgressState::Indeterminate);
    assert_eq!(term.program_status().records().len(), 1);
    parser.advance(&mut term, b"\x1b]9;4;1;999\x07");
    assert_eq!(term.progress().percent, 100);
    parser.advance(&mut term, b"\x1b]9;4;0\x07");
    assert_eq!(term.progress().state, ProgressState::Hidden);
    assert_eq!(term.program_status().records().len(), 1);
}

#[test]
fn cancelled_truncated_and_unfinished_packets_cannot_commit_state() {
    let (mut term, mut parser) = terminal(20, 2);
    for prefix in ["7;file:///tmp/", "9;4;1;", "133;A;"] {
        parser.advance(&mut term, format!("\x1b]{prefix}\x18").as_bytes());
        parser.advance(&mut term, format!("\x1b]{prefix}{}\x07", "x".repeat(70_000)).as_bytes());
        parser.advance(&mut term, format!("\x1b]{prefix}\x1bX").as_bytes());
    }
    assert!(term.working_directory().is_none());
    assert_eq!(term.progress().state, ProgressState::Hidden);
    assert_eq!(term.shell_state().phase(), Phase::Unknown);
    parser.advance(&mut term, b"\x1b]7;file:///tmp\x1b");
    assert!(term.working_directory().is_none());
    parser.advance(&mut term, b"\\");
    assert_eq!(term.working_directory().unwrap().path, "/tmp");
}

#[test]
fn prompt_exit_and_full_reset_clear_progress() {
    let (mut term, mut parser) = terminal(20, 2);
    for clear in [b"\x1b]133;A\x07".as_slice(), b"\x1bc"] {
        parser.advance(&mut term, b"\x1b]9;4;1;12\x07");
        parser.advance(&mut term, clear);
        assert_eq!(term.progress().state, ProgressState::Hidden);
    }
    parser.advance(&mut term, b"\x1b]9;4;3\x07");
    term.exit();
    assert_eq!(term.progress().state, ProgressState::Hidden);
    term.scroll_display(Scroll::Bottom);
    assert!(term.screen_lines() > 0);
}
