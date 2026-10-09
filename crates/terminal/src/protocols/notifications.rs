//! Bounded OSC 99 notification assembly. Native delivery and focus policy belong to the host.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

const MAX_PENDING: usize = 32;
const MAX_ACTIVE: usize = 32;
const MAX_TEXT: usize = 32 * 1024;
const MAX_ID: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Occasion {
    #[default]
    Always,
    Unfocused,
    Invisible,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub serial: u64,
    pub id: String,
    pub title: String,
    pub body: String,
    pub buttons: Vec<String>,
    pub focus: bool,
    pub report: bool,
    pub report_close: bool,
    pub occasion: Occasion,
    pub urgency: u8,
    pub sound: String,
    pub application: String,
    pub icons: Vec<String>,
    pub expires: Option<Instant>,
    pub persistent: bool,
}

impl Default for Notification {
    fn default() -> Self {
        Self {
            serial: 0,
            id: String::new(),
            title: String::new(),
            body: String::new(),
            buttons: vec![],
            focus: true,
            report: false,
            report_close: false,
            occasion: Occasion::Always,
            urgency: 1,
            sound: "system".into(),
            application: String::new(),
            icons: vec![],
            expires: None,
            persistent: false,
        }
    }
}

#[derive(Debug)]
pub enum Effect {
    Show(Box<Notification>),
    Close(u64),
    Reply(String),
    QueryAlive(String),
}

#[derive(Default)]
struct Text {
    data: Vec<u8>,
    base64: Vec<u8>,
}

impl Text {
    fn append(&mut self, payload: &[u8], encoded: bool) -> Option<()> {
        if payload.len() > if encoded { 4096 } else { 2048 } {
            return None;
        }
        if encoded {
            self.base64.extend_from_slice(payload);
            let complete = self.base64.len() / 4 * 4;
            for chunk in self.base64[..complete].as_chunks::<4>().0 {
                self.data.extend(STANDARD.decode(chunk).ok()?);
            }
            self.base64.drain(..complete);
        } else {
            if !self.base64.is_empty()
                || std::str::from_utf8(payload).ok()?.chars().any(char::is_control)
            {
                return None;
            }
            self.data.extend_from_slice(payload);
        }
        (self.data.len() <= MAX_TEXT).then_some(())
    }

    fn finish(mut self) -> Option<String> {
        if !self.base64.is_empty() {
            self.data.extend(STANDARD_NO_PAD.decode(&self.base64).ok()?);
        }
        let value = String::from_utf8(self.data).ok()?;
        if value.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) {
            return None;
        }
        Some(value)
    }
}

struct Pending {
    notification: Notification,
    title: Text,
    body: Text,
    buttons: Text,
    updated: Instant,
}

impl Pending {
    fn new(now: Instant) -> Self {
        Self {
            notification: Default::default(),
            title: Default::default(),
            body: Default::default(),
            buttons: Default::default(),
            updated: now,
        }
    }
}

#[derive(Default)]
pub struct Notifications {
    pending: HashMap<String, Pending>,
    active: HashMap<u64, Notification>,
    serial: u64,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'.'))
}

fn decode(value: &str) -> Option<String> {
    let bytes = STANDARD.decode(value).or_else(|_| STANDARD_NO_PAD.decode(value)).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (text.len() <= 1024 && !text.chars().any(char::is_control)).then_some(text)
}

fn reply(id: &str, kind: Option<&str>, payload: &str) -> String {
    let id = if id.is_empty() { "0" } else { id };
    let kind = kind.map(|p| format!(":p={p}")).unwrap_or_default();
    format!("\x1b]99;i={id}{kind};{payload}\x1b\\")
}

