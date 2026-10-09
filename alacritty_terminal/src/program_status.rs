//! OSC 7501 program status records, scoped to a single terminal.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

/// Reported program state. `Clear` removes records and is never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Clear,
}

/// The user action needed by a blocked program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

/// The recognized keys of a program's latest report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatus {
    /// Empty for the root record.
    pub id: String,
    pub state: State,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    /// Explicit app name. Use [`ProgramStatuses::app_for`] to resolve inheritance.
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

impl ProgramStatus {
    fn parse(body: &[u8]) -> Option<Self> {
        let mut state = None;
        let mut id = None;
        let mut kind = None;
        let mut progress = None;
        let mut app = None;
        let mut title = None;
        let mut msg = None;

        for pair in body.split(|byte| *byte == b':') {
            let Some(equals) = pair.iter().position(|byte| *byte == b'=') else { continue };
            let key = pair[..equals].trim_ascii();
            let value = pair[equals + 1..].trim_ascii();
            if key.len() > 16 {
                return None;
            }
            // Check all occurrences, including values later replaced by duplicate keys.
            let limit = match key {
                b"msg" => 2732,
                b"title" => 256,
                b"app" => 32,
                b"id" => 128,
                _ => usize::MAX,
            };
            if value.len() > limit {
                return None;
            }
            // Invalid ids must never fall back to the root record.
            if key == b"id" {
                let value = std::str::from_utf8(value).ok()?;
                if value.split('/').count() > 8
                    || value.split('/').any(|segment| segment.len() > 32)
                {
                    return None;
                }
                id = Some(value);
            }
            if key.is_empty()
                || !key.iter().all(u8::is_ascii_lowercase)
                || !value.iter().all(|c| c.is_ascii_alphanumeric() || b"_.,+/=-".contains(c))
            {
                continue;
            }
            let value = std::str::from_utf8(value).ok()?;
            match key {
                b"state" => state = Some(value),
                b"kind" => kind = Some(value),
                b"progress" => progress = Some(value),
                b"app" => app = Some(value),
                b"title" => title = Some(decode_text(value, 192)?),
                b"msg" => msg = Some(decode_text(value, 2048)?),
                _ => (),
            }
        }

        let state = match state? {
            "idle" => State::Idle,
            "working" => State::Working,
            "done" => State::Done,
            "blocked" => State::Blocked,
            "error" => State::Error,
            "clear" => State::Clear,
            _ => return None,
        };
        if id.is_some_and(|id| !id.split('/').all(valid_segment)) {
            return None;
        }
        let kind = match (state, kind) {
            (State::Blocked, Some("permission")) => Some(BlockedKind::Permission),
            (State::Blocked, Some("question")) => Some(BlockedKind::Question),
            (State::Blocked, Some("auth")) => Some(BlockedKind::Auth),
            _ => None,
        };
        let progress = progress
            .filter(|_| matches!(state, State::Working | State::Blocked))
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|value| *value <= 100);

        Some(Self {
            id: id.unwrap_or_default().into(),
            state,
            kind,
            progress,
            app: app.filter(|value| valid_segment(value)).map(str::to_owned),
            title,
            msg,
        })
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn decode_text(value: &str, limit: usize) -> Option<String> {
    let bytes = STANDARD.decode(value).or_else(|_| STANDARD_NO_PAD.decode(value)).ok()?;
    if bytes.len() > limit {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

/// Bounded records in update order, oldest first.
#[derive(Default)]
pub struct ProgramStatuses {
    records: Vec<ProgramStatus>,
}

impl ProgramStatuses {
    /// Current records, including both root and child records.
    pub fn records(&self) -> &[ProgramStatus] {
        &self.records
    }

    /// Resolve the record's app from itself or its nearest ancestor.
    pub fn app_for(&self, record: &ProgramStatus) -> Option<&str> {
        let mut id = record.id.as_str();
        loop {
            if let Some(app) = self
                .records
                .iter()
                .find(|record| record.id == id)
                .and_then(|record| record.app.as_deref())
            {
                return Some(app);
            }
            if id.is_empty() {
                return None;
            }
            id = id.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }

    pub(crate) fn apply(&mut self, body: &[u8]) -> bool {
        let Some(record) = ProgramStatus::parse(body) else { return false };
        if record.state == State::Clear {
            self.records.retain(|existing| {
                !record.id.is_empty()
                    && existing.id != record.id
                    && !existing
                        .id
                        .strip_prefix(&record.id)
                        .is_some_and(|tail| tail.starts_with('/'))
            });
        } else {
            self.records.retain(|existing| existing.id != record.id);
            if self.records.len() == 256 {
                self.records.remove(0);
            }
            self.records.push(record);
        }
        true
    }

    pub(crate) fn finish(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|record| !matches!(record.state, State::Working | State::Blocked));
        before != self.records.len()
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
    }

    pub(crate) fn window_title(&self, title: Option<&str>) -> Option<String> {
        // Prefer records needing attention, then active work, with newest breaking ties.
        let record = self.records.iter().max_by_key(|record| match record.state {
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
        let label = record.title.as_deref().or_else(|| self.app_for(record)).or(title);
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
}
