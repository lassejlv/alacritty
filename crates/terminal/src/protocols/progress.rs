//! ConEmu OSC 9;4 progress, independent of OSC 7501 program records.

/// Idle time after which applications are expected to have refreshed visible progress.
pub const PROGRESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Whether an OSC 9 body is a ConEmu command (sub-ID 1 to 12) rather than notification text.
pub fn is_conemu_command(body: &[u8]) -> bool {
    let id_len = body.iter().take_while(|b| b.is_ascii_digit()).count();
    let id = std::str::from_utf8(&body[..id_len]).ok().and_then(|id| id.parse::<u8>().ok());
    matches!(id, Some(1..=12)) && matches!(body.get(id_len), None | Some(b';'))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressState {
    #[default]
    Hidden,
    Normal,
    Error,
    Indeterminate,
    Paused,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub state: ProgressState,
    pub percent: u8,
}

impl Progress {
    pub fn update(&mut self, body: &[u8]) -> bool {
        let Ok(body) = std::str::from_utf8(body) else { return false };
        let Some(body) = body.strip_prefix("4;") else { return false };
        let mut fields = body.split(';');
        let state = match fields.next() {
            Some("0") => ProgressState::Hidden,
            Some("1") => ProgressState::Normal,
            Some("2") => ProgressState::Error,
            Some("3") => ProgressState::Indeterminate,
            Some("4") => ProgressState::Paused,
            _ => return false,
        };
        let percent = match fields.next() {
            Some("") | None => None,
            Some(value) if value.bytes().all(|b| b.is_ascii_digit()) => {
                let Ok(value) = value.parse::<u32>() else { return false };
                Some(value.min(100) as u8)
            },
            _ => return false,
        };
        if fields.next().is_some() {
            return false;
        }
        self.percent = match state {
            ProgressState::Hidden => 0,
            ProgressState::Normal => percent.unwrap_or(0),
            ProgressState::Indeterminate => self.percent,
            _ => percent.unwrap_or(self.percent),
        };
        self.state = state;
        true
    }
}
