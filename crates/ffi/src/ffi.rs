//! C ABI declared in `include/alacritty.h`.
//!
//! Handles are not thread-safe: call functions for one terminal from one thread at a time. The
//! wakeup callback may run on any thread, possibly while the terminal is locked, so it must only
//! schedule a call to [`alac_terminal_poll_event`] and never call into the library itself.
//!
//! Strings returned as `char *` are owned by the caller and released with [`alac_string_free`].
//! Pointers inside events and snapshots stay valid until the next poll or the snapshot is freed.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::time::Instant;

use alacritty_terminal::clipboard::{TerminalClipboardContent, TerminalClipboardLocation};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::Direction;
use alacritty_terminal::program_status::{BlockedKind, State};
use alacritty_terminal::protocols::notifications::{Notification, Occasion};
use alacritty_terminal::protocols::progress::{Progress, ProgressState};
use alacritty_terminal::protocols::shell::Phase;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::color::COUNT;
use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::{Osc52, TermMode};
use alacritty_terminal::vi_mode::ViMotion;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};

use crate::colors::{pack, unpack};
use crate::engine::{
    ClipboardProvider, Engine, HostEvent, Link, Options, ProgramStatusInfo, PtyOptions, Writer,
};
use crate::keyboard::{self, Key, KeyAction, KeyEvent, KeyLocation, Modifiers, NamedKey};
use crate::mouse::MouseButton;
use crate::snapshot::{self, Cell, Cursor, Image, LineDamage, Snapshot, c_string};

/// Incremented on incompatible changes to the C ABI, and the major version of the shared
/// library's soname. Defined in `build.rs`.
pub const ALAC_ABI_VERSION: u32 = match u32::from_str_radix(env!("ALAC_ABI_VERSION"), 10) {
    Ok(version) => version,
    Err(_) => panic!("invalid ALAC_ABI_VERSION"),
};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(message: impl AsRef<str>) {
    LAST_ERROR.with(|error| *error.borrow_mut() = Some(c_string(message.as_ref())));
}

/// Run `f`, converting panics into `default` and a last error.
fn guard<R>(default: R, f: impl FnOnce() -> R) -> R {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|message| message.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            set_error(format!("panic: {message}"));
            default
        },
    }
}

unsafe fn str_arg<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}

unsafe fn bytes_arg<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(data, len) } }
}

unsafe fn str_list(values: *const *const c_char, count: usize) -> Vec<String> {
    if values.is_null() {
        return Vec::new();
    }
    let values = unsafe { std::slice::from_raw_parts(values, count) };
    values.iter().filter_map(|&value| unsafe { str_arg(value) }).map(str::to_owned).collect()
}

fn owned_string(value: Option<String>) -> *mut c_char {
    value.map_or(ptr::null_mut(), |value| c_string(&value).into_raw())
}

/// Copy `text` into a caller buffer, returning the full byte length without the terminator.
unsafe fn copy_out(text: &[u8], out: *mut u8, cap: usize) -> usize {
    if !out.is_null() && cap > 0 {
        let len = text.len().min(cap - 1);
        unsafe {
            ptr::copy_nonoverlapping(text.as_ptr(), out, len);
            *out.add(len) = 0;
        }
    }
    text.len()
}

/// Opaque terminal handle.
pub struct AlacTerminal {
    engine: Engine,
    event: EventStorage,
}

#[derive(Default)]
struct EventStorage {
    data: Vec<u8>,
    notification: Option<NotificationStorage>,
}

struct NotificationStorage {
    _strings: Vec<CString>,
    _buttons: Vec<*const c_char>,
    _icons: Vec<*const c_char>,
    value: AlacNotification,
}

pub const ALAC_OSC52_DISABLED: u32 = 0;
pub const ALAC_OSC52_ONLY_COPY: u32 = 1;
pub const ALAC_OSC52_ONLY_PASTE: u32 = 2;
pub const ALAC_OSC52_COPY_PASTE: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacOptions {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub scrollback: u32,
    pub osc52: u32,
    pub cursor_shape: u32,
    pub cursor_blinking: bool,
    pub kitty_keyboard: bool,
    pub bold_is_bright: bool,
    pub scroll_on_input: bool,
    pub desktop_notifications: bool,
    pub semantic_escape_chars: *const c_char,
}

impl From<&Options> for AlacOptions {
    fn from(options: &Options) -> Self {
        Self {
            columns: options.columns as u32,
            rows: options.rows as u32,
            cell_width: options.cell_width,
            cell_height: options.cell_height,
            scrollback: options.scrollback as u32,
            osc52: match options.osc52 {
                Osc52::Disabled => ALAC_OSC52_DISABLED,
                Osc52::OnlyCopy => ALAC_OSC52_ONLY_COPY,
                Osc52::OnlyPaste => ALAC_OSC52_ONLY_PASTE,
                Osc52::CopyPaste => ALAC_OSC52_COPY_PASTE,
            },
            cursor_shape: snapshot_shape(options.cursor_style.shape),
            cursor_blinking: options.cursor_style.blinking,
            kitty_keyboard: options.kitty_keyboard,
            bold_is_bright: options.bold_is_bright,
            scroll_on_input: options.scroll_on_input,
            desktop_notifications: options.desktop_notifications,
            semantic_escape_chars: ptr::null(),
        }
    }
}

fn snapshot_shape(shape: CursorShape) -> u32 {
    match shape {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Beam => 2,
        CursorShape::HollowBlock => 3,
        CursorShape::Hidden => 4,
    }
}

