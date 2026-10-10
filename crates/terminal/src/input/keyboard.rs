//! Toolkit-independent keyboard encoding for legacy, xterm and Kitty keyboard protocols.
//!
//! [`encode`] implements the protocols themselves. [`encode_with_default_bindings`] also applies
//! the default key bindings of the Alacritty application, such as application cursor keys, for
//! hosts without their own binding system.

use std::borrow::Cow;

use crate::term::TermMode;

bitflags::bitflags! {
    /// Keyboard modifiers, using the bit layout of the Kitty keyboard protocol.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct Modifiers: u32 {
        const SHIFT   = 0b0001;
        const ALT     = 0b0010;
        const CONTROL = 0b0100;
        const SUPER   = 0b1000;
    }
}

impl Modifiers {
    /// Value used in escape sequence modifier parameters.
    fn encode_esc_sequence(self) -> u8 {
        (self & Self::all()).bits() as u8 + 1
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum KeyLocation {
    #[default]
    Standard,
    Left,
    Right,
    Numpad,
}

/// Keys without a printable character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    /// Function keys F1 to F35.
    F(u8),
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    ContextMenu,
    Shift,
    Control,
    Alt,
    Super,
    Hyper,
    Meta,
    MediaPlay,
    MediaPause,
    MediaPlayPause,
    MediaStop,
    MediaFastForward,
    MediaRewind,
    MediaTrackNext,
    MediaTrackPrevious,
    MediaRecord,
    AudioVolumeDown,
    AudioVolumeUp,
    AudioVolumeMute,
}

impl NamedKey {
    /// Text produced by the key without modifiers, as reported by native toolkits.
    fn to_text(self) -> Option<&'static str> {
        match self {
            NamedKey::Enter => Some("\r"),
            NamedKey::Tab => Some("\t"),
            NamedKey::Backspace => Some("\x08"),
            NamedKey::Escape => Some("\x1b"),
            NamedKey::Space => Some(" "),
            _ => None,
        }
    }

    fn is_modifier(self) -> bool {
        matches!(self, NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    /// The logical character with Shift and the keyboard layout applied, but not Control or Alt.
    Character(char),
    /// Keys without a single character or known name, such as dead keys and multi-character
    /// layout output. Only their text is sent.
    Other,
}

impl Key {
    /// Treat control characters sent as characters like their named keys.
    fn normalized(self) -> Self {
        match self {
            Key::Character(' ') => Key::Named(NamedKey::Space),
            Key::Character('\r' | '\n') => Key::Named(NamedKey::Enter),
            Key::Character('\t') => Key::Named(NamedKey::Tab),
            Key::Character('\x1b') => Key::Named(NamedKey::Escape),
            Key::Character('\x08' | '\x7f') => Key::Named(NamedKey::Backspace),
            key => key,
        }
    }
}

/// A key event in toolkit-neutral form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent<'a> {
    pub action: KeyAction,
    pub key: Key,
    /// The key's character without any modifiers, such as `1` for `!` on US layouts.
    pub unshifted: Option<char>,
    /// Text produced with all modifiers applied, such as `\x03` for Ctrl+C.
    ///
    /// When `None`, the text is derived from the key and modifiers.
    pub text: Option<&'a str>,
    pub location: KeyLocation,
    pub mods: Modifiers,
    /// Alt composed the text instead of acting as a modifier, like macOS Option without
    /// "Option as Alt". Composed text is sent without an escape prefix.
    pub alt_composes: bool,
}

impl<'a> KeyEvent<'a> {
    pub fn new(key: Key, mods: Modifiers) -> Self {
        Self {
            action: KeyAction::Press,
            key,
            unshifted: None,
            text: None,
            location: KeyLocation::Standard,
            mods,
            alt_composes: false,
        }
    }

    fn released(&self) -> bool {
        self.action == KeyAction::Release
    }

    fn is_modifier_key(&self) -> bool {
        matches!(self.key, Key::Named(named) if named.is_modifier())
    }