impl Notifications {
    pub fn apply(
        &mut self,
        packet: &[u8],
        truncated: bool,
        now: Instant,
        focused: bool,
        visible: bool,
        enabled: bool,
    ) -> Vec<Effect> {
        let Some(separator) = packet.iter().position(|b| *b == b';') else { return vec![] };
        let Ok(metadata) = std::str::from_utf8(&packet[..separator]) else { return vec![] };
        let fields: Vec<_> =
            metadata.split(':').filter_map(|field| field.split_once('=')).collect();
        let id = fields.iter().rev().find(|(key, _)| *key == "i").map_or("", |(_, value)| *value);
        if !id.is_empty() && !valid_id(id) {
            return vec![];
        }
        self.pending.retain(|_, value| {
            now.saturating_duration_since(value.updated) < Duration::from_secs(60)
        });
        if truncated || metadata.len() > 4096 {
            self.pending.remove(id);
            return vec![];
        }
        let payload = &packet[separator + 1..];
        let kind =
            fields.iter().rev().find(|(key, _)| *key == "p").map_or("title", |(_, value)| *value);
        match kind {
            "?" => {
                return vec![Effect::Reply(reply(
                    id,
                    Some("?"),
                    "a=focus,report:c=1:o=always,unfocused,invisible:p=title,body,buttons,close,\
                     alive,?:s=system,silent:u=0,1,2:w=1",
                ))];
            },
            "alive" => return vec![Effect::QueryAlive(id.into())],
            "close" => {
                self.pending.remove(id);
                let Some(serial) = self
                    .active
                    .iter()
                    .find(|(_, n)| !id.is_empty() && n.id == id)
                    .map(|(serial, _)| *serial)
                else {
                    return vec![];
                };
                let mut effects = vec![Effect::Close(serial)];
                effects.extend(self.closed(serial));
                return effects;
            },
            "title" | "body" | "buttons" => (),
            _ => return vec![],
        }
        if !enabled {
            self.pending.remove(id);
            return vec![];
        }
        let mut pending = match self.pending.remove(id) {
            Some(pending) => pending,
            None if self.pending.len() < MAX_PENDING => Pending::new(now),
            None => return vec![],
        };
        let n = &mut pending.notification;
        n.id = id.into();
        let mut encoded = false;
        let mut done = true;
        for (key, value) in fields {
            match (key, value) {
                ("e", "1") => encoded = true,
                ("e", "0") => encoded = false,
                ("d", "0") => done = false,
                ("d", "1") => done = true,
                ("c", "0") => n.report_close = false,
                ("c", "1") => n.report_close = true,
                ("o", "always") => n.occasion = Occasion::Always,
                ("o", "unfocused") => n.occasion = Occasion::Unfocused,
                ("o", "invisible") => n.occasion = Occasion::Invisible,
                ("a", actions) => {
                    for action in actions.split(',') {
                        match action {
                            "focus" => n.focus = true,
                            "-focus" => n.focus = false,
                            "report" => n.report = true,
                            "-report" => n.report = false,
                            _ => (),
                        }
                    }
                },
                ("u", value) => {
                    if let Ok(value @ 0..=2) = value.parse() {
                        n.urgency = value;
                    }
                },
                ("s", value) => {
                    if let Some(value) = decode(value) {
                        n.sound = value;
                    }
                },
                ("f", value) => {
                    if let Some(value) = decode(value) {
                        n.application = value;
                    }
                },
                ("n", value) => {
                    if n.icons.len() < 8
                        && let Some(value) = decode(value)
                    {
                        n.icons.push(value);
                    }
                },
                ("w", value) => {
                    if let Ok(value) = value.parse::<i64>() {
                        if value < -1 {
                            return vec![];
                        }
                        n.persistent = value == 0;
                        n.expires = if value > 0 {
                            now.checked_add(Duration::from_millis(value as u64))
                        } else {
                            None
                        };
                    }
                },
                ("e" | "d" | "c" | "o", _) => return vec![],
                _ => (),
            }
        }
        let text = match kind {
            "body" => &mut pending.body,
            "buttons" => &mut pending.buttons,
            _ => &mut pending.title,
        };
        if text.append(payload, encoded).is_none() {
            return vec![];
        }
        pending.updated = now;
        if !done {
            self.pending.insert(id.into(), pending);
            return vec![];
        }
        let (Some(title), Some(body), Some(buttons)) =
            (pending.title.finish(), pending.body.finish(), pending.buttons.finish())
        else {
            return vec![];
        };
        let mut n = pending.notification;
        n.title = title;
        n.body = body;
        n.buttons = buttons
            .split('\u{2028}')
            .filter(|s| !s.is_empty())
            .take(8)
            .map(str::to_owned)
            .collect();
        if n.title.is_empty() {
            n.title = std::mem::take(&mut n.body);
        }
        if n.title.is_empty()
            || (n.occasion != Occasion::Always && focused)
            || (n.occasion == Occasion::Invisible && visible)
        {
            return vec![];
        }
        let mut effects = vec![];
        if let Some(old) = self
            .active
            .iter()
            .find(|(_, n)| !id.is_empty() && n.id == id)
            .map(|(serial, _)| *serial)
        {
            self.active.remove(&old);
            effects.push(Effect::Close(old));
        }
        if self.active.len() >= MAX_ACTIVE {
            return effects;
        }
        self.serial = self.serial.wrapping_add(1);
        n.serial = self.serial;
        self.active.insert(n.serial, n.clone());
        effects.push(Effect::Show(Box::new(n)));
        effects
    }

    pub fn activated(&mut self, serial: u64, button: Option<usize>) -> (bool, Vec<Effect>) {
        let Some(notification) = self.active.get(&serial) else { return (false, vec![]) };
        if button.is_some_and(|index| index == 0 || index > notification.buttons.len()) {
            return (false, vec![]);
        }
        let focus = notification.focus;
        let mut effects = vec![];
        if notification.report {
            effects.push(Effect::Reply(reply(
                &notification.id,
                None,
                &button.map(|b| b.to_string()).unwrap_or_default(),
            )));
        }
        effects.push(Effect::Close(serial));
        effects.extend(self.closed(serial));
        (focus, effects)
    }

    pub fn closed(&mut self, serial: u64) -> Vec<Effect> {
        self.active
            .remove(&serial)
            .filter(|n| n.report_close)
            .map(|n| vec![Effect::Reply(reply(&n.id, Some("close"), ""))])
            .unwrap_or_default()
    }

    pub fn alive(&self, query: &str, serials: &[u64]) -> String {
        let mut ids: Vec<_> = serials
            .iter()
            .filter_map(|s| self.active.get(s))
            .filter(|n| !n.id.is_empty())
            .map(|n| n.id.as_str())
            .collect();
        ids.sort_unstable();
        reply(query, Some("alive"), &ids.join(","))
    }

    pub fn expire(&mut self, now: Instant) -> Vec<Effect> {
        let expired: Vec<_> = self
            .active
            .iter()
            .filter(|(_, n)| n.expires.is_some_and(|at| at <= now))
            .map(|(id, _)| *id)
            .collect();
        let mut effects = vec![];
        for serial in expired {
            effects.push(Effect::Close(serial));
            effects.extend(self.closed(serial));
        }
        effects
    }

    pub fn next_expiry(&self) -> Option<Instant> {
        self.active.values().filter_map(|n| n.expires).min()
    }

    pub fn reset(&mut self) -> Vec<Effect> {
        self.pending.clear();
        self.active.drain().map(|(id, _)| Effect::Close(id)).collect()
    }
}