unsafe fn options_arg(options: *const AlacOptions) -> Options {
    let Some(options) = (unsafe { options.as_ref() }) else { return Options::default() };
    Options {
        columns: options.columns as usize,
        rows: options.rows as usize,
        cell_width: options.cell_width,
        cell_height: options.cell_height,
        scrollback: options.scrollback as usize,
        kitty_keyboard: options.kitty_keyboard,
        osc52: match options.osc52 {
            ALAC_OSC52_DISABLED => Osc52::Disabled,
            ALAC_OSC52_ONLY_PASTE => Osc52::OnlyPaste,
            ALAC_OSC52_COPY_PASTE => Osc52::CopyPaste,
            _ => Osc52::OnlyCopy,
        },
        cursor_style: CursorStyle {
            shape: match options.cursor_shape {
                1 => CursorShape::Underline,
                2 => CursorShape::Beam,
                3 => CursorShape::HollowBlock,
                4 => CursorShape::Hidden,
                _ => CursorShape::Block,
            },
            blinking: options.cursor_blinking,
        },
        bold_is_bright: options.bold_is_bright,
        scroll_on_input: options.scroll_on_input,
        desktop_notifications: options.desktop_notifications,
        semantic_escape_chars: unsafe { str_arg(options.semantic_escape_chars) }.map(str::to_owned),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacPtyOptions {
    pub program: *const c_char,
    pub args: *const *const c_char,
    pub arg_count: usize,
    pub working_directory: *const c_char,
    /// `KEY=VALUE` entries.
    pub env: *const *const c_char,
    pub env_count: usize,
    pub drain_on_exit: bool,
}

pub type AlacWakeupFn = Option<unsafe extern "C" fn(userdata: *mut c_void)>;

struct Userdata(*mut c_void);

// SAFETY: The C API requires wakeup userdata to be usable from any thread.
unsafe impl Send for Userdata {}
unsafe impl Sync for Userdata {}

fn wakeup(callback: AlacWakeupFn, userdata: *mut c_void) -> Option<Box<dyn Fn() + Send + Sync>> {
    let callback = callback?;
    let userdata = Userdata(userdata);
    Some(Box::new(move || {
        let Userdata(userdata) = &userdata;
        unsafe { callback(*userdata) }
    }))
}

/// ABI version of the loaded library.
#[unsafe(no_mangle)]
pub extern "C" fn alac_abi_version() -> u32 {
    ALAC_ABI_VERSION
}

/// Library version as a static NUL-terminated string.
#[unsafe(no_mangle)]
pub extern "C" fn alac_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Last error on this thread, or null. Valid until the next failing call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn alac_last_error() -> *const c_char {
    LAST_ERROR.with(|error| error.borrow().as_ref().map_or(ptr::null(), |error| error.as_ptr()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_options_default(out: *mut AlacOptions) {
    if let Some(out) = unsafe { out.as_mut() } {
        *out = AlacOptions::from(&Options::default());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

/// Create a terminal whose child output is provided through `alac_terminal_feed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_new_headless(
    options: *const AlacOptions,
    callback: AlacWakeupFn,
    userdata: *mut c_void,
) -> *mut AlacTerminal {
    guard(ptr::null_mut(), || {
        let engine = Engine::headless(unsafe { options_arg(options) }, wakeup(callback, userdata));
        Box::into_raw(Box::new(AlacTerminal { engine, event: Default::default() }))
    })
}

/// Create a terminal running a child process on a new PTY. Returns null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_spawn(
    options: *const AlacOptions,
    pty: *const AlacPtyOptions,
    callback: AlacWakeupFn,
    userdata: *mut c_void,
) -> *mut AlacTerminal {
    guard(ptr::null_mut(), || {
        let mut pty_options = PtyOptions::default();
        if let Some(pty) = unsafe { pty.as_ref() } {
            pty_options.program = unsafe { str_arg(pty.program) }.map(str::to_owned);
            pty_options.args = unsafe { str_list(pty.args, pty.arg_count) };
            pty_options.working_directory =
                unsafe { str_arg(pty.working_directory) }.map(Into::into);
            for entry in unsafe { str_list(pty.env, pty.env_count) } {
                if let Some((key, value)) = entry.split_once('=') {
                    pty_options.env.insert(key.into(), value.into());
                }
            }
            pty_options.drain_on_exit = pty.drain_on_exit;
        }
        let options = unsafe { options_arg(options) };
        match Engine::spawn(options, pty_options, wakeup(callback, userdata)) {
            Ok(engine) => {
                Box::into_raw(Box::new(AlacTerminal { engine, event: Default::default() }))
            },
            Err(err) => {
                set_error(format!("failed to spawn PTY: {err}"));
                ptr::null_mut()
            },
        }
    })
}

/// Destroy a terminal, asking its PTY child to shut down.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_free(term: *mut AlacTerminal) {
    if !term.is_null() {
        guard((), || drop(unsafe { Box::from_raw(term) }));
    }
}

macro_rules! term {
    ($term:expr) => {
        match unsafe { $term.as_mut() } {
            Some(term) => term,
            None => return,
        }
    };
    ($term:expr, $default:expr) => {
        match unsafe { $term.as_mut() } {
            Some(term) => term,
            None => return $default,
        }
    };
}

macro_rules! term_ref {
    ($term:expr) => {
        match unsafe { $term.as_ref() } {
            Some(term) => term,
            None => return,
        }
    };
    ($term:expr, $default:expr) => {
        match unsafe { $term.as_ref() } {
            Some(term) => term,
            None => return $default,
        }
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_is_headless(term: *const AlacTerminal) -> bool {
    term_ref!(term, false).engine.is_headless()
}

/// Process id of the PTY child, or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_child_pid(term: *const AlacTerminal) -> i64 {
    term_ref!(term, -1).engine.child_pid().map_or(-1, i64::from)
}

/// Replace runtime options. Size fields are ignored; use `alac_terminal_resize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_options(
    term: *mut AlacTerminal,
    options: *const AlacOptions,
) {
    let term = term!(term);
    let options = unsafe { options_arg(options) };
    guard((), || term.engine.set_options(options));
}

/// Parse child output in a headless terminal. Returns false for PTY terminals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_feed(
    term: *mut AlacTerminal,
    data: *const u8,
    len: usize,
) -> bool {
    let term = term!(term, false);
    let data = unsafe { bytes_arg(data, len) };
    guard(false, || term.engine.feed(data))
}

/// Send raw bytes to the child process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_write(term: *mut AlacTerminal, data: *const u8, len: usize) {
    let term = term!(term);
    let data = unsafe { bytes_arg(data, len) }.to_vec();
    guard((), || term.engine.write(data));
}

/// Send committed text, such as IME output, as typed input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_text(
    term: *mut AlacTerminal,
    text: *const c_char,
    len: usize,
) {
    let term = term!(term);
    let text = String::from_utf8_lossy(unsafe { bytes_arg(text.cast(), len) }).into_owned();
    guard((), || term.engine.text(&text));
}

/// Paste text, using bracketed paste when enabled by the application.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_paste(
    term: *mut AlacTerminal,
    text: *const c_char,
    len: usize,
) {
    let term = term!(term);
    let text = String::from_utf8_lossy(unsafe { bytes_arg(text.cast(), len) }).into_owned();
    guard((), || term.engine.paste(&text));
}

pub const ALAC_CLIPBOARD: u32 = 0;
pub const ALAC_CLIPBOARD_PRIMARY: u32 = 1;

fn location_arg(location: u32) -> TerminalClipboardLocation {
    match location {
        ALAC_CLIPBOARD_PRIMARY => TerminalClipboardLocation::Primary,
        _ => TerminalClipboardLocation::Clipboard,
    }
}

fn location_value(location: TerminalClipboardLocation) -> u32 {
    match location {
        TerminalClipboardLocation::Clipboard => ALAC_CLIPBOARD,
        TerminalClipboardLocation::Primary => ALAC_CLIPBOARD_PRIMARY,
    }
}

/// Paste from the clipboard callbacks, sending Kitty paste events when requested.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_paste_clipboard(
    term: *mut AlacTerminal,
    location: u32,
) -> bool {
    let term = term!(term, false);
    guard(false, || term.engine.paste_clipboard(location_arg(location)))
}

pub const ALAC_KEY_PRESS: u32 = 0;
pub const ALAC_KEY_REPEAT: u32 = 1;
pub const ALAC_KEY_RELEASE: u32 = 2;

pub const ALAC_LOCATION_STANDARD: u32 = 0;
pub const ALAC_LOCATION_LEFT: u32 = 1;
pub const ALAC_LOCATION_RIGHT: u32 = 2;
pub const ALAC_LOCATION_NUMPAD: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacKeyEvent {
    pub action: u32,
    pub key: u32,
    pub codepoint: u32,
    pub unshifted_codepoint: u32,
    pub text: *const c_char,
    pub location: u32,
    pub mods: u32,
    /// Alt or Option is composing `text` (like macOS Option producing `ƒ`), so it must not be
    /// sent with an escape prefix.
    pub alt_composes: bool,
}

