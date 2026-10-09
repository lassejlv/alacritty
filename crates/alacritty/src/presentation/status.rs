//! Window-title presentation for terminal program status records.
use alacritty_terminal::program_status::{ProgramStatuses, State};

pub fn window_title(statuses: &ProgramStatuses, title: Option<&str>) -> Option<String> {
    // Prefer records needing attention, then active work, with newest breaking ties.
    let record = statuses.records().iter().max_by_key(|record| match record.state {
        State::Blocked | State::Error => 3,
        State::Done => 2,
        State::Working => 1,
        _ => 0,
    })?;
    let state = match record.state {
        State::Idle => "idle",
        State::Working => "working",
        State::Done => "done",
        State::Blocked => "blocked",
        State::Error => "error",
        State::Clear => return None,
    };
    let label = record.title.as_deref().or_else(|| statuses.app_for(record)).or(title);
    let mut result = format!("[{state}");
    if let Some(progress) = record.progress {
        result.push_str(&format!(" {progress}%"));
    }
    result.push(']');
    if let Some(label) = label {
        result.push(' ');
        result.push_str(label);
    }
    if let Some(msg) = &record.msg {
        result.push_str(": ");
        result.push_str(msg);
    }
    // Status text is untrusted. Keep invisible direction/format controls out of chrome.
    Some(
        result
            .chars()
            .filter(|c| {
                !matches!(c,
                    '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
                    | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
                )
            })
            .take(256)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn terminal() -> Term<VoidListener> {
        Term::new(Config::default(), &TermSize::new(80, 24), VoidListener)
    }

    #[test]
    fn status_shows_progress_label_and_decoded_message() {
        let mut term = terminal();
        let mut parser: Processor = Default::default();
        parser.advance(&mut term, b"\x1b]7501;state=blocked:kind=permission:title=RGVwbG95:msg=QXBwcm92ZT8:progress=40\x07");
        assert_eq!(
            window_title(term.program_status(), Some("shell")).as_deref(),
            Some("[blocked 40%] Deploy: Approve?")
        );
    }

    #[test]
    fn clearing_status_restores_the_latest_shell_title() {
        let mut term = terminal();
        let mut parser: Processor = Default::default();
        parser.advance(&mut term, b"\x1b]7501;state=working\x07");
        assert_eq!(
            window_title(term.program_status(), Some("new shell")).as_deref(),
            Some("[working] new shell")
        );
        parser.advance(&mut term, b"\x1b]7501;state=clear\x07");
        assert_eq!(window_title(term.program_status(), Some("new shell")), None);
    }

    #[test]
    fn status_prioritizes_attention_and_disarms_direction_controls() {
        let mut term = terminal();
        let mut parser: Processor = Default::default();
        parser.advance(&mut term, b"\x1b]7501;state=blocked:id=first:msg=YXBwcm924oCuZQ==\x07\x1b]7501;state=working:id=second\x07");
        assert_eq!(
            window_title(term.program_status(), None).as_deref(),
            Some("[blocked]: approve")
        );
        parser.advance(&mut term, b"\x1b]7501;state=error:id=third:app=latest\x07");
        assert_eq!(window_title(term.program_status(), None).as_deref(), Some("[error] latest"));
    }
}