    fn text(&self) -> Cow<'a, str> {
        if self.released() {
            return Cow::Borrowed("");
        }
        match self.text {
            Some(text) => Cow::Borrowed(text),
            None => derived_text(self.key, self.mods),
        }
    }
}

/// Text a native toolkit would report for keys that the host didn't provide text for.
fn derived_text(key: Key, mods: Modifiers) -> Cow<'static, str> {
    let control = mods.contains(Modifiers::CONTROL);
    match key {
        Key::Named(NamedKey::Space) if control => Cow::Borrowed("\0"),
        Key::Named(named) => Cow::Borrowed(named.to_text().unwrap_or_default()),
        Key::Character(character) if control => match control_character(character) {
            Some(byte) => Cow::Owned(char::from(byte).to_string()),
            None => Cow::Owned(character.to_string()),
        },
        Key::Character(character) => Cow::Owned(character.to_string()),
        Key::Other => Cow::Borrowed(""),
    }
}

/// The C0 control code produced by Control and a character.
fn control_character(character: char) -> Option<u8> {
    match character.to_ascii_lowercase() {
        character @ 'a'..='z' => Some(character as u8 - b'a' + 1),
        '@' | '2' | ' ' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Whether input from this key should clear the selection and scroll to the bottom.
pub fn starts_input(event: &KeyEvent<'_>) -> bool {
    !event.is_modifier_key()
}

/// Encode a key event for the terminal's current modes.
///
/// Returns an empty buffer when the key produces no input. Hosts with their own key bindings
/// should resolve them first, like the desktop application does.
pub fn encode(event: &KeyEvent<'_>, mode: TermMode) -> Vec<u8> {
    encode_inner(event, mode, false)
}

/// Encode a key event, applying Alacritty's default terminal key bindings first.
///
/// These cover application cursor keys, F1-F4, Shift+Tab, Backspace and numpad Enter.
pub fn encode_with_default_bindings(event: &KeyEvent<'_>, mode: TermMode) -> Vec<u8> {
    encode_inner(event, mode, true)
}

fn encode_inner(event: &KeyEvent<'_>, mode: TermMode, default_bindings: bool) -> Vec<u8> {
    let event = KeyEvent { key: event.key.normalized(), ..*event };
    let text = event.text();

    if event.released() {
        return encode_release(&event, &text, mode);
    }

    if default_bindings && let Some(bytes) = default_binding(&event, mode) {
        return bytes.into_owned();
    }

    // Mask `Alt` modifier from input when we won't send esc.
    let mods = if alt_send_esc(&event, &text) { event.mods } else { event.mods - Modifiers::ALT };

    if should_build_sequence(&event, &text, mode, mods) {
        build_sequence(&event, &text, mods, mode)
    } else {
        let mut bytes = Vec::with_capacity(text.len() + 1);
        if mods.contains(Modifiers::ALT) {
            bytes.push(b'\x1b');
        }
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }
}

fn encode_release(event: &KeyEvent<'_>, text: &str, mode: TermMode) -> Vec<u8> {
    if !mode.contains(TermMode::REPORT_EVENT_TYPES) {
        return Vec::new();
    }

    let mods = if alt_send_esc(event, text) { event.mods } else { event.mods - Modifiers::ALT };

    match event.key {
        Key::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace)
            if !mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC) =>
        {
            Vec::new()
        },
        _ => build_sequence(event, text, mods, mode),
    }
}