const NAMED_KEYS: [NamedKey; 39] = [
    NamedKey::Enter,
    NamedKey::Tab,
    NamedKey::Backspace,
    NamedKey::Escape,
    NamedKey::Space,
    NamedKey::Insert,
    NamedKey::Delete,
    NamedKey::Home,
    NamedKey::End,
    NamedKey::PageUp,
    NamedKey::PageDown,
    NamedKey::ArrowUp,
    NamedKey::ArrowDown,
    NamedKey::ArrowLeft,
    NamedKey::ArrowRight,
    NamedKey::CapsLock,
    NamedKey::ScrollLock,
    NamedKey::NumLock,
    NamedKey::PrintScreen,
    NamedKey::Pause,
    NamedKey::ContextMenu,
    NamedKey::Shift,
    NamedKey::Control,
    NamedKey::Alt,
    NamedKey::Super,
    NamedKey::Hyper,
    NamedKey::Meta,
    NamedKey::MediaPlay,
    NamedKey::MediaPause,
    NamedKey::MediaPlayPause,
    NamedKey::MediaStop,
    NamedKey::MediaFastForward,
    NamedKey::MediaRewind,
    NamedKey::MediaTrackNext,
    NamedKey::MediaTrackPrevious,
    NamedKey::MediaRecord,
    NamedKey::AudioVolumeDown,
    NamedKey::AudioVolumeUp,
    NamedKey::AudioVolumeMute,
];

/// First function key value; `ALAC_KEY_F1 + n - 1` is key Fn.
pub const ALAC_KEY_F1: u32 = 101;

unsafe fn key_event_arg<'a>(event: &'a AlacKeyEvent) -> Option<KeyEvent<'a>> {
    let key = match event.key {
        0 => Key::Character(char::from_u32(event.codepoint)?),
        key @ 1..=39 => Key::Named(NAMED_KEYS[key as usize - 1]),
        key @ ALAC_KEY_F1..=135 => Key::Named(NamedKey::F((key - ALAC_KEY_F1 + 1) as u8)),
        _ => return None,
    };
    Some(KeyEvent {
        action: match event.action {
            ALAC_KEY_REPEAT => KeyAction::Repeat,
            ALAC_KEY_RELEASE => KeyAction::Release,
            _ => KeyAction::Press,
        },
        key,
        unshifted: char::from_u32(event.unshifted_codepoint).filter(|&c| c != '\0'),
        text: unsafe { str_arg(event.text) },
        location: match event.location {
            ALAC_LOCATION_LEFT => KeyLocation::Left,
            ALAC_LOCATION_RIGHT => KeyLocation::Right,
            ALAC_LOCATION_NUMPAD => KeyLocation::Numpad,
            _ => KeyLocation::Standard,
        },
        mods: Modifiers::from_bits_truncate(event.mods),
        alt_composes: event.alt_composes,
    })
}

/// Encode and send a key event. Returns whether the key was used.
///
/// In vi mode keys move the vi cursor instead; unused keys like `/` are left to the host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_key(
    term: *mut AlacTerminal,
    event: *const AlacKeyEvent,
) -> bool {
    let term = term!(term, false);
    let Some(event) = (unsafe { event.as_ref() }) else { return false };
    let Some(event) = (unsafe { key_event_arg(event) }) else {
        set_error("invalid key event");
        return false;
    };
    guard(false, || term.engine.key(&event))
}

/// Encode a key event for `mode` without a terminal, including the default bindings for keys
/// like application cursor mode arrows.
///
/// Writes up to `cap - 1` bytes and a NUL terminator, returning the full encoded length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_encode_key(
    event: *const AlacKeyEvent,
    mode: u32,
    out: *mut u8,
    cap: usize,
) -> usize {
    let Some(event) = (unsafe { event.as_ref() }) else { return 0 };
    let Some(event) = (unsafe { key_event_arg(event) }) else { return 0 };
    let bytes = guard(Vec::new(), || {
        keyboard::encode_with_default_bindings(&event, TermMode::from_bits_truncate(mode))
    });
    unsafe { copy_out(&bytes, out, cap) }
}

pub const ALAC_MOUSE_LEFT: u32 = 0;
pub const ALAC_MOUSE_MIDDLE: u32 = 1;
pub const ALAC_MOUSE_RIGHT: u32 = 2;

/// Press or release a mouse button over a viewport cell.
///
/// Returns true when reported to the application; otherwise the host may select text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_mouse_button(
    term: *mut AlacTerminal,
    button: u32,
    pressed: bool,
    column: u32,
    row: u32,
    mods: u32,
) -> bool {
    let term = term!(term, false);
    let button = match button {
        ALAC_MOUSE_LEFT => MouseButton::Left,
        ALAC_MOUSE_MIDDLE => MouseButton::Middle,
        ALAC_MOUSE_RIGHT => MouseButton::Right,
        _ => return false,
    };
    let mods = Modifiers::from_bits_truncate(mods);
    guard(false, || term.engine.mouse_button(button, pressed, column as usize, row as usize, mods))
}

/// Move the mouse over a viewport cell. Returns true when motion was reported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_mouse_move(
    term: *mut AlacTerminal,
    column: u32,
    row: u32,
    mods: u32,
) -> bool {
    let term = term!(term, false);
    let mods = Modifiers::from_bits_truncate(mods);
    guard(false, || term.engine.mouse_move(column as usize, row as usize, mods))
}

pub const ALAC_WHEEL_IGNORED: u32 = 0;
pub const ALAC_WHEEL_REPORTED: u32 = 1;
pub const ALAC_WHEEL_ALTERNATE_SCROLL: u32 = 2;
pub const ALAC_WHEEL_SCROLLED: u32 = 3;