/// Sequences from the desktop application's default key bindings.
fn default_binding(event: &KeyEvent<'_>, mode: TermMode) -> Option<Cow<'static, [u8]>> {
    let Key::Named(named) = event.key else { return None };
    let mods = event.mods;
    let legacy =
        !mode.intersects(TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::DISAMBIGUATE_ESC_CODES);
    let none = mods.is_empty();

    let bytes: &'static [u8] = match named {
        NamedKey::Home if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOH",
        NamedKey::End if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOF",
        NamedKey::ArrowUp if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOA",
        NamedKey::ArrowDown if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOB",
        NamedKey::ArrowRight if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOC",
        NamedKey::ArrowLeft if none && mode.contains(TermMode::APP_CURSOR) => b"\x1bOD",
        NamedKey::F(1) if none && legacy => b"\x1bOP",
        NamedKey::F(2) if none && legacy => b"\x1bOQ",
        NamedKey::F(3) if none && legacy => b"\x1bOR",
        NamedKey::F(4) if none && legacy => b"\x1bOS",
        NamedKey::Tab if mods == Modifiers::SHIFT && legacy => b"\x1b[Z",
        NamedKey::Tab if mods == Modifiers::SHIFT | Modifiers::ALT && legacy => b"\x1b\x1b[Z",
        NamedKey::Backspace if none && !mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC) => b"\x7f",
        NamedKey::Backspace if mods == Modifiers::ALT && legacy => b"\x1b\x7f",
        NamedKey::Backspace if mods == Modifiers::SHIFT && legacy => b"\x7f",
        NamedKey::Enter if none && legacy && event.location == KeyLocation::Numpad => b"\n",
        _ => return None,
    };

    Some(Cow::Borrowed(bytes))
}

/// Whether `Alt` remains a modifier instead of being masked from the input.
fn alt_send_esc(event: &KeyEvent<'_>, text: &str) -> bool {
    let alt = event.mods.contains(Modifiers::ALT);
    let alt_send_esc = alt && !event.alt_composes;
    match event.key {
        Key::Named(named) if named.to_text().is_some() => alt_send_esc,
        // Named keys without text, like ArrowUp, always treat `Alt` as a modifier.
        Key::Named(_) => alt,
        Key::Character(_) | Key::Other => alt_send_esc && text.chars().count() == 1,
    }
}

/// Check whether an escape sequence should be built instead of sending the key's text.
fn should_build_sequence(
    event: &KeyEvent<'_>,
    text: &str,
    mode: TermMode,
    mods: Modifiers,
) -> bool {
    if mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC) {
        return true;
    }

    let disambiguate = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES)
        && (event.key == Key::Named(NamedKey::Escape)
            || event.location == KeyLocation::Numpad
            || (!mods.is_empty()
                && (mods != Modifiers::SHIFT
                    || matches!(
                        event.key,
                        Key::Named(NamedKey::Tab | NamedKey::Enter | NamedKey::Backspace)
                    ))));

    match event.key {
        _ if disambiguate => true,
        // Exclude all the named keys unless they have textual representation.
        Key::Named(named) => named.to_text().is_none(),
        Key::Character(_) | Key::Other => text.is_empty(),
    }
}

/// Build a key's escape sequence based on the given modifiers and terminal mode.
fn build_sequence(
    event: &KeyEvent<'_>,
    text: &str,
    mut mods: Modifiers,
    mode: TermMode,
) -> Vec<u8> {
    let kitty_seq = mode.intersects(
        TermMode::REPORT_ALL_KEYS_AS_ESC
            | TermMode::DISAMBIGUATE_ESC_CODES
            | TermMode::REPORT_EVENT_TYPES,
    );
    let kitty_encode_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    // The default parameter is 1, so we can omit it.
    let kitty_event_type = mode.contains(TermMode::REPORT_EVENT_TYPES)
        && matches!(event.action, KeyAction::Repeat | KeyAction::Release);

    let context = SequenceBuilder { mode, mods, kitty_seq, kitty_encode_all, kitty_event_type };

    let associated_text = Some(text).filter(|text| {
        mode.contains(TermMode::REPORT_ASSOCIATED_TEXT)
            && !event.released()
            && !text.is_empty()
            && !is_control_character(text)
    });

    let sequence_base = context
        .try_build_numpad(event)
        .or_else(|| context.try_build_named_kitty(event))
        .or_else(|| context.try_build_named_normal(event, associated_text.is_some()))
        .or_else(|| context.try_build_control_char_or_mod(event, &mut mods))
        .or_else(|| context.try_build_textual(event, associated_text));

    let Some(SequenceBase { payload, terminator }) = sequence_base else { return Vec::new() };

    let mut payload = format!("\x1b[{payload}");

    // Add modifiers information.
    if kitty_event_type || !mods.is_empty() || associated_text.is_some() {
        payload.push_str(&format!(";{}", mods.encode_esc_sequence()));
    }

    // Push event type.
    if kitty_event_type {
        payload.push(':');
        payload.push(match event.action {
            KeyAction::Press => '1',
            KeyAction::Repeat => '2',
            KeyAction::Release => '3',
        });
    }

    if let Some(text) = associated_text {
        let mut codepoints = text.chars().map(u32::from);
        if let Some(codepoint) = codepoints.next() {
            payload.push_str(&format!(";{codepoint}"));
        }
        for codepoint in codepoints {
            payload.push_str(&format!(":{codepoint}"));
        }
    }

    payload.push(terminator.encode_esc_sequence());

    payload.into_bytes()
}