/// Scroll by whole lines and columns; positive values scroll up and left.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_mouse_wheel(
    term: *mut AlacTerminal,
    lines: i32,
    columns: i32,
    column: u32,
    row: u32,
    mods: u32,
) -> u32 {
    use crate::engine::WheelAction;

    let term = term!(term, ALAC_WHEEL_IGNORED);
    let mods = Modifiers::from_bits_truncate(mods);
    let action = guard(WheelAction::Ignored, || {
        term.engine.mouse_wheel(lines, columns, column as usize, row as usize, mods)
    });
    match action {
        WheelAction::Ignored => ALAC_WHEEL_IGNORED,
        WheelAction::Reported => ALAC_WHEEL_REPORTED,
        WheelAction::AlternateScroll => ALAC_WHEEL_ALTERNATE_SCROLL,
        WheelAction::Scrolled => ALAC_WHEEL_SCROLLED,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_resize(
    term: *mut AlacTerminal,
    columns: u32,
    rows: u32,
    cell_width: f32,
    cell_height: f32,
) {
    let term = term!(term);
    guard((), || term.engine.resize(columns as usize, rows as usize, cell_width, cell_height));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_focused(term: *mut AlacTerminal, focused: bool) {
    let term = term!(term);
    guard((), || term.engine.set_focused(focused));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_visible(term: *mut AlacTerminal, visible: bool) {
    term!(term).engine.set_visible(visible);
}

/// Active `ALAC_MODE_*` flags.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_mode(term: *const AlacTerminal) -> u32 {
    let term = term_ref!(term, 0);
    guard(0, || term.engine.mode().bits())
}

pub const ALAC_SCROLL_DELTA: u32 = 0;
pub const ALAC_SCROLL_PAGE_UP: u32 = 1;
pub const ALAC_SCROLL_PAGE_DOWN: u32 = 2;
pub const ALAC_SCROLL_TOP: u32 = 3;
pub const ALAC_SCROLL_BOTTOM: u32 = 4;

/// Scroll the viewport; positive deltas move up into history.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_scroll(term: *mut AlacTerminal, kind: u32, delta: i32) {
    let term = term!(term);
    let scroll = match kind {
        ALAC_SCROLL_PAGE_UP => Scroll::PageUp,
        ALAC_SCROLL_PAGE_DOWN => Scroll::PageDown,
        ALAC_SCROLL_TOP => Scroll::Top,
        ALAC_SCROLL_BOTTOM => Scroll::Bottom,
        _ => Scroll::Delta(delta),
    };
    guard((), || term.engine.scroll(scroll));
}

/// Scroll to the previous (`direction < 0`) or next shell prompt.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_jump_to_prompt(term: *mut AlacTerminal, direction: i32) {
    let term = term!(term);
    let direction = if direction < 0 { Direction::Left } else { Direction::Right };
    guard((), || term.engine.jump_to_prompt(direction));
}

pub const ALAC_EVENT_NONE: u32 = 0;
pub const ALAC_EVENT_WAKEUP: u32 = 1;
pub const ALAC_EVENT_TITLE: u32 = 2;
pub const ALAC_EVENT_BELL: u32 = 3;
pub const ALAC_EVENT_CURSOR_BLINKING_CHANGE: u32 = 4;
pub const ALAC_EVENT_MOUSE_CURSOR_DIRTY: u32 = 5;
pub const ALAC_EVENT_PROGRAM_STATUS_CHANGED: u32 = 6;
pub const ALAC_EVENT_PROGRESS_CHANGED: u32 = 7;
pub const ALAC_EVENT_NOTIFICATION_SHOW: u32 = 8;
pub const ALAC_EVENT_NOTIFICATION_CLOSE: u32 = 9;
pub const ALAC_EVENT_NOTIFICATION_QUERY_ALIVE: u32 = 10;
pub const ALAC_EVENT_OUTPUT: u32 = 11;
pub const ALAC_EVENT_CHILD_EXIT: u32 = 12;
pub const ALAC_EVENT_EXIT: u32 = 13;
pub const ALAC_EVENT_WORKING_DIRECTORY_CHANGED: u32 = 14;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AlacProgress {
    pub state: u32,
    pub percent: u32,
}

impl From<Progress> for AlacProgress {
    fn from(progress: Progress) -> Self {
        let state = match progress.state {
            ProgressState::Hidden => 0,
            ProgressState::Normal => 1,
            ProgressState::Error => 2,
            ProgressState::Indeterminate => 3,
            ProgressState::Paused => 4,
        };
        Self { state, percent: u32::from(progress.percent) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacNotification {
    pub serial: u64,
    pub id: *const c_char,
    pub title: *const c_char,
    pub body: *const c_char,
    pub sound: *const c_char,
    pub application: *const c_char,
    pub buttons: *const *const c_char,
    pub button_count: usize,
    pub icons: *const *const c_char,
    pub icon_count: usize,
    pub urgency: u32,
    pub occasion: u32,
    pub expires_in_ms: i64,
    pub focus: bool,
    pub report: bool,
    pub report_close: bool,
    pub persistent: bool,
}

impl NotificationStorage {
    fn new(notification: &Notification) -> Self {
        let mut strings = Vec::new();
        let mut push = |value: &str| {
            let value = c_string(value);
            let pointer = value.as_ptr();
            strings.push(value);
            pointer
        };
        let id = push(&notification.id);
        let title = push(&notification.title);
        let body = push(&notification.body);
        let sound = push(&notification.sound);
        let application = push(&notification.application);
        let buttons: Vec<_> = notification.buttons.iter().map(|button| push(button)).collect();
        let icons: Vec<_> = notification.icons.iter().map(|icon| push(icon)).collect();
        let expires_in_ms = notification.expires.map_or(-1, |expires| {
            expires.saturating_duration_since(Instant::now()).as_millis() as i64
        });
        let value = AlacNotification {
            serial: notification.serial,
            id,
            title,
            body,
            sound,
            application,
            buttons: buttons.as_ptr(),
            button_count: buttons.len(),
            icons: icons.as_ptr(),
            icon_count: icons.len(),
            urgency: u32::from(notification.urgency),
            occasion: match notification.occasion {
                Occasion::Always => 0,
                Occasion::Unfocused => 1,
                Occasion::Invisible => 2,
            },
            expires_in_ms,
            focus: notification.focus,
            report: notification.report,
            report_close: notification.report_close,
            persistent: notification.persistent,
        };
        Self { _strings: strings, _buttons: buttons, _icons: icons, value }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacEvent {
    pub kind: u32,
    /// Exit code for `ALAC_EVENT_CHILD_EXIT`, or -1 when unknown.
    pub exit_code: i32,
    /// Notification serial for show and close events.
    pub serial: u64,
    /// Output bytes, or NUL-terminated title and query text. Null for a reset title.
    pub data: *const u8,
    pub len: usize,
    pub progress: AlacProgress,
    pub notification: *const AlacNotification,
}

impl Default for AlacEvent {
    fn default() -> Self {
        Self {
            kind: ALAC_EVENT_NONE,
            exit_code: -1,
            serial: 0,
            data: ptr::null(),
            len: 0,
            progress: Default::default(),
            notification: ptr::null(),
        }
    }
}

impl EventStorage {
    fn text(&mut self, event: &mut AlacEvent, text: &str) {
        self.data.clear();
        self.data.extend(text.bytes().filter(|&byte| byte != 0));
        event.len = self.data.len();
        self.data.push(0);
        event.data = self.data.as_ptr();
    }

    fn fill(&mut self, host_event: HostEvent) -> AlacEvent {
        self.notification = None;
        let mut event = AlacEvent::default();
        event.kind = match host_event {
            HostEvent::Wakeup => ALAC_EVENT_WAKEUP,
            HostEvent::Title(title) => {
                if let Some(title) = title {
                    self.text(&mut event, &title);
                }
                ALAC_EVENT_TITLE
            },
            HostEvent::Bell => ALAC_EVENT_BELL,
            HostEvent::CursorBlinkingChange => ALAC_EVENT_CURSOR_BLINKING_CHANGE,
            HostEvent::MouseCursorDirty => ALAC_EVENT_MOUSE_CURSOR_DIRTY,
            HostEvent::ProgramStatusChanged => ALAC_EVENT_PROGRAM_STATUS_CHANGED,
            HostEvent::ProgressChanged(progress) => {
                event.progress = progress.into();
                ALAC_EVENT_PROGRESS_CHANGED
            },
            HostEvent::NotificationShow(notification) => {
                let storage = self.notification.insert(NotificationStorage::new(&notification));
                event.serial = notification.serial;
                event.notification = &storage.value;
                ALAC_EVENT_NOTIFICATION_SHOW
            },
            HostEvent::NotificationClose(serial) => {
                event.serial = serial;
                ALAC_EVENT_NOTIFICATION_CLOSE
            },
            HostEvent::NotificationQueryAlive(query) => {
                self.text(&mut event, &query);
                ALAC_EVENT_NOTIFICATION_QUERY_ALIVE
            },
            HostEvent::Output(bytes) => {
                self.data = bytes;
                event.data = self.data.as_ptr();
                event.len = self.data.len();
                ALAC_EVENT_OUTPUT
            },
            HostEvent::ChildExit(code) => {
                event.exit_code = code.unwrap_or(-1);
                ALAC_EVENT_CHILD_EXIT
            },
            HostEvent::Exit => ALAC_EVENT_EXIT,
            HostEvent::WorkingDirectoryChanged => ALAC_EVENT_WORKING_DIRECTORY_CHANGED,
        };
        event
    }
}

/// Take the next event. Returns false when none are pending.
///
/// Protocol replies, clipboard requests and notifications are serviced during this call, so
/// hosts should drain events whenever the wakeup callback fires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_poll_event(
    term: *mut AlacTerminal,
    out: *mut AlacEvent,
) -> bool {
    let term = term!(term, false);
    let Some(out) = (unsafe { out.as_mut() }) else { return false };
    guard(false, || match term.engine.poll() {
        Some(event) => {
            *out = term.event.fill(event);
            true
        },
        None => {
            *out = AlacEvent::default();
            false
        },
    })
}

/// Milliseconds until `alac_terminal_tick` should run, or -1 without pending timers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_next_timeout_ms(term: *const AlacTerminal) -> i64 {
    let term = term_ref!(term, -1);
    guard(-1, || {
        term.engine.next_deadline().map_or(-1, |deadline| {
            // Round up so hosts don't wake before the deadline.
            let remaining = deadline.saturating_duration_since(Instant::now()).as_nanos();
            remaining.div_ceil(1_000_000).min(i64::MAX as u128) as i64
        })
    })
}

/// Run expired timers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_tick(term: *mut AlacTerminal) {
    let term = term!(term);
    guard((), || term.engine.tick());
}

/// The user activated a notification (`button < 0`) or one of its buttons.
///
/// Returns true when the host should focus this terminal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_notification_activated(
    term: *mut AlacTerminal,
    serial: u64,
    button: i32,
) -> bool {
    let term = term!(term, false);
    let button = usize::try_from(button).ok();
    guard(false, || term.engine.notification_activated(serial, button))
}

/// A notification was dismissed or failed to show.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_notification_closed(term: *mut AlacTerminal, serial: u64) {
    let term = term!(term);
    guard((), || term.engine.notification_closed(serial));
}

/// Answer `ALAC_EVENT_NOTIFICATION_QUERY_ALIVE` with the serials still shown.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_notifications_alive(
    term: *mut AlacTerminal,
    query: *const c_char,
    serials: *const u64,
    count: usize,
) {
    let term = term!(term);
    let query = unsafe { str_arg(query) }.unwrap_or_default().to_owned();
    let serials = if serials.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(serials, count) }.to_vec()
    };
    guard((), || term.engine.notifications_alive(&query, &serials));
}