/// Helper to build escape sequence payloads from a [`KeyEvent`].
struct SequenceBuilder {
    mode: TermMode,
    /// The emitted sequence should follow the kitty keyboard protocol.
    kitty_seq: bool,
    /// Encode all the keys according to the protocol.
    kitty_encode_all: bool,
    /// Report event types.
    kitty_event_type: bool,
    mods: Modifiers,
}

impl SequenceBuilder {
    /// Try building sequence from the event's emitting text.
    fn try_build_textual(
        &self,
        event: &KeyEvent<'_>,
        associated_text: Option<&str>,
    ) -> Option<SequenceBase> {
        let character = match event.key {
            Key::Character(character) if self.kitty_seq => character,
            // Fallback when we need to report text without an associated key.
            Key::Other if self.kitty_encode_all && associated_text.is_some() => {
                return Some(SequenceBase::new("0".into(), SequenceTerminator::Kitty));
            },
            _ => return None,
        };

        let shift = self.mods.contains(Modifiers::SHIFT);
        let unshifted = if shift { character.to_lowercase().next().unwrap() } else { character };

        let alternate_key_code = u32::from(character);
        let mut unicode_key_code = u32::from(unshifted);

        // Use the base for keys which change based on modifier, like `1` for `!`.
        //
        // However it should only be performed when `SHIFT` is pressed.
        if shift && alternate_key_code == unicode_key_code {
            unicode_key_code = u32::from(event.unshifted.unwrap_or(unshifted));
        }

        let payload = if self.mode.contains(TermMode::REPORT_ALTERNATE_KEYS)
            && alternate_key_code != unicode_key_code
        {
            format!("{unicode_key_code}:{alternate_key_code}")
        } else {
            unicode_key_code.to_string()
        };

        Some(SequenceBase::new(payload.into(), SequenceTerminator::Kitty))
    }

    /// Try building from numpad key.
    fn try_build_numpad(&self, event: &KeyEvent<'_>) -> Option<SequenceBase> {
        if !self.kitty_seq || event.location != KeyLocation::Numpad {
            return None;
        }

        let base = match event.key {
            Key::Character('0') => "57399",
            Key::Character('1') => "57400",
            Key::Character('2') => "57401",
            Key::Character('3') => "57402",
            Key::Character('4') => "57403",
            Key::Character('5') => "57404",
            Key::Character('6') => "57405",
            Key::Character('7') => "57406",
            Key::Character('8') => "57407",
            Key::Character('9') => "57408",
            Key::Character('.') => "57409",
            Key::Character('/') => "57410",
            Key::Character('*') => "57411",
            Key::Character('-') => "57412",
            Key::Character('+') => "57413",
            Key::Character('=') => "57415",
            Key::Named(named) => match named {
                NamedKey::Enter => "57414",
                NamedKey::ArrowLeft => "57417",
                NamedKey::ArrowRight => "57418",
                NamedKey::ArrowUp => "57419",
                NamedKey::ArrowDown => "57420",
                NamedKey::PageUp => "57421",
                NamedKey::PageDown => "57422",
                NamedKey::Home => "57423",
                NamedKey::End => "57424",
                NamedKey::Insert => "57425",
                NamedKey::Delete => "57426",
                _ => return None,
            },
            _ => return None,
        };

        Some(SequenceBase::new(base.into(), SequenceTerminator::Kitty))
    }