pub const ALAC_COLOR_COUNT: u32 = COUNT as u32;

/// Set a palette color as `0xRRGGBB`. Indices follow `ALAC_COLOR_*`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_color(term: *mut AlacTerminal, index: u32, rgb: u32) {
    let term = term!(term);
    let mut palette = *term.engine.palette();
    if let Some(color) = palette.0.get_mut(index as usize) {
        *color = unpack(rgb);
        term.engine.set_palette(palette);
    }
}

/// Set `count` palette colors starting at index 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_colors(
    term: *mut AlacTerminal,
    colors: *const u32,
    count: usize,
) {
    let term = term!(term);
    if colors.is_null() {
        return;
    }
    let colors = unsafe { std::slice::from_raw_parts(colors, count.min(COUNT)) };
    let mut palette = *term.engine.palette();
    for (slot, &rgb) in palette.0.iter_mut().zip(colors) {
        *slot = unpack(rgb);
    }
    term.engine.set_palette(palette);
}

/// Set foreground and background, deriving cursor, dim and bright foreground colors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_primary_colors(
    term: *mut AlacTerminal,
    foreground: u32,
    background: u32,
) {
    let term = term!(term);
    let mut palette = *term.engine.palette();
    palette.set_primary(unpack(foreground), unpack(background));
    term.engine.set_palette(palette);
}

/// Effective color at `index`, including application overrides from OSC 4/10/11/12.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_color(term: *const AlacTerminal, index: u32) -> u32 {
    let term = term_ref!(term, 0);
    if index as usize >= COUNT {
        return 0;
    }
    guard(0, || pack(term.engine.color(index as usize)))
}

/// Collects clipboard formats or data from host callbacks.
#[derive(Default)]
pub struct AlacClipboardSink {
    formats: Vec<String>,
    data: Option<Vec<u8>>,
}

/// Add an available MIME type from a `formats` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_clipboard_sink_add_format(
    sink: *mut AlacClipboardSink,
    mime_type: *const c_char,
) {
    let Some(sink) = (unsafe { sink.as_mut() }) else { return };
    if let Some(mime_type) = unsafe { str_arg(mime_type) } {
        sink.formats.push(mime_type.into());
    }
}

/// Provide the requested data from a `read` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_clipboard_sink_set_data(
    sink: *mut AlacClipboardSink,
    data: *const u8,
    len: usize,
) {
    let Some(sink) = (unsafe { sink.as_mut() }) else { return };
    sink.data = Some(unsafe { bytes_arg(data, len) }.to_vec());
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacClipboardItem {
    pub mime_type: *const c_char,
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacClipboardCallbacks {
    pub userdata: *mut c_void,
    pub formats: Option<
        unsafe extern "C" fn(*mut c_void, location: u32, sink: *mut AlacClipboardSink) -> bool,
    >,
    pub read: Option<
        unsafe extern "C" fn(
            *mut c_void,
            location: u32,
            mime_type: *const c_char,
            sink: *mut AlacClipboardSink,
        ) -> bool,
    >,
    pub write: Option<
        unsafe extern "C" fn(
            *mut c_void,
            location: u32,
            items: *const AlacClipboardItem,
            count: usize,
        ) -> bool,
    >,
}

struct CallbackClipboard(AlacClipboardCallbacks);

impl ClipboardProvider for CallbackClipboard {
    fn formats(&mut self, location: TerminalClipboardLocation) -> Option<Vec<String>> {
        let callback = self.0.formats?;
        let mut sink = AlacClipboardSink::default();
        unsafe { callback(self.0.userdata, location_value(location), &mut sink) }
            .then_some(sink.formats)
    }

    fn read(&mut self, location: TerminalClipboardLocation, mime_type: &str) -> Option<Vec<u8>> {
        let callback = self.0.read?;
        let mime_type = c_string(mime_type);
        let mut sink = AlacClipboardSink::default();
        let ok = unsafe {
            callback(self.0.userdata, location_value(location), mime_type.as_ptr(), &mut sink)
        };
        if ok { sink.data } else { None }
    }

    fn write(
        &mut self,
        location: TerminalClipboardLocation,
        contents: &[TerminalClipboardContent],
    ) -> bool {
        let Some(callback) = self.0.write else { return false };
        let mime_types: Vec<_> =
            contents.iter().map(|content| c_string(&content.mime_type)).collect();
        let items: Vec<_> = contents
            .iter()
            .zip(&mime_types)
            .map(|(content, mime_type)| AlacClipboardItem {
                mime_type: mime_type.as_ptr(),
                data: content.data.as_ptr(),
                len: content.data.len(),
            })
            .collect();
        unsafe { callback(self.0.userdata, location_value(location), items.as_ptr(), items.len()) }
    }
}

/// Install clipboard callbacks for OSC 52, Kitty clipboard and paste. Pass null to remove them.
///
/// Callbacks run on the thread calling `alac_terminal_poll_event` or a paste function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_clipboard(
    term: *mut AlacTerminal,
    callbacks: *const AlacClipboardCallbacks,
) {
    let term = term!(term);
    let provider = unsafe { callbacks.as_ref() }
        .map(|callbacks| Box::new(CallbackClipboard(*callbacks)) as Box<dyn ClipboardProvider>);
    term.engine.set_clipboard(provider);
}

/// Opaque snapshot of visible terminal content.
pub struct AlacSnapshot(Snapshot);

pub type AlacCell = Cell;
pub type AlacCursor = Cursor;
pub type AlacLineDamage = LineDamage;
pub type AlacImage = Image;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacSnapshotInfo {
    pub columns: u32,
    pub rows: u32,
    pub display_offset: u32,
    pub history_size: u32,
    pub mode: u32,
    pub foreground: u32,
    pub background: u32,
    pub full_damage: bool,
    pub cursor: AlacCursor,
    pub cells: *const AlacCell,
    pub codepoints: *const u32,
    pub codepoint_count: usize,
    pub damage: *const AlacLineDamage,
    pub damage_count: usize,
    pub images: *const AlacImage,
    pub image_count: usize,
}

/// Capture visible content, cursor, damage since the last snapshot and image placements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_snapshot(term: *mut AlacTerminal) -> *mut AlacSnapshot {
    let term = term!(term, ptr::null_mut());
    guard(ptr::null_mut(), || Box::into_raw(Box::new(AlacSnapshot(term.engine.snapshot()))))
}

/// Empty snapshot for `alac_terminal_snapshot_update`.
#[unsafe(no_mangle)]
pub extern "C" fn alac_snapshot_new() -> *mut AlacSnapshot {
    Box::into_raw(Box::new(AlacSnapshot(Snapshot::default())))
}

/// Refresh a snapshot in place, reusing its memory.
///
/// Only damaged rows are copied when the snapshot was the last one updated for this terminal.
/// Pointers from an earlier `alac_snapshot_info` are invalidated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_snapshot_update(
    term: *mut AlacTerminal,
    snapshot: *mut AlacSnapshot,
) -> bool {
    let term = term!(term, false);
    let Some(snapshot) = (unsafe { snapshot.as_mut() }) else { return false };
    guard(false, || {
        term.engine.update_snapshot(&mut snapshot.0);
        true
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_snapshot_free(snapshot: *mut AlacSnapshot) {
    if !snapshot.is_null() {
        drop(unsafe { Box::from_raw(snapshot) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_snapshot_info(
    snapshot: *const AlacSnapshot,
    out: *mut AlacSnapshotInfo,
) {
    let (Some(snapshot), Some(out)) = (unsafe { snapshot.as_ref() }, unsafe { out.as_mut() })
    else {
        return;
    };
    let snapshot = &snapshot.0;
    *out = AlacSnapshotInfo {
        columns: snapshot.columns as u32,
        rows: snapshot.rows as u32,
        display_offset: snapshot.display_offset as u32,
        history_size: snapshot.history_size as u32,
        mode: snapshot.mode,
        foreground: snapshot.foreground,
        background: snapshot.background,
        full_damage: snapshot.full_damage,
        cursor: snapshot.cursor,
        cells: snapshot.cells.as_ptr(),
        codepoints: snapshot.zerowidth.as_ptr(),
        codepoint_count: snapshot.zerowidth.len(),
        damage: snapshot.damage.as_ptr(),
        damage_count: snapshot.damage.len(),
        images: snapshot.images.as_ptr(),
        image_count: snapshot.images.len(),
    };
}

/// UTF-8 text of a cell including combining characters, returning its full byte length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_snapshot_cell_text(
    snapshot: *const AlacSnapshot,
    column: u32,
    row: u32,
    out: *mut c_char,
    cap: usize,
) -> usize {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else { return 0 };
    let Some(cell) = snapshot.0.cell(column as usize, row as usize) else { return 0 };
    let text = snapshot.0.cell_text(cell);
    unsafe { copy_out(text.as_bytes(), out.cast(), cap) }
}

/// URI of a one-based hyperlink index from `AlacCell.hyperlink`, valid while the snapshot lives.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_snapshot_hyperlink_uri(
    snapshot: *const AlacSnapshot,
    index: u32,
) -> *const c_char {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else { return ptr::null() };
    let link = (index as usize).checked_sub(1).and_then(|index| snapshot.0.hyperlinks.get(index));
    link.map_or(ptr::null(), |link| link.uri.as_ptr())
}

/// Identifier of a one-based hyperlink index; cells of one link share it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_snapshot_hyperlink_id(
    snapshot: *const AlacSnapshot,
    index: u32,
) -> *const c_char {
    let Some(snapshot) = (unsafe { snapshot.as_ref() }) else { return ptr::null() };
    let link = (index as usize).checked_sub(1).and_then(|index| snapshot.0.hyperlinks.get(index));
    link.map_or(ptr::null(), |link| link.id.as_ptr())
}

/// Text of the visible screen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_screen_text(term: *const AlacTerminal) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(Some(term.engine.screen_text())))
}

/// Text between grid lines; negative lines are history and 0 is the top of the screen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_text_range(
    term: *const AlacTerminal,
    start_line: i32,
    end_line: i32,
) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(Some(term.engine.text_range(start_line, end_line))))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_history_size(term: *const AlacTerminal) -> u32 {
    let term = term_ref!(term, 0);
    guard(0, || term.engine.history_size() as u32)
}

pub const ALAC_SELECTION_SIMPLE: u32 = 0;
pub const ALAC_SELECTION_BLOCK: u32 = 1;
pub const ALAC_SELECTION_SEMANTIC: u32 = 2;
pub const ALAC_SELECTION_LINES: u32 = 3;

pub const ALAC_SIDE_LEFT: u32 = 0;
pub const ALAC_SIDE_RIGHT: u32 = 1;

fn side_arg(side: u32) -> Direction {
    if side == ALAC_SIDE_RIGHT { Direction::Right } else { Direction::Left }
}

fn selection_arg(kind: u32) -> SelectionType {
    match kind {
        ALAC_SELECTION_BLOCK => SelectionType::Block,
        ALAC_SELECTION_SEMANTIC => SelectionType::Semantic,
        ALAC_SELECTION_LINES => SelectionType::Lines,
        _ => SelectionType::Simple,
    }
}

/// Start a selection at a viewport cell; `side` is the half of the cell under the pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_select_start(
    term: *mut AlacTerminal,
    kind: u32,
    column: u32,
    row: u32,
    side: u32,
) {
    let term = term!(term);
    let kind = selection_arg(kind);
    guard((), || term.engine.select_start(kind, column as usize, row as usize, side_arg(side)));
}

/// Start a selection for a click: one click selects cells, two a word and three whole lines.
/// `block` makes single clicks start a rectangular selection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_select_click(
    term: *mut AlacTerminal,
    column: u32,
    row: u32,
    side: u32,
    clicks: u32,
    block: bool,
) {
    let term = term!(term);
    guard((), || {
        term.engine.select_click(column as usize, row as usize, side_arg(side), clicks, block)
    });
}

/// Viewport cell and side under a pixel position relative to the top-left of the grid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_cell_at(
    term: *const AlacTerminal,
    x: f32,
    y: f32,
    column: *mut u32,
    row: *mut u32,
    side: *mut u32,
) {
    let term = term_ref!(term);
    let (cell_column, cell_row, cell_side) =
        guard((0, 0, Direction::Left), || term.engine.cell_at(x, y));
    unsafe {
        if let Some(column) = column.as_mut() {
            *column = cell_column as u32;
        }
        if let Some(row) = row.as_mut() {
            *row = cell_row as u32;
        }
        if let Some(side) = side.as_mut() {
            *side = if cell_side == Direction::Right { ALAC_SIDE_RIGHT } else { ALAC_SIDE_LEFT };
        }
    }
}

/// Select the entire buffer, including history.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_select_all(term: *mut AlacTerminal) {
    let term = term!(term);
    guard((), || term.engine.select_all());
}