    /// Try building from [`NamedKey`] using the kitty keyboard protocol encoding for functional
    /// keys.
    fn try_build_named_kitty(&self, event: &KeyEvent<'_>) -> Option<SequenceBase> {
        let named = match event.key {
            Key::Named(named) if self.kitty_seq => named,
            _ => return None,
        };

        let (base, terminator) = match named {
            // F3 in kitty protocol diverges from alacritty's terminfo.
            NamedKey::F(3) => ("13".into(), SequenceTerminator::Normal('~')),
            NamedKey::F(number @ 13..=35) => {
                ((57376 + u32::from(number) - 13).to_string().into(), SequenceTerminator::Kitty)
            },
            NamedKey::ScrollLock => ("57359".into(), SequenceTerminator::Kitty),
            NamedKey::PrintScreen => ("57361".into(), SequenceTerminator::Kitty),
            NamedKey::Pause => ("57362".into(), SequenceTerminator::Kitty),
            NamedKey::ContextMenu => ("57363".into(), SequenceTerminator::Kitty),
            NamedKey::MediaPlay => ("57428".into(), SequenceTerminator::Kitty),
            NamedKey::MediaPause => ("57429".into(), SequenceTerminator::Kitty),
            NamedKey::MediaPlayPause => ("57430".into(), SequenceTerminator::Kitty),
            NamedKey::MediaStop => ("57432".into(), SequenceTerminator::Kitty),
            NamedKey::MediaFastForward => ("57433".into(), SequenceTerminator::Kitty),
            NamedKey::MediaRewind => ("57434".into(), SequenceTerminator::Kitty),
            NamedKey::MediaTrackNext => ("57435".into(), SequenceTerminator::Kitty),
            NamedKey::MediaTrackPrevious => ("57436".into(), SequenceTerminator::Kitty),
            NamedKey::MediaRecord => ("57437".into(), SequenceTerminator::Kitty),
            NamedKey::AudioVolumeDown => ("57438".into(), SequenceTerminator::Kitty),
            NamedKey::AudioVolumeUp => ("57439".into(), SequenceTerminator::Kitty),
            NamedKey::AudioVolumeMute => ("57440".into(), SequenceTerminator::Kitty),
            _ => return None,
        };

        Some(SequenceBase::new(base, terminator))
    }

    /// Try building from [`NamedKey`].
    fn try_build_named_normal(
        &self,
        event: &KeyEvent<'_>,
        has_associated_text: bool,
    ) -> Option<SequenceBase> {
        let Key::Named(named) = event.key else { return None };

        // The default parameter is 1, so we can omit it.
        let one_based = if self.mods.is_empty() && !self.kitty_event_type && !has_associated_text {
            ""
        } else {
            "1"
        };
        let (base, terminator) = match named {
            NamedKey::PageUp => ("5", SequenceTerminator::Normal('~')),
            NamedKey::PageDown => ("6", SequenceTerminator::Normal('~')),
            NamedKey::Insert => ("2", SequenceTerminator::Normal('~')),
            NamedKey::Delete => ("3", SequenceTerminator::Normal('~')),
            NamedKey::Home => (one_based, SequenceTerminator::Normal('H')),
            NamedKey::End => (one_based, SequenceTerminator::Normal('F')),
            NamedKey::ArrowLeft => (one_based, SequenceTerminator::Normal('D')),
            NamedKey::ArrowRight => (one_based, SequenceTerminator::Normal('C')),
            NamedKey::ArrowUp => (one_based, SequenceTerminator::Normal('A')),
            NamedKey::ArrowDown => (one_based, SequenceTerminator::Normal('B')),
            NamedKey::F(1) => (one_based, SequenceTerminator::Normal('P')),
            NamedKey::F(2) => (one_based, SequenceTerminator::Normal('Q')),
            NamedKey::F(3) => (one_based, SequenceTerminator::Normal('R')),
            NamedKey::F(4) => (one_based, SequenceTerminator::Normal('S')),
            NamedKey::F(5) => ("15", SequenceTerminator::Normal('~')),
            NamedKey::F(6) => ("17", SequenceTerminator::Normal('~')),
            NamedKey::F(7) => ("18", SequenceTerminator::Normal('~')),
            NamedKey::F(8) => ("19", SequenceTerminator::Normal('~')),
            NamedKey::F(9) => ("20", SequenceTerminator::Normal('~')),
            NamedKey::F(10) => ("21", SequenceTerminator::Normal('~')),
            NamedKey::F(11) => ("23", SequenceTerminator::Normal('~')),
            NamedKey::F(12) => ("24", SequenceTerminator::Normal('~')),
            NamedKey::F(13) => ("25", SequenceTerminator::Normal('~')),
            NamedKey::F(14) => ("26", SequenceTerminator::Normal('~')),
            NamedKey::F(15) => ("28", SequenceTerminator::Normal('~')),
            NamedKey::F(16) => ("29", SequenceTerminator::Normal('~')),
            NamedKey::F(17) => ("31", SequenceTerminator::Normal('~')),
            NamedKey::F(18) => ("32", SequenceTerminator::Normal('~')),
            NamedKey::F(19) => ("33", SequenceTerminator::Normal('~')),
            NamedKey::F(20) => ("34", SequenceTerminator::Normal('~')),
            _ => return None,
        };

        Some(SequenceBase::new(base.into(), terminator))
    }

    /// Try building escape from control characters (e.g. Enter) and modifiers.
    fn try_build_control_char_or_mod(
        &self,
        event: &KeyEvent<'_>,
        mods: &mut Modifiers,
    ) -> Option<SequenceBase> {
        if !self.kitty_encode_all && !self.kitty_seq {
            return None;
        }

        let Key::Named(named) = event.key else { return None };

        let base = match named {
            NamedKey::Tab => "9",
            NamedKey::Enter => "13",
            NamedKey::Escape => "27",
            NamedKey::Space => "32",
            NamedKey::Backspace => "127",
            _ => "",
        };

        // Fail when the key is not a named control character and the active mode prohibits us
        // from encoding modifier keys.
        if !self.kitty_encode_all && base.is_empty() {
            return None;
        }

        let base = match (named, event.location) {
            (NamedKey::Shift, KeyLocation::Left) => "57441",
            (NamedKey::Control, KeyLocation::Left) => "57442",
            (NamedKey::Alt, KeyLocation::Left) => "57443",
            (NamedKey::Super, KeyLocation::Left) => "57444",
            (NamedKey::Hyper, KeyLocation::Left) => "57445",
            (NamedKey::Meta, KeyLocation::Left) => "57446",
            (NamedKey::Shift, _) => "57447",
            (NamedKey::Control, _) => "57448",
            (NamedKey::Alt, _) => "57449",
            (NamedKey::Super, _) => "57450",
            (NamedKey::Hyper, _) => "57451",
            (NamedKey::Meta, _) => "57452",
            (NamedKey::CapsLock, _) => "57358",
            (NamedKey::NumLock, _) => "57360",
            _ => base,
        };

        // Kitty's protocol mandates that the modifier state is applied before key press, so
        // modifier keys report their own state like kitty does.
        let press = !event.released();
        match named {
            NamedKey::Shift => mods.set(Modifiers::SHIFT, press),
            NamedKey::Control => mods.set(Modifiers::CONTROL, press),
            NamedKey::Alt => mods.set(Modifiers::ALT, press),
            NamedKey::Super => mods.set(Modifiers::SUPER, press),
            _ => (),
        }

        if base.is_empty() {
            None
        } else {
            Some(SequenceBase::new(base.into(), SequenceTerminator::Kitty))
        }
    }
}

struct SequenceBase {
    /// The base of the payload, which is the `number` and optionally an alt base from the kitty
    /// spec.
    payload: Cow<'static, str>,
    terminator: SequenceTerminator,
}