/// Copy the selection with the clipboard callbacks. Returns false without a selection,
/// clipboard callbacks or when the write failed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_copy_selection(
    term: *mut AlacTerminal,
    location: u32,
) -> bool {
    let term = term!(term, false);
    guard(false, || term.engine.copy_selection(location_arg(location)))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_select_update(
    term: *mut AlacTerminal,
    column: u32,
    row: u32,
    side: u32,
) {
    let term = term!(term);
    guard((), || term.engine.select_update(column as usize, row as usize, side_arg(side)));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_select_clear(term: *mut AlacTerminal) {
    let term = term!(term);
    guard((), || term.engine.select_clear());
}

/// Selected text, or null without a selection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_selection_text(term: *const AlacTerminal) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(term.engine.selection_text()))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AlacMatch {
    pub start_line: i32,
    pub start_column: u32,
    pub end_line: i32,
    pub end_column: u32,
}

/// Find the next regex match toward the bottom (`direction >= 0`) or top, continuing from the
/// previous match. Returns 1 when found, 0 when not found and -1 for invalid patterns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_search(
    term: *mut AlacTerminal,
    pattern: *const c_char,
    direction: i32,
    out: *mut AlacMatch,
) -> i32 {
    let term = term!(term, -1);
    let Some(pattern) = (unsafe { str_arg(pattern) }) else {
        set_error("pattern must be UTF-8");
        return -1;
    };
    let direction = if direction < 0 { Direction::Left } else { Direction::Right };
    guard(-1, || match term.engine.search(pattern, direction) {
        Ok(Some(found)) => {
            if let Some(out) = unsafe { out.as_mut() } {
                *out = (&found).into();
            }
            1
        },
        Ok(None) => 0,
        Err(err) => {
            set_error(err);
            -1
        },
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_search_clear(term: *mut AlacTerminal) {
    term!(term).engine.clear_search();
}

impl From<&Match> for AlacMatch {
    fn from(found: &Match) -> Self {
        Self {
            start_line: found.start().line.0,
            start_column: found.start().column.0 as u32,
            end_line: found.end().line.0,
            end_column: found.end().column.0 as u32,
        }
    }
}

/// URI of the OSC 8 hyperlink or detected URL at a viewport cell, or null.
///
/// Its range is stored in `range` and whether it's an OSC 8 hyperlink in `hyperlink`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_link_at(
    term: *mut AlacTerminal,
    column: u32,
    row: u32,
    range: *mut AlacMatch,
    hyperlink: *mut bool,
) -> *mut c_char {
    let term = term!(term, ptr::null_mut());
    let Some(link) = guard(None, || term.engine.link_at(column as usize, row as usize)) else {
        return ptr::null_mut();
    };
    unsafe {
        if let Some(range) = range.as_mut() {
            *range = (&link.range).into();
        }
        if let Some(hyperlink) = hyperlink.as_mut() {
            *hyperlink = link.hyperlink;
        }
    }
    owned_string(Some(link.uri))
}

/// Regex for URL detection; null restores the default and an empty pattern disables detection.
/// Returns false for invalid patterns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_set_url_pattern(
    term: *mut AlacTerminal,
    pattern: *const c_char,
) -> bool {
    let term = term!(term, false);
    let pattern = if pattern.is_null() {
        None
    } else {
        match unsafe { str_arg(pattern) } {
            Some(pattern) => Some(pattern),
            None => {
                set_error("pattern must be UTF-8");
                return false;
            },
        }
    };
    guard(false, || match term.engine.set_url_pattern(pattern) {
        Ok(()) => true,
        Err(err) => {
            set_error(err);
            false
        },
    })
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacLink {
    pub uri: *const c_char,
    pub range: AlacMatch,
    pub hyperlink: bool,
}

/// Links of `alac_terminal_visible_links`.
pub struct AlacLinkList {
    _uris: Vec<CString>,
    links: Vec<AlacLink>,
}

impl AlacLinkList {
    fn new(links: Vec<Link>) -> Self {
        let uris: Vec<_> = links.iter().map(|link| c_string(&link.uri)).collect();
        let links = links
            .iter()
            .zip(&uris)
            .map(|(link, uri)| AlacLink {
                uri: uri.as_ptr(),
                range: (&link.range).into(),
                hyperlink: link.hyperlink,
            })
            .collect();
        Self { _uris: uris, links }
    }
}

/// Visible OSC 8 hyperlinks and detected URLs in reading order, for keyboard hints.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_visible_links(term: *mut AlacTerminal) -> *mut AlacLinkList {
    let term = term!(term, ptr::null_mut());
    guard(ptr::null_mut(), || {
        Box::into_raw(Box::new(AlacLinkList::new(term.engine.visible_links())))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_link_list_len(list: *const AlacLinkList) -> usize {
    unsafe { list.as_ref() }.map_or(0, |list| list.links.len())
}

/// Link at `index`, valid until the list is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_link_list_get(
    list: *const AlacLinkList,
    index: usize,
) -> *const AlacLink {
    let link = unsafe { list.as_ref() }.and_then(|list| list.links.get(index));
    link.map_or(ptr::null(), |link| link as *const _)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_link_list_free(list: *mut AlacLinkList) {
    if !list.is_null() {
        drop(unsafe { Box::from_raw(list) });
    }
}

/// Enter or leave vi mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_vi_toggle(term: *mut AlacTerminal) {
    let term = term!(term);
    guard((), || term.engine.vi_toggle());
}

pub const ALAC_VI_UP: u32 = 0;
pub const ALAC_VI_DOWN: u32 = 1;
pub const ALAC_VI_LEFT: u32 = 2;
pub const ALAC_VI_RIGHT: u32 = 3;
pub const ALAC_VI_FIRST: u32 = 4;
pub const ALAC_VI_LAST: u32 = 5;
pub const ALAC_VI_FIRST_OCCUPIED: u32 = 6;
pub const ALAC_VI_HIGH: u32 = 7;
pub const ALAC_VI_MIDDLE: u32 = 8;
pub const ALAC_VI_LOW: u32 = 9;
pub const ALAC_VI_SEMANTIC_LEFT: u32 = 10;
pub const ALAC_VI_SEMANTIC_RIGHT: u32 = 11;
pub const ALAC_VI_SEMANTIC_LEFT_END: u32 = 12;
pub const ALAC_VI_SEMANTIC_RIGHT_END: u32 = 13;
pub const ALAC_VI_WORD_LEFT: u32 = 14;
pub const ALAC_VI_WORD_RIGHT: u32 = 15;
pub const ALAC_VI_WORD_LEFT_END: u32 = 16;
pub const ALAC_VI_WORD_RIGHT_END: u32 = 17;
pub const ALAC_VI_BRACKET: u32 = 18;
pub const ALAC_VI_PARAGRAPH_UP: u32 = 19;
pub const ALAC_VI_PARAGRAPH_DOWN: u32 = 20;

const VI_MOTIONS: [ViMotion; 21] = [
    ViMotion::Up,
    ViMotion::Down,
    ViMotion::Left,
    ViMotion::Right,
    ViMotion::First,
    ViMotion::Last,
    ViMotion::FirstOccupied,
    ViMotion::High,
    ViMotion::Middle,
    ViMotion::Low,
    ViMotion::SemanticLeft,
    ViMotion::SemanticRight,
    ViMotion::SemanticLeftEnd,
    ViMotion::SemanticRightEnd,
    ViMotion::WordLeft,
    ViMotion::WordRight,
    ViMotion::WordLeftEnd,
    ViMotion::WordRightEnd,
    ViMotion::Bracket,
    ViMotion::ParagraphUp,
    ViMotion::ParagraphDown,
];

/// Move the vi cursor with an `ALAC_VI_*` motion. Ignored outside vi mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_vi_motion(term: *mut AlacTerminal, motion: u32) {
    let term = term!(term);
    let Some(&motion) = VI_MOTIONS.get(motion as usize) else {
        set_error("invalid vi motion");
        return;
    };
    guard((), || term.engine.vi_motion(motion));
}

/// Toggle an `ALAC_SELECTION_*` selection at the vi cursor, like vi's `v`, `V` and `Ctrl-v`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_vi_select(term: *mut AlacTerminal, kind: u32) {
    let term = term!(term);
    guard((), || term.engine.vi_select(selection_arg(kind)));
}

/// Current window title, or null when unset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_title(term: *const AlacTerminal) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(term.engine.title()))
}