impl SequenceBase {
    fn new(payload: Cow<'static, str>, terminator: SequenceTerminator) -> Self {
        Self { payload, terminator }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceTerminator {
    /// The normal key esc sequence terminator defined by xterm/dec.
    Normal(char),
    /// The terminator is for kitty escape sequence.
    Kitty,
}

impl SequenceTerminator {
    fn encode_esc_sequence(self) -> char {
        match self {
            SequenceTerminator::Normal(char) => char,
            SequenceTerminator::Kitty => 'u',
        }
    }
}

/// Check whether the `text` is `0x7f`, `C0` or `C1` control code.
fn is_control_character(text: &str) -> bool {
    // 0x7f (DEL) is included here since it has a dedicated control code (`^?`) which generally
    // does not match the reported text (`^H`), despite not technically being part of C0 or C1.
    let codepoint = text.bytes().next().unwrap();
    text.len() == 1 && (codepoint < 0x20 || (0x7f..=0x9f).contains(&codepoint))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Most expectations match the application with its default bindings.
    use super::encode_with_default_bindings as encode;

    fn key(key: Key, mods: Modifiers, mode: TermMode) -> Vec<u8> {
        encode(&KeyEvent::new(key, mods), mode)
    }

    fn named(named: NamedKey, mods: Modifiers, mode: TermMode) -> Vec<u8> {
        key(Key::Named(named), mods, mode)
    }

    const KITTY: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;

    #[test]
    fn legacy_text_and_control_characters() {
        let mode = TermMode::default();
        assert_eq!(key(Key::Character('a'), Modifiers::empty(), mode), b"a");
        assert_eq!(key(Key::Character('c'), Modifiers::CONTROL, mode), b"\x03");
        assert_eq!(key(Key::Character('C'), Modifiers::CONTROL | Modifiers::SHIFT, mode), b"\x03");
        assert_eq!(key(Key::Character('b'), Modifiers::ALT, mode), b"\x1bb");
        assert_eq!(named(NamedKey::Space, Modifiers::CONTROL, mode), b"\0");
        assert_eq!(named(NamedKey::Enter, Modifiers::empty(), mode), b"\r");
        assert_eq!(named(NamedKey::Backspace, Modifiers::empty(), mode), b"\x7f");
        assert_eq!(named(NamedKey::Backspace, Modifiers::ALT, mode), b"\x1b\x7f");
        assert_eq!(named(NamedKey::Tab, Modifiers::SHIFT, mode), b"\x1b[Z");
        assert_eq!(key(Key::Character('\r'), Modifiers::empty(), mode), b"\r");
    }

    #[test]
    fn host_text_takes_precedence() {
        let mut event = KeyEvent::new(Key::Character('e'), Modifiers::empty());
        event.text = Some("é");
        assert_eq!(encode(&event, TermMode::default()), "é".as_bytes());
    }

    #[test]
    fn cursor_keys_follow_application_cursor_mode() {
        let mode = TermMode::default();
        assert_eq!(named(NamedKey::ArrowUp, Modifiers::empty(), mode), b"\x1b[A");
        assert_eq!(named(NamedKey::ArrowUp, Modifiers::empty(), TermMode::APP_CURSOR), b"\x1bOA");
        assert_eq!(named(NamedKey::ArrowLeft, Modifiers::CONTROL, mode), b"\x1b[1;5D");
        assert_eq!(
            named(NamedKey::ArrowLeft, Modifiers::CONTROL, TermMode::APP_CURSOR),
            b"\x1b[1;5D"
        );
        assert_eq!(named(NamedKey::F(1), Modifiers::empty(), mode), b"\x1bOP");
        assert_eq!(named(NamedKey::F(1), Modifiers::SHIFT, mode), b"\x1b[1;2P");
        assert_eq!(named(NamedKey::F(5), Modifiers::empty(), mode), b"\x1b[15~");
        assert_eq!(named(NamedKey::Delete, Modifiers::empty(), mode), b"\x1b[3~");
    }

    #[test]
    fn kitty_disambiguates_modified_keys() {
        assert_eq!(key(Key::Character('a'), Modifiers::empty(), KITTY), b"a");
        assert_eq!(key(Key::Character('c'), Modifiers::CONTROL, KITTY), b"\x1b[99;5u");
        assert_eq!(named(NamedKey::Escape, Modifiers::empty(), KITTY), b"\x1b[27u");
        assert_eq!(named(NamedKey::Enter, Modifiers::SHIFT, KITTY), b"\x1b[13;2u");
        assert_eq!(named(NamedKey::F(3), Modifiers::empty(), KITTY), b"\x1b[13~");
        assert_eq!(named(NamedKey::F(13), Modifiers::empty(), KITTY), b"\x1b[57376u");
        // Unmodified Backspace keeps its legacy byte under disambiguation.
        assert_eq!(named(NamedKey::Backspace, Modifiers::empty(), KITTY), b"\x7f");
    }

    #[test]
    fn kitty_reports_event_types_and_alternates() {
        let mode = KITTY | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS;
        let mut event = KeyEvent::new(Key::Character('a'), Modifiers::empty());
        event.action = KeyAction::Release;
        assert_eq!(encode(&event, mode), b"\x1b[97;1:3u");
        // Text keys keep sending text unless all keys are reported.
        event.action = KeyAction::Repeat;
        assert_eq!(encode(&event, mode), b"a");
        assert_eq!(encode(&event, mode | TermMode::REPORT_ALL_KEYS_AS_ESC), b"\x1b[97;1:2u");

        let mut shifted = KeyEvent::new(Key::Character('!'), Modifiers::SHIFT);
        shifted.unshifted = Some('1');
        assert_eq!(encode(&shifted, mode | TermMode::REPORT_ALL_KEYS_AS_ESC), b"\x1b[49:33;2u");

        // Releases are dropped without event type reporting.
        event.action = KeyAction::Release;
        assert!(encode(&event, KITTY).is_empty());
    }

    #[test]
    fn kitty_report_all_keys_with_associated_text() {
        let mode = TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(key(Key::Character('a'), Modifiers::empty(), mode), b"\x1b[97;1;97u");
        let mut shift = KeyEvent::new(Key::Named(NamedKey::Shift), Modifiers::SHIFT);
        shift.location = KeyLocation::Left;
        assert_eq!(encode(&shift, mode), b"\x1b[57441;2u");
        assert_eq!(
            encode(
                &KeyEvent {
                    location: KeyLocation::Numpad,
                    ..KeyEvent::new(Key::Character('1'), Modifiers::empty())
                },
                mode
            ),
            b"\x1b[57400;1;49u"
        );
    }

    #[test]
    fn protocol_encoding_leaves_bindings_to_the_host() {
        let up = KeyEvent::new(Key::Named(NamedKey::ArrowUp), Modifiers::empty());
        assert_eq!(super::encode(&up, TermMode::APP_CURSOR), b"\x1b[A");
        let backspace = KeyEvent::new(Key::Named(NamedKey::Backspace), Modifiers::empty());
        assert_eq!(super::encode(&backspace, TermMode::default()), b"\x08");
    }

    #[test]
    fn composed_alt_text_has_no_escape_prefix() {
        let mut event = KeyEvent::new(Key::Character('a'), Modifiers::ALT);
        event.text = Some("å");
        assert_eq!(encode(&event, TermMode::default()), b"\x1b\xc3\xa5");
        event.alt_composes = true;
        assert_eq!(encode(&event, TermMode::default()), "å".as_bytes());
        // Named keys without text keep Alt as a modifier.
        let mut left = KeyEvent::new(Key::Named(NamedKey::ArrowLeft), Modifiers::ALT);
        left.alt_composes = true;
        assert_eq!(encode(&left, TermMode::default()), b"\x1b[1;3D");
    }

    #[test]
    fn other_keys_send_text_only() {
        let mut event = KeyEvent::new(Key::Other, Modifiers::empty());
        event.text = Some("ab");
        assert_eq!(encode(&event, TermMode::default()), b"ab");
        let mode = TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(encode(&event, mode), b"\x1b[0;1;97:98u");
        event.text = None;
        assert!(encode(&event, TermMode::default()).is_empty());
    }
}