/// Thread-safe handle for writing to a terminal's child process.
pub struct AlacWriter(Writer);

/// Create a writer usable from any thread, including after the terminal is freed.
///
/// For headless terminals, written bytes arrive as `ALAC_EVENT_OUTPUT` events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_writer(term: *const AlacTerminal) -> *mut AlacWriter {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || Box::into_raw(Box::new(AlacWriter(term.engine.writer()))))
}

/// Queue bytes for the child process. Returns false once the terminal is gone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_writer_write(
    writer: *const AlacWriter,
    data: *const u8,
    len: usize,
) -> bool {
    let Some(writer) = (unsafe { writer.as_ref() }) else { return false };
    let bytes = unsafe { bytes_arg(data, len) }.to_vec();
    guard(false, || writer.0.write(bytes))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_writer_free(writer: *mut AlacWriter) {
    if !writer.is_null() {
        drop(unsafe { Box::from_raw(writer) });
    }
}

/// Current OSC 9;4 progress.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_progress(term: *const AlacTerminal, out: *mut AlacProgress) {
    let term = term_ref!(term);
    if let Some(out) = unsafe { out.as_mut() } {
        *out = guard(Default::default(), || term.engine.progress().into());
    }
}

/// OSC 7 working directory path, or null. The reported host is stored in `host` when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_working_directory(
    term: *const AlacTerminal,
    host: *mut *mut c_char,
) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    let directory = guard(None, || term.engine.working_directory());
    if let Some(host_out) = unsafe { host.as_mut() } {
        *host_out = owned_string(directory.as_ref().map(|(host, _)| host.clone()));
    }
    owned_string(directory.map(|(_, path)| path))
}

pub const ALAC_SHELL_UNKNOWN: u32 = 0;
pub const ALAC_SHELL_PROMPT: u32 = 1;
pub const ALAC_SHELL_INPUT: u32 = 2;
pub const ALAC_SHELL_OUTPUT: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AlacShellState {
    pub phase: u32,
    pub has_last_command: bool,
    pub last_command_id: u64,
    /// Exit status of the last command, or -1 when unknown.
    pub last_exit_status: i32,
}

/// OSC 133 shell integration state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_shell_state(
    term: *const AlacTerminal,
    out: *mut AlacShellState,
) {
    let term = term_ref!(term);
    let Some(out) = (unsafe { out.as_mut() }) else { return };
    let info = guard(Default::default(), || term.engine.shell_info());
    *out = AlacShellState {
        phase: match info.phase {
            Phase::Unknown => ALAC_SHELL_UNKNOWN,
            Phase::Prompt => ALAC_SHELL_PROMPT,
            Phase::Input => ALAC_SHELL_INPUT,
            Phase::Output => ALAC_SHELL_OUTPUT,
        },
        has_last_command: info.last_command_id.is_some(),
        last_command_id: info.last_command_id.unwrap_or_default(),
        last_exit_status: info.last_exit_status.map_or(-1, i32::from),
    };
}

/// Command line of the last finished shell command, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_last_command(term: *const AlacTerminal) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(term.engine.shell_info().last_command))
}

/// Output of the last finished shell command, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_last_command_output(
    term: *const AlacTerminal,
) -> *mut c_char {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || owned_string(term.engine.last_command_output()))
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AlacProgramStatus {
    /// Empty for the root record.
    pub id: *const c_char,
    /// Application name, resolved through parent records. Null when unknown.
    pub app: *const c_char,
    pub title: *const c_char,
    pub message: *const c_char,
    /// 0 idle, 1 working, 2 done, 3 blocked, 4 error.
    pub state: u32,
    /// 0 none, 1 permission, 2 question, 3 auth.
    pub blocked_kind: u32,
    /// Percent, or -1 without progress.
    pub progress: i32,
}

/// OSC 7501 program status records.
pub struct AlacProgramStatusList {
    _strings: Vec<CString>,
    records: Vec<AlacProgramStatus>,
}

impl AlacProgramStatusList {
    fn new(statuses: Vec<ProgramStatusInfo>) -> Self {
        let mut strings = Vec::new();
        let mut push = |value: Option<&str>| {
            value.map_or(ptr::null(), |value| {
                let value = c_string(value);
                let pointer = value.as_ptr();
                strings.push(value);
                pointer
            })
        };
        let records = statuses
            .iter()
            .map(|status| {
                let record = &status.record;
                AlacProgramStatus {
                    id: push(Some(&record.id)),
                    app: push(status.app.as_deref()),
                    title: push(record.title.as_deref()),
                    message: push(record.msg.as_deref()),
                    state: match record.state {
                        State::Idle | State::Clear => 0,
                        State::Working => 1,
                        State::Done => 2,
                        State::Blocked => 3,
                        State::Error => 4,
                    },
                    blocked_kind: match record.kind {
                        None => 0,
                        Some(BlockedKind::Permission) => 1,
                        Some(BlockedKind::Question) => 2,
                        Some(BlockedKind::Auth) => 3,
                    },
                    progress: record.progress.map_or(-1, i32::from),
                }
            })
            .collect();
        Self { _strings: strings, records }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_terminal_program_statuses(
    term: *const AlacTerminal,
) -> *mut AlacProgramStatusList {
    let term = term_ref!(term, ptr::null_mut());
    guard(ptr::null_mut(), || {
        Box::into_raw(Box::new(AlacProgramStatusList::new(term.engine.program_statuses())))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_program_status_list_len(list: *const AlacProgramStatusList) -> usize {
    unsafe { list.as_ref() }.map_or(0, |list| list.records.len())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_program_status_list_get(
    list: *const AlacProgramStatusList,
    index: usize,
) -> *const AlacProgramStatus {
    unsafe { list.as_ref() }
        .and_then(|list| list.records.get(index))
        .map_or(ptr::null(), |record| record as *const _)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn alac_program_status_list_free(list: *mut AlacProgramStatusList) {
    if !list.is_null() {
        drop(unsafe { Box::from_raw(list) });
    }
}

// Keep the snapshot flag constants exported for bindings generated from Rust.
pub const ALAC_CELL_SELECTED: u32 = snapshot::CELL_SELECTED;
pub const ALAC_CELL_DEFAULT_BACKGROUND: u32 = snapshot::CELL_DEFAULT_BACKGROUND;
pub const ALAC_CELL_UNDERLINE_COLOR: u32 = snapshot::CELL_UNDERLINE_COLOR;
pub const ALAC_CELL_IMAGE_PLACEHOLDER: u32 = snapshot::CELL_IMAGE_PLACEHOLDER;
