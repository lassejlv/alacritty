//! Safe embedding engine behind the C ABI.
//!
//! An [`Engine`] owns one terminal and either a PTY worker or a headless parser fed by the host.
//! Terminal events are queued from any thread and serviced on the host thread by [`Engine::poll`],
//! which answers protocol requests itself and returns only the events a host must present.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use alacritty_session::events::{EventLoopSender, Msg};
use alacritty_session::sync::FairMutex;
use alacritty_session::{Session, pty};
use alacritty_terminal::clipboard::{
    ClipboardHost, KittyClipboardHostState, MAX_WRITE_BYTES, TerminalClipboardContent,
    TerminalClipboardLocation, TerminalClipboardReadRequest, TerminalClipboardReadResult,
    TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
};
use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::input::keyboard::{self, Key, KeyAction, KeyEvent, Modifiers, NamedKey};
use alacritty_terminal::input::mouse::{self, MouseButton};
use alacritty_terminal::program_status::ProgramStatus;
use alacritty_terminal::protocols::notifications::{Effect, Notification, Notifications};
use alacritty_terminal::protocols::progress::{PROGRESS_TIMEOUT, Progress, ProgressState};
use alacritty_terminal::protocols::shell::Phase;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::hints::{
    HintPostProcessor, URL_REGEX, hyperlink_at, regex_match_at, visible_regex_match_iter,
    visible_unique_hyperlinks_iter,
};
use alacritty_terminal::term::search::{Match, RegexSearch};
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term, TermMode, viewport_to_point};
use alacritty_terminal::vi_mode::ViMotion;
use alacritty_terminal::vte::ansi::{CursorStyle, Processor, Rgb};
use parking_lot::Mutex;

use crate::colors::Palette;
use crate::snapshot::{Snapshot, Update, View};

/// Terminal construction and runtime options.
#[derive(Clone, Debug)]
pub struct Options {
    pub columns: usize,
    pub rows: usize,
    pub cell_width: f32,
    pub cell_height: f32,
    pub scrollback: usize,
    pub kitty_keyboard: bool,
    pub osc52: Osc52,
    pub cursor_style: CursorStyle,
    pub bold_is_bright: bool,
    /// Clear the selection and scroll to the bottom when input is sent.
    pub scroll_on_input: bool,
    pub desktop_notifications: bool,
    pub semantic_escape_chars: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            cell_width: 8.,
            cell_height: 16.,
            scrollback: 10_000,
            kitty_keyboard: true,
            osc52: Osc52::OnlyCopy,
            cursor_style: CursorStyle::default(),
            bold_is_bright: false,
            scroll_on_input: true,
            desktop_notifications: true,
            semantic_escape_chars: None,
        }
    }
}

impl Options {
    fn term_config(&self) -> Config {
        let mut config = Config {
            scrolling_history: self.scrollback,
            default_cursor_style: self.cursor_style,
            kitty_keyboard: self.kitty_keyboard,
            osc52: self.osc52,
            ..Config::default()
        };
        if let Some(chars) = &self.semantic_escape_chars {
            config.semantic_escape_chars = chars.clone();
        }
        config
    }

    fn size(&self) -> TermSize {
        TermSize { columns: self.columns.max(2), lines: self.rows.max(1) }
    }

    fn window_size(&self) -> WindowSize {
        let size = self.size();
        WindowSize {
            num_cols: size.columns.min(u16::MAX as usize) as u16,
            num_lines: size.lines.min(u16::MAX as usize) as u16,
            cell_width: self.cell_width.max(1.) as u16,
            cell_height: self.cell_height.max(1.) as u16,
        }
    }
}

/// Child process options for PTY-backed terminals.
#[derive(Clone, Debug, Default)]
pub struct PtyOptions {
    /// Program to run, or the user's login shell when `None`.
    pub program: Option<String>,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub drain_on_exit: bool,
}

/// Native clipboard access provided by the host.
///
/// The engine enforces focus and OSC 52 permission policy before calling the provider for
/// application requests. User actions like paste and copy are not restricted.
pub trait ClipboardProvider {
    /// MIME types available at `location`, or `None` when the location is unsupported.
    fn formats(&mut self, location: TerminalClipboardLocation) -> Option<Vec<String>>;
    /// Contents of one MIME type.
    fn read(&mut self, location: TerminalClipboardLocation, mime_type: &str) -> Option<Vec<u8>>;
    /// Replace the clipboard with all provided representations.
    fn write(
        &mut self,
        location: TerminalClipboardLocation,
        contents: &[TerminalClipboardContent],
    ) -> bool;
}

/// Events a host presents or acts on.
#[derive(Debug)]
pub enum HostEvent {
    /// New terminal content is available.
    Wakeup,
    /// Window title change, or `None` to restore the host's default title.
    Title(Option<String>),
    Bell,
    CursorBlinkingChange,
    MouseCursorDirty,
    /// OSC 7501 records changed; read them with [`Engine::program_statuses`].
    ProgramStatusChanged,
    /// OSC 9;4 progress changed.
    ProgressChanged(Progress),
    /// An OSC 9 or OSC 99 notification should be shown.
    NotificationShow(Box<Notification>),
    /// A shown notification should be closed.
    NotificationClose(u64),
    /// Report live notifications with [`Engine::notifications_alive`].
    NotificationQueryAlive(String),
    /// Bytes for the child process of a headless terminal.
    Output(Vec<u8>),
    /// The child process exited, with its exit code when known.
    ChildExit(Option<i32>),
    /// The terminal requested shutdown.
    Exit,
    /// The OSC 7 working directory changed; read it with [`Engine::working_directory`].
    WorkingDirectoryChanged,
}

/// Result of a mouse wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelAction {
    Ignored,
    /// Reported to the application through mouse reporting.
    Reported,
    /// Sent as cursor keys because the alternate screen requested alternate scroll.
    AlternateScroll,
    /// The viewport scrolled through history.
    Scrolled,
}

/// Snapshot of shell integration state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellInfo {
    pub phase: Phase,
    pub last_command: Option<String>,
    pub last_command_id: Option<u64>,
    pub last_exit_status: Option<u8>,
}

/// OSC 7501 record with its resolved application name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStatusInfo {
    pub record: ProgramStatus,
    pub app: Option<String>,
}

/// A URL or OSC 8 hyperlink in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub uri: String,
    pub range: Match,
    /// The link is an OSC 8 hyperlink rather than detected text.
    pub hyperlink: bool,
}

#[derive(Copy, Clone, Debug)]
struct TermSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

type Wakeup = Box<dyn Fn() + Send + Sync>;

enum Queued {
    Term(Event),
    /// Bytes from a [`Writer`] of a headless terminal.
    Output(Vec<u8>),
}

struct Shared {
    queue: Mutex<VecDeque<Queued>>,
    /// Cleared when the engine is dropped, since the PTY worker can outlive it.
    wakeup: Mutex<Option<Wakeup>>,
}

impl Shared {
    fn new(wakeup: Option<Wakeup>) -> Arc<Self> {
        Arc::new(Self { queue: Default::default(), wakeup: Mutex::new(wakeup) })
    }

    fn push(&self, item: Queued) {
        let mut queue = self.queue.lock();
        let wakeup = |item: Option<&Queued>| matches!(item, Some(Queued::Term(Event::Wakeup)));
        if wakeup(Some(&item)) && wakeup(queue.back()) {
            return;
        }
        queue.push_back(item);
        drop(queue);
        self.wake();
    }

    fn wake(&self) {
        if let Some(wakeup) = &*self.wakeup.lock() {
            wakeup();
        }
    }
}

/// Terminal event sink queuing events for the host thread.
#[derive(Clone)]
pub struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.push(Queued::Term(event));
    }
}

/// Thread-safe handle sending bytes to a terminal's child process.
///
/// Writers stay valid after the terminal is dropped; writes then fail.
#[derive(Clone)]
pub struct Writer(WriterTarget);

#[derive(Clone)]
enum WriterTarget {
    Pty(EventLoopSender),
    Headless(Weak<Shared>),
}

impl Writer {
    /// Queue bytes for the child. Returns `false` once the terminal is gone.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) -> bool {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return true;
        }
        match &self.0 {
            WriterTarget::Pty(sender) => sender.send(Msg::Input(bytes)).is_ok(),
            WriterTarget::Headless(shared) => match shared.upgrade() {
                Some(shared) => {
                    shared.push(Queued::Output(bytes.into_owned()));
                    true
                },
                None => false,
            },
        }
    }
}

enum Backend {
    Pty(Session<Listener>),
    Headless(Box<Processor>),
}

#[derive(Default)]
struct MouseState {
    buttons: [bool; 3],
    cell: Option<(usize, usize)>,
}

struct SearchState {
    pattern: String,
    regex: RegexSearch,
    last: Option<Match>,
}

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);

/// One embedded terminal.
pub struct Engine {
    term: Arc<FairMutex<Term<Listener>>>,
    backend: Backend,
    shared: Arc<Shared>,
    ready: VecDeque<HostEvent>,
    options: Options,
    palette: Palette,
    focused: bool,
    visible: bool,
    notifications: Notifications,
    kitty_clipboard: KittyClipboardHostState,
    clipboard: Option<Box<dyn ClipboardProvider>>,
    progress_deadline: Option<Instant>,
    animation_deadline: Option<Instant>,
    full_damage: bool,
    /// Identifier of the snapshot updated last, for incremental updates.
    last_snapshot: u64,
    /// View of the last snapshot, which damage is relative to.
    view: Option<View>,
    mouse: MouseState,
    search: Option<SearchState>,
    url_regex: Option<RegexSearch>,
}

impl Engine {
    /// Create a terminal parsing bytes provided through [`Engine::feed`].
    pub fn headless(options: Options, wakeup: Option<Wakeup>) -> Self {
        let shared = Shared::new(wakeup);
        let term = Self::create_term(&options, Listener(shared.clone()));
        let term = Arc::new(FairMutex::new(term));
        Self::new(term, Backend::Headless(Box::default()), shared, options)
    }

    /// Create a terminal running a child process on a new PTY.
    pub fn spawn(
        options: Options,
        pty_options: PtyOptions,
        wakeup: Option<Wakeup>,
    ) -> io::Result<Self> {
        let shared = Shared::new(wakeup);
        let listener = Listener(shared.clone());
        let term = Self::create_term(&options, listener.clone());

        let mut env = pty_options.env;
        env.entry("TERM".into()).or_insert_with(|| "xterm-256color".into());
        env.entry("COLORTERM".into()).or_insert_with(|| "truecolor".into());
        let pty_config = pty::Options {
            shell: pty_options.program.map(|program| pty::Shell::new(program, pty_options.args)),
            working_directory: pty_options.working_directory,
            drain_on_exit: pty_options.drain_on_exit,
            env,
            #[cfg(windows)]
            escape_args: true,
        };

        let window_id = NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed);
        let session =
            Session::new(term, &pty_config, options.window_size(), window_id, listener, false)?;
        let term = session.terminal.clone();
        Ok(Self::new(term, Backend::Pty(session), shared, options))
    }

    fn create_term(options: &Options, listener: Listener) -> Term<Listener> {
        let mut term = Term::new(options.term_config(), &options.size(), listener);
        term.set_graphics_cell_size(options.cell_width, options.cell_height);
        term.is_focused = true;
        term
    }

    fn new(
        term: Arc<FairMutex<Term<Listener>>>,
        backend: Backend,
        shared: Arc<Shared>,
        options: Options,
    ) -> Self {
        Self {
            term,
            backend,
            shared,
            options,
            ready: Default::default(),
            palette: Default::default(),
            focused: true,
            visible: true,
            notifications: Default::default(),
            kitty_clipboard: Default::default(),
            clipboard: None,
            progress_deadline: None,
            animation_deadline: None,
            full_damage: true,
            last_snapshot: 0,
            view: None,
            mouse: Default::default(),
            search: None,
            url_regex: None,
        }
    }

    /// Shared terminal state, for direct use of the `alacritty_terminal` API.
    ///
    /// Don't hold the lock across calls into the engine.
    pub fn terminal(&self) -> &Arc<FairMutex<Term<Listener>>> {
        &self.term
    }

    pub fn is_headless(&self) -> bool {
        matches!(self.backend, Backend::Headless(_))
    }

    /// Process id of the PTY child.
    pub fn child_pid(&self) -> Option<u32> {
        match &self.backend {
            #[cfg(not(windows))]
            Backend::Pty(session) => Some(session.shell_pid),
            _ => None,
        }
    }

    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Replace runtime options. Size changes are ignored; use [`Engine::resize`].
    pub fn set_options(&mut self, mut options: Options) {
        options.columns = self.options.columns;
        options.rows = self.options.rows;
        options.cell_width = self.options.cell_width;
        options.cell_height = self.options.cell_height;
        self.term.lock().set_options(options.term_config());
        if options.osc52 == Osc52::Disabled {
            self.kitty_clipboard.reset();
        }
        self.options = options;
        self.full_damage = true;
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        self.full_damage = true;
    }

    pub fn set_clipboard(&mut self, clipboard: Option<Box<dyn ClipboardProvider>>) {
        self.clipboard = clipboard;
    }

    /// Parse bytes from the child process of a headless terminal.
    ///
    /// Returns `false` for PTY-backed terminals, whose output is parsed by their worker.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        let Backend::Headless(parser) = &mut self.backend else { return false };
        if bytes.is_empty() {
            return true;
        }
        parser.advance(&mut *self.term.lock(), bytes);
        // Queue a redraw unless all processed bytes were synchronized.
        if parser.sync_bytes_count() < bytes.len() {
            self.shared.push(Queued::Term(Event::Wakeup));
        }
        true
    }

    /// Send bytes to the child process.
    pub fn write(&mut self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        match &self.backend {
            Backend::Pty(session) => session.notifier.notify(bytes),
            Backend::Headless(_) => {
                self.ready.push_back(HostEvent::Output(bytes.into_owned()));
                self.shared.wake();
            },
        }
    }

    /// Thread-safe handle for sending bytes to the child process.
    pub fn writer(&self) -> Writer {
        Writer(match &self.backend {
            Backend::Pty(session) => WriterTarget::Pty(session.notifier.0.clone()),
            Backend::Headless(_) => WriterTarget::Headless(Arc::downgrade(&self.shared)),
        })
    }

    /// Next host event, answering queued protocol requests on the way.
    pub fn poll(&mut self) -> Option<HostEvent> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Some(event);
            }
            let item = self.shared.queue.lock().pop_front()?;
            match item {
                Queued::Term(event) => self.process(event),
                Queued::Output(bytes) => self.ready.push_back(HostEvent::Output(bytes)),
            }
        }
    }

    fn process(&mut self, event: Event) {
        match event {
            Event::Wakeup => self.ready.push_back(HostEvent::Wakeup),
            Event::Title(title) => self.ready.push_back(HostEvent::Title(Some(title))),
            Event::ResetTitle => self.ready.push_back(HostEvent::Title(None)),
            Event::Bell => self.ready.push_back(HostEvent::Bell),
            Event::Exit => self.ready.push_back(HostEvent::Exit),
            Event::ChildExit(status) => self.ready.push_back(HostEvent::ChildExit(status.code())),
            Event::CursorBlinkingChange => self.ready.push_back(HostEvent::CursorBlinkingChange),
            Event::MouseCursorDirty => self.ready.push_back(HostEvent::MouseCursorDirty),
            Event::ProgramStatusChanged => self.ready.push_back(HostEvent::ProgramStatusChanged),
            Event::WorkingDirectoryChanged => {
                self.ready.push_back(HostEvent::WorkingDirectoryChanged)
            },
            Event::ProgressChanged => {
                let progress = self.term.lock().progress();
                self.progress_deadline = (progress.state != ProgressState::Hidden)
                    .then(|| Instant::now() + PROGRESS_TIMEOUT);
                self.ready.push_back(HostEvent::ProgressChanged(progress));
            },
            Event::PtyWrite(text) => self.write(text.into_bytes()),
            Event::ColorRequest(index, format) => {
                let color = self.palette.get(self.term.lock().colors(), index);
                self.write(format(color).into_bytes());
            },
            Event::TextAreaSizeRequest(format) => {
                let text = format(self.options.window_size());
                self.write(text.into_bytes());
            },
            Event::ClipboardStore(ty, text) => {
                let Some(clipboard) = self.clipboard.as_mut().filter(|_| self.focused) else {
                    return;
                };
                let content =
                    TerminalClipboardContent { mime_type: "text/plain".into(), data: text.into() };
                clipboard.write(location(ty), &[content]);
            },
            Event::ClipboardLoad(ty, format) => {
                let Some(clipboard) = self.clipboard.as_mut().filter(|_| self.focused) else {
                    return;
                };
                let data = clipboard.read(location(ty), "text/plain").unwrap_or_default();
                let text = format(&String::from_utf8_lossy(&data));
                self.write(text.into_bytes());
            },
            Event::KittyClipboard(osc) => {
                let mut host = Host {
                    clipboard: self.clipboard.as_mut(),
                    permission: self.options.osc52,
                    focused: self.focused,
                };
                let replies = self.kitty_clipboard.handle_osc(osc, &mut host);
                for reply in replies {
                    self.write(reply);
                }
            },
            Event::KittyClipboardMode(enabled) => {
                self.kitty_clipboard.set_paste_events_enabled(enabled)
            },
            Event::KittyClipboardReset => self.kitty_clipboard.reset(),
            Event::DesktopNotification { body, truncated } => {
                let effects = self.notifications.apply(
                    &body,
                    truncated,
                    Instant::now(),
                    self.focused,
                    self.visible,
                    self.options.desktop_notifications,
                );
                self.notification_effects(effects);
            },
            Event::NotificationsReset => {
                let effects = self.notifications.reset();
                self.notification_effects(effects);
            },
        }
    }

    fn notification_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Reply(reply) => self.write(reply.into_bytes()),
                Effect::Show(notification) => {
                    self.ready.push_back(HostEvent::NotificationShow(notification))
                },
                Effect::Close(serial) => self.ready.push_back(HostEvent::NotificationClose(serial)),
                Effect::QueryAlive(query) => {
                    self.ready.push_back(HostEvent::NotificationQueryAlive(query))
                },
            }
        }
    }

    /// The user activated a notification or one of its buttons.
    ///
    /// Returns `true` when the host should focus this terminal.
    pub fn notification_activated(&mut self, serial: u64, button: Option<usize>) -> bool {
        let (focus, effects) = self.notifications.activated(serial, button);
        self.notification_effects(effects);
        focus
    }

    /// A notification was dismissed or failed to show.
    pub fn notification_closed(&mut self, serial: u64) {
        let effects = self.notifications.closed(serial);
        self.notification_effects(effects);
    }

    /// Answer a [`HostEvent::NotificationQueryAlive`] with the serials still shown.
    pub fn notifications_alive(&mut self, query: &str, serials: &[u64]) {
        let reply = self.notifications.alive(query, serials);
        self.write(reply.into_bytes());
    }

    /// Earliest time [`Engine::tick`] has work to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let sync = match &self.backend {
            Backend::Headless(parser) => parser.sync_timeout().sync_timeout(),
            Backend::Pty(_) => None,
        };
        [self.notifications.next_expiry(), self.progress_deadline, self.animation_deadline, sync]
            .into_iter()
            .flatten()
            .min()
    }

    /// Run expired timers: notification expiry, progress timeout, image animation and
    /// synchronized update timeouts.
    pub fn tick(&mut self) {
        let now = Instant::now();

        let effects = self.notifications.expire(now);
        self.notification_effects(effects);

        if self.progress_deadline.is_some_and(|deadline| deadline <= now) {
            self.progress_deadline = None;
            self.term.lock().clear_progress();
        }

        if self.animation_deadline.is_some_and(|deadline| deadline <= now) {
            self.animation_deadline = None;
            self.shared.push(Queued::Term(Event::Wakeup));
        }

        if let Backend::Headless(parser) = &mut self.backend
            && parser.sync_timeout().sync_timeout().is_some_and(|deadline| deadline <= now)
        {
            parser.stop_sync(&mut *self.term.lock());
            self.shared.push(Queued::Term(Event::Wakeup));
        }
    }

    /// Resize the grid and inform the child process.
    pub fn resize(&mut self, columns: usize, rows: usize, cell_width: f32, cell_height: f32) {
        self.options.columns = columns;
        self.options.rows = rows;
        self.options.cell_width = cell_width;
        self.options.cell_height = cell_height;
        let size = self.options.size();
        {
            let mut term = self.term.lock();
            term.resize(size);
            term.set_graphics_cell_size(cell_width, cell_height);
        }
        if let Backend::Pty(session) = &mut self.backend {
            session.notifier.on_resize(self.options.window_size());
        }
        self.mouse.cell = None;
        self.full_damage = true;
    }

    /// Update focus, reporting it when the application requested focus events.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        let report = {
            let mut term = self.term.lock();
            term.is_focused = focused;
            term.mode().contains(TermMode::FOCUS_IN_OUT)
        };
        if report {
            self.write(if focused { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
        }
        self.full_damage = true;
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether the terminal is visible to the user, used for notification policy.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn on_input_start(&mut self) {
        if !self.options.scroll_on_input {
            return;
        }
        let mut term = self.term.lock();
        self.full_damage |= term.selection.take().is_some();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
    }

    /// Encode and send a key event. Returns whether the key was used.
    ///
    /// Keys drive vi mode instead of the child process while it is active.
    pub fn key(&mut self, event: &KeyEvent<'_>) -> bool {
        let mode = self.mode();
        if mode.contains(TermMode::VI) {
            return self.vi_key(event);
        }
        let bytes = keyboard::encode_with_default_bindings(event, mode);
        if bytes.is_empty() {
            return false;
        }
        if event.action != KeyAction::Release && keyboard::starts_input(event) {
            self.on_input_start();
        }
        self.write(bytes);
        true
    }

    /// Send committed text, such as IME output, as typed input.
    pub fn text(&mut self, text: &str) {
        if text.is_empty() || self.mode().contains(TermMode::VI) {
            return;
        }
        self.on_input_start();
        self.write(text.as_bytes().to_vec());
    }

    /// Paste text, using bracketed paste when the application enabled it.
    pub fn paste(&mut self, text: &str) {
        self.on_input_start();
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            self.write(&b"\x1b[200~"[..]);
            // Remove escapes so the paste can't terminate bracketed paste early, and `\x03`
            // since some shells incorrectly terminate bracketed paste on it.
            self.write(text.replace(['\x1b', '\x03'], "").into_bytes());
            self.write(&b"\x1b[201~"[..]);
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
        }
    }

    /// Paste from the host clipboard, notifying Kitty clipboard clients when requested.
    ///
    /// Returns `false` when no clipboard provider is installed.
    pub fn paste_clipboard(&mut self, location: TerminalClipboardLocation) -> bool {
        let Some(clipboard) = self.clipboard.as_mut() else { return false };
        if self.kitty_clipboard.paste_events_enabled() && self.options.osc52 != Osc52::Disabled {
            let formats = clipboard.formats(location).unwrap_or_default();
            if let Some(notification) = self.kitty_clipboard.paste_notification(location, &formats)
            {
                self.on_input_start();
                self.write(notification);
            }
            return true;
        }
        let data = clipboard.read(location, "text/plain").unwrap_or_default();
        self.paste(&String::from_utf8_lossy(&data));
        true
    }

    /// Copy the selection to the host clipboard. Returns `false` without a selection or
    /// clipboard provider.
    pub fn copy_selection(&mut self, location: TerminalClipboardLocation) -> bool {
        let Some(text) = self.selection_text() else { return false };
        let Some(clipboard) = self.clipboard.as_mut() else { return false };
        let content =
            TerminalClipboardContent { mime_type: "text/plain".into(), data: text.into() };
        clipboard.write(location, &[content])
    }

    /// Viewport cell and side under a pixel position relative to the top-left of the grid.
    pub fn cell_at(&self, x: f32, y: f32) -> (usize, usize, Side) {
        let size = self.options.size();
        let cell_width = self.options.cell_width.max(1.);
        let cell_height = self.options.cell_height.max(1.);
        let column = (x.max(0.) / cell_width) as usize;
        let row = (y.max(0.) / cell_height) as usize;
        let offset = x.max(0.) - column as f32 * cell_width;
        let side = if offset > cell_width / 2. { Side::Right } else { Side::Left };
        if column >= size.columns {
            (size.columns - 1, row.min(size.lines - 1), Side::Right)
        } else {
            (column, row.min(size.lines - 1), side)
        }
    }

    /// Press or release a mouse button over a viewport cell.
    ///
    /// Returns `true` when the event was reported to the application; otherwise the host may
    /// use it for selection.
    pub fn mouse_button(
        &mut self,
        button: MouseButton,
        pressed: bool,
        column: usize,
        row: usize,
        mods: Modifiers,
    ) -> bool {
        self.mouse.buttons[button.code() as usize] = pressed;
        let mode = self.mode();
        if !Self::mouse_reporting(mode, mods) {
            return false;
        }
        let (column, row) = self.clamp_cell(column, row);
        self.report_mouse(button.code(), pressed, column, row, mods, mode)
    }

    /// Move the mouse over a viewport cell, reporting motion when requested.
    pub fn mouse_move(&mut self, column: usize, row: usize, mods: Modifiers) -> bool {
        let cell = self.clamp_cell(column, row);
        if self.mouse.cell.replace(cell) == Some(cell) {
            return false;
        }
        let mode = self.mode();
        if !Self::mouse_reporting(mode, mods)
            || !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG)
        {
            return false;
        }
        let code = match self.mouse.buttons {
            [true, ..] => mouse::MOTION + MouseButton::Left.code(),
            [_, true, _] => mouse::MOTION + MouseButton::Middle.code(),
            [.., true] => mouse::MOTION + MouseButton::Right.code(),
            _ if mode.contains(TermMode::MOUSE_MOTION) => mouse::MOTION_NO_BUTTON,
            _ => return false,
        };
        self.report_mouse(code, true, cell.0, cell.1, mods, mode)
    }

    /// Scroll by whole lines and columns; positive values scroll up and left.
    pub fn mouse_wheel(
        &mut self,
        lines: i32,
        columns: i32,
        column: usize,
        row: usize,
        mods: Modifiers,
    ) -> WheelAction {
        if lines == 0 && columns == 0 {
            return WheelAction::Ignored;
        }
        let mode = self.mode();
        let (column, row) = self.clamp_cell(column, row);
        if Self::mouse_reporting(mode, mods) {
            let code = if lines > 0 { mouse::WHEEL_UP } else { mouse::WHEEL_DOWN };
            for _ in 0..lines.unsigned_abs() {
                self.report_mouse(code, true, column, row, mods, mode);
            }
            let code = if columns > 0 { mouse::WHEEL_LEFT } else { mouse::WHEEL_RIGHT };
            for _ in 0..columns.unsigned_abs() {
                self.report_mouse(code, true, column, row, mods, mode);
            }
            WheelAction::Reported
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !mode.contains(TermMode::VI)
            && !mods.contains(Modifiers::SHIFT)
        {
            // The sequences match the respective arrow keys.
            let line_cmd = if lines > 0 { b'A' } else { b'B' };
            let column_cmd = if columns > 0 { b'D' } else { b'C' };
            let mut content = Vec::new();
            for _ in 0..lines.unsigned_abs() {
                content.extend_from_slice(&[0x1b, b'O', line_cmd]);
            }
            for _ in 0..columns.unsigned_abs() {
                content.extend_from_slice(&[0x1b, b'O', column_cmd]);
            }
            self.write(content);
            WheelAction::AlternateScroll
        } else if lines != 0 {
            self.scroll(Scroll::Delta(lines));
            WheelAction::Scrolled
        } else {
            WheelAction::Ignored
        }
    }

    /// Mouse events go to the application unless Shift is held or vi mode is active.
    fn mouse_reporting(mode: TermMode, mods: Modifiers) -> bool {
        mode.intersects(TermMode::MOUSE_MODE)
            && !mode.contains(TermMode::VI)
            && !mods.contains(Modifiers::SHIFT)
    }

    fn report_mouse(
        &mut self,
        code: u8,
        pressed: bool,
        column: usize,
        row: usize,
        mods: Modifiers,
        mode: TermMode,
    ) -> bool {
        match mouse::report(code, pressed, column, row, mods, mode) {
            Some(bytes) => {
                self.write(bytes);
                true
            },
            None => false,
        }
    }

    fn clamp_cell(&self, column: usize, row: usize) -> (usize, usize) {
        let size = self.options.size();
        (column.min(size.columns - 1), row.min(size.lines - 1))
    }

    /// Scroll the viewport through history.
    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
    }

    /// Scroll to the previous (`Left`) or next (`Right`) shell prompt.
    pub fn jump_to_prompt(&mut self, direction: Direction) {
        self.term.lock().jump_to_prompt(direction);
    }

    fn viewport_point(term: &Term<Listener>, column: usize, row: usize) -> Point {
        let row = row.min(term.screen_lines() - 1);
        let column = column.min(term.columns() - 1);
        viewport_to_point(term.grid().display_offset(), Point::new(row, Column(column)))
    }

    /// Start a selection at a viewport cell.
    pub fn select_start(&mut self, ty: SelectionType, column: usize, row: usize, side: Side) {
        let mut term = self.term.lock();
        let point = Self::viewport_point(&term, column, row);
        term.selection = Some(Selection::new(ty, point, side));
        if term.mode().contains(TermMode::VI) {
            term.vi_mode_cursor.point = point;
        }
        self.full_damage = true;
    }

    /// Start a selection for a mouse click: one click selects cells, two select a word and
    /// three select lines. `block` selects a rectangle for single clicks.
    pub fn select_click(
        &mut self,
        column: usize,
        row: usize,
        side: Side,
        clicks: u32,
        block: bool,
    ) {
        let ty = match clicks {
            0 | 1 if block => SelectionType::Block,
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        self.select_start(ty, column, row, side);
    }

    /// Extend the selection to a viewport cell.
    pub fn select_update(&mut self, column: usize, row: usize, side: Side) {
        let mut term = self.term.lock();
        let point = Self::viewport_point(&term, column, row);
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
            self.full_damage = true;
        }
    }

    /// Select the entire buffer, including history.
    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        term.selection = Some(selection);
        self.full_damage = true;
    }

    pub fn select_clear(&mut self) {
        self.full_damage |= self.term.lock().selection.take().is_some();
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    /// Text of the visible screen.
    pub fn screen_text(&self) -> String {
        let term = self.term.lock();
        let top = -(term.grid().display_offset() as i32);
        let bottom = top + term.screen_lines() as i32 - 1;
        Self::lines_text(&term, top, bottom)
    }

    /// Text between grid lines, where negative lines are in history and `0` is the top of the
    /// screen. Out of range lines are clamped.
    pub fn text_range(&self, start: i32, end: i32) -> String {
        Self::lines_text(&self.term.lock(), start, end)
    }

    fn lines_text(term: &Term<Listener>, start: i32, end: i32) -> String {
        let start = start.max(term.topmost_line().0);
        let end = end.min(term.bottommost_line().0);
        if start > end {
            return String::new();
        }
        term.bounds_to_string(
            Point::new(Line(start), Column(0)),
            Point::new(Line(end), term.last_column()),
        )
    }

    pub fn history_size(&self) -> usize {
        self.term.lock().history_size()
    }

    /// Find the next regex match, continuing from the previous match of the same pattern.
    ///
    /// `Right` searches toward the bottom and `Left` toward the top, wrapping around. The match
    /// is selected and scrolled into view; in vi mode the vi cursor moves to it.
    pub fn search(&mut self, pattern: &str, direction: Direction) -> Result<Option<Match>, String> {
        if self.search.as_ref().is_none_or(|search| search.pattern != pattern) {
            let regex = RegexSearch::new(pattern).map_err(|err| err.to_string())?;
            self.search = Some(SearchState { pattern: pattern.into(), regex, last: None });
        }
        let search = self.search.as_mut().unwrap();
        let mut term = self.term.lock();
        let origin = match (&search.last, direction) {
            (Some(last), Direction::Right) => last.end().add(&*term, Boundary::None, 1),
            (Some(last), Direction::Left) => last.start().sub(&*term, Boundary::None, 1),
            (None, Direction::Right) => Point::new(term.topmost_line(), Column(0)),
            (None, Direction::Left) => Point::new(term.bottommost_line(), term.last_column()),
        };
        let origin = origin.grid_clamp(&*term, Boundary::Grid);
        let found = term.search_next(&mut search.regex, origin, direction, Side::Left, None);
        if let Some(found) = &found {
            let mut selection = Selection::new(SelectionType::Simple, *found.start(), Side::Left);
            selection.update(*found.end(), Side::Right);
            term.selection = Some(selection);
            if term.mode().contains(TermMode::VI) {
                term.vi_goto_point(*found.start());
            } else {
                term.scroll_to_point(*found.start());
            }
            self.full_damage = true;
        }
        search.last = found.clone();
        Ok(found)
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    /// Repeat the last search, as vi mode's `n` and `N` do.
    fn search_again(&mut self, direction: Direction) -> bool {
        let Some(pattern) = self.search.as_ref().map(|search| search.pattern.clone()) else {
            return false;
        };
        matches!(self.search(&pattern, direction), Ok(Some(_)))
    }

    /// Detect URLs with `pattern`, or disable detection with an empty pattern. `None` restores
    /// the default URL pattern.
    pub fn set_url_pattern(&mut self, pattern: Option<&str>) -> Result<(), String> {
        self.url_regex = match pattern {
            Some("") => Some(RegexSearch::new("$^").map_err(|err| err.to_string())?),
            Some(pattern) => Some(RegexSearch::new(pattern).map_err(|err| err.to_string())?),
            None => None,
        };
        Ok(())
    }

    fn url_regex(&mut self) -> &mut RegexSearch {
        self.url_regex.get_or_insert_with(|| RegexSearch::new(URL_REGEX).unwrap())
    }

    /// OSC 8 hyperlink or detected URL at a viewport cell.
    pub fn link_at(&mut self, column: usize, row: usize) -> Option<Link> {
        let term = self.term.clone();
        let term = term.lock();
        let point = Self::viewport_point(&term, column, row);
        if let Some((hyperlink, range)) = hyperlink_at(&term, point) {
            return Some(Link { uri: hyperlink.uri().to_owned(), range, hyperlink: true });
        }
        let range = regex_match_at(&term, point, self.url_regex(), true)?;
        let uri = term.bounds_to_string(*range.start(), *range.end());
        Some(Link { uri, range, hyperlink: false })
    }

    /// All visible OSC 8 hyperlinks and detected URLs, for keyboard hint interfaces.
    pub fn visible_links(&mut self) -> Vec<Link> {
        let term = self.term.clone();
        let term = term.lock();
        let mut links: Vec<_> = visible_unique_hyperlinks_iter(&term)
            .filter_map(|range| {
                let hyperlink = term.grid()[*range.start()].hyperlink()?;
                Some(Link { uri: hyperlink.uri().to_owned(), range, hyperlink: true })
            })
            .collect();
        let regex = self.url_regex();
        let matches: Vec<_> = visible_regex_match_iter(&term, regex).collect();
        for found in matches {
            for range in HintPostProcessor::new(&term, regex, found).collect::<Vec<_>>() {
                // Prefer hyperlinks covering the same text.
                if links.iter().any(|link| link.range.contains(range.start())) {
                    continue;
                }
                let uri = term.bounds_to_string(*range.start(), *range.end());
                links.push(Link { uri, range, hyperlink: false });
            }
        }
        links.sort_by_key(|link| *link.range.start());
        links
    }

    /// Enter or leave vi mode, which moves a keyboard cursor through the buffer.
    pub fn vi_toggle(&mut self) {
        let mut term = self.term.lock();
        if !term.mode().contains(TermMode::VI) {
            term.selection = None;
        }
        term.toggle_vi_mode();
        self.full_damage = true;
    }

    /// Move the vi cursor. Ignored outside vi mode.
    pub fn vi_motion(&mut self, motion: ViMotion) {
        self.term.lock().vi_motion(motion);
    }

    /// Toggle a selection of `ty` anchored at the vi cursor, like vi mode's `v`, `V` and `^V`.
    pub fn vi_select(&mut self, ty: SelectionType) {
        let mut term = self.term.lock();
        if !term.mode().contains(TermMode::VI) {
            return;
        }
        match &mut term.selection {
            Some(selection) if selection.ty == ty && !selection.is_empty() => {
                term.selection = None;
            },
            Some(selection) if !selection.is_empty() => selection.ty = ty,
            _ => {
                let point = term.vi_mode_cursor.point;
                let mut selection = Selection::new(ty, point, Side::Left);
                selection.include_all();
                term.selection = Some(selection);
            },
        }
        self.full_damage = true;
    }

    /// Scroll in vi mode, moving the vi cursor with the content.
    fn vi_scroll(&mut self, lines: i32) {
        let mut term = self.term.lock();
        term.vi_mode_cursor = term.vi_mode_cursor.scroll(&term, lines);
        term.scroll_display(Scroll::Delta(lines));
        self.full_damage = true;
    }

    /// Move the vi cursor to the first occupied cell of the topmost or bottommost line.
    fn vi_scroll_to(&mut self, top: bool) {
        let mut term = self.term.lock();
        if top {
            term.scroll_display(Scroll::Top);
            term.vi_mode_cursor.point.line = term.topmost_line();
            term.vi_motion(ViMotion::FirstOccupied);
        } else {
            term.scroll_display(Scroll::Bottom);
            term.vi_mode_cursor.point.line = term.bottommost_line();
            // Move to beginning twice, to always jump across linewraps.
            term.vi_motion(ViMotion::FirstOccupied);
            term.vi_motion(ViMotion::FirstOccupied);
        }
        self.full_damage = true;
    }

    /// Default vi mode key bindings of the desktop application.
    ///
    /// Search prompts (`/`, `?`) and inline search are left to the host.
    fn vi_key(&mut self, event: &KeyEvent<'_>) -> bool {
        if event.action == KeyAction::Release {
            return false;
        }
        let mods = event.mods - Modifiers::SHIFT;
        let control = mods == Modifiers::CONTROL;
        let character = match event.key {
            Key::Character(character) if mods.is_empty() || control => character,
            Key::Named(named) if mods.is_empty() => {
                let motion = match named {
                    NamedKey::ArrowUp => ViMotion::Up,
                    NamedKey::ArrowDown => ViMotion::Down,
                    NamedKey::ArrowLeft => ViMotion::Left,
                    NamedKey::ArrowRight => ViMotion::Right,
                    NamedKey::Home => ViMotion::First,
                    NamedKey::End => ViMotion::Last,
                    NamedKey::Escape => {
                        self.select_clear();
                        return true;
                    },
                    _ => return false,
                };
                self.vi_motion(motion);
                return true;
            },
            _ if mods == Modifiers::ALT && event.key == Key::Character('v') => {
                self.vi_select(SelectionType::Semantic);
                return true;
            },
            _ => return false,
        };

        let half_page = (self.options.size().lines / 2) as i32;
        let page = self.options.size().lines as i32;
        if control {
            match character.to_ascii_lowercase() {
                'c' => self.vi_toggle(),
                'y' => self.vi_scroll(1),
                'e' => self.vi_scroll(-1),
                'b' => self.vi_scroll(page),
                'f' => self.vi_scroll(-page),
                'u' => self.vi_scroll(half_page),
                'd' => self.vi_scroll(-half_page),
                'v' => self.vi_select(SelectionType::Block),
                _ => return false,
            }
            return true;
        }

        let motion = match character {
            'i' => {
                self.vi_toggle();
                self.scroll(Scroll::Bottom);
                return true;
            },
            'g' => {
                self.vi_scroll_to(true);
                return true;
            },
            'G' => {
                self.vi_scroll_to(false);
                return true;
            },
            'y' => {
                self.copy_selection(TerminalClipboardLocation::Clipboard);
                self.select_clear();
                return true;
            },
            'Y' => {
                self.vi_select(SelectionType::Simple);
                self.vi_motion(ViMotion::Last);
                self.copy_selection(TerminalClipboardLocation::Clipboard);
                self.select_clear();
                return true;
            },
            'v' => {
                self.vi_select(SelectionType::Simple);
                return true;
            },
            'V' => {
                self.vi_select(SelectionType::Lines);
                return true;
            },
            'n' => return self.search_again(Direction::Right),
            'N' => return self.search_again(Direction::Left),
            'z' => {
                let mut term = self.term.lock();
                let display_offset = term.grid().display_offset() as i32;
                let target = -display_offset + term.screen_lines() as i32 / 2 - 1;
                let line = term.vi_mode_cursor.point.line;
                term.scroll_display(Scroll::Delta(target - line.0));
                self.full_damage = true;
                return true;
            },
            'k' => ViMotion::Up,
            'j' => ViMotion::Down,
            'h' => ViMotion::Left,
            'l' => ViMotion::Right,
            '0' => ViMotion::First,
            '$' => ViMotion::Last,
            '^' => ViMotion::FirstOccupied,
            'H' => ViMotion::High,
            'M' => ViMotion::Middle,
            'L' => ViMotion::Low,
            'b' => ViMotion::SemanticLeft,
            'w' => ViMotion::SemanticRight,
            'e' => ViMotion::SemanticRightEnd,
            'B' => ViMotion::WordLeft,
            'W' => ViMotion::WordRight,
            'E' => ViMotion::WordRightEnd,
            '%' => ViMotion::Bracket,
            '{' => ViMotion::ParagraphUp,
            '}' => ViMotion::ParagraphDown,
            _ => return false,
        };
        self.vi_motion(motion);
        true
    }

    pub fn progress(&self) -> Progress {
        self.term.lock().progress()
    }

    /// Current window title set by the application.
    pub fn title(&self) -> Option<String> {
        self.term.lock().title().map(str::to_owned)
    }

    /// OSC 7 working directory as `(host, path)`.
    pub fn working_directory(&self) -> Option<(String, String)> {
        let term = self.term.lock();
        term.working_directory().map(|directory| (directory.host.clone(), directory.path.clone()))
    }

    /// OSC 133 shell integration state.
    pub fn shell_info(&self) -> ShellInfo {
        let term = self.term.lock();
        let state = term.shell_state();
        let last = state.last_command();
        ShellInfo {
            phase: state.phase(),
            last_command: last.map(|command| command.text.clone()),
            last_command_id: last.map(|command| command.id),
            last_exit_status: last.and_then(|command| command.exit_status),
        }
    }

    /// Output of the last finished shell command.
    pub fn last_command_output(&self) -> Option<String> {
        self.term.lock().last_command_output()
    }

    /// OSC 7501 program status records, oldest first.
    pub fn program_statuses(&self) -> Vec<ProgramStatusInfo> {
        let term = self.term.lock();
        let statuses = term.program_status();
        statuses
            .records()
            .iter()
            .map(|record| ProgramStatusInfo {
                app: statuses.app_for(record).map(str::to_owned),
                record: record.clone(),
            })
            .collect()
    }

    /// Capture the visible terminal for rendering.
    ///
    /// Damage is relative to the previous snapshot.
    pub fn snapshot(&mut self) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let full_damage = std::mem::take(&mut self.full_damage);
        self.capture(&mut snapshot, true, full_damage);
        snapshot
    }

    /// Refresh `snapshot` in place, reusing its allocations.
    ///
    /// Only damaged rows are copied when `snapshot` was the last one updated by this engine;
    /// otherwise the whole snapshot is rebuilt and reported as damaged.
    pub fn update_snapshot(&mut self, snapshot: &mut Snapshot) {
        let incremental = snapshot.id != 0 && snapshot.id == self.last_snapshot;
        let full_damage = std::mem::take(&mut self.full_damage) || !incremental;
        self.capture(snapshot, !incremental, full_damage);
    }

    fn capture(&mut self, snapshot: &mut Snapshot, rebuild: bool, full_damage: bool) {
        let mut term = self.term.lock();
        let update = Update { rebuild, full_damage, previous: self.view };
        self.view =
            Some(snapshot.update(&mut term, &self.palette, &self.options, self.focused, update));
        snapshot.id = NEXT_SNAPSHOT.fetch_add(1, Ordering::Relaxed);
        self.last_snapshot = snapshot.id;
        self.animation_deadline = snapshot.animation_deadline;
    }

    /// Color at palette `index`, including terminal overrides.
    pub fn color(&self, index: usize) -> Rgb {
        self.palette.get(self.term.lock().colors(), index)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Waits for an in-flight callback, so none runs once the host frees its userdata.
        *self.shared.wakeup.lock() = None;
    }
}

fn location(ty: ClipboardType) -> TerminalClipboardLocation {
    match ty {
        ClipboardType::Clipboard => TerminalClipboardLocation::Clipboard,
        ClipboardType::Selection => TerminalClipboardLocation::Primary,
    }
}

/// Permission policy for terminal-originated clipboard requests.
struct Host<'a> {
    clipboard: Option<&'a mut Box<dyn ClipboardProvider>>,
    permission: Osc52,
    focused: bool,
}

impl ClipboardHost for Host<'_> {
    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        let allowed = request.mime_types.is_empty()
            || request.permission_granted
            || matches!(self.permission, Osc52::OnlyPaste | Osc52::CopyPaste);
        if !self.focused || self.permission == Osc52::Disabled || !allowed {
            return TerminalClipboardReadResult::Denied;
        }
        let Some(clipboard) = self.clipboard.as_mut() else {
            return TerminalClipboardReadResult::Unsupported;
        };
        let Some(available_formats) = clipboard.formats(request.location) else {
            return TerminalClipboardReadResult::Unsupported;
        };
        let mut contents = Vec::new();
        let mut size = 0usize;
        for mime_type in &request.mime_types {
            let Some(data) = clipboard.read(request.location, mime_type) else { continue };
            size = size.saturating_add(data.len());
            if size > MAX_WRITE_BYTES {
                return TerminalClipboardReadResult::Busy;
            }
            contents.push(TerminalClipboardContent { mime_type: mime_type.clone(), data });
        }
        TerminalClipboardReadResult::Success {
            available_formats,
            contents,
            remember_permission: false,
        }
    }

    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        if !self.focused || !matches!(self.permission, Osc52::OnlyCopy | Osc52::CopyPaste) {
            return TerminalClipboardWriteResult::Denied;
        }
        let Some(clipboard) = self.clipboard.as_mut() else {
            return TerminalClipboardWriteResult::Unsupported;
        };
        if clipboard.write(request.location, &request.contents) {
            TerminalClipboardWriteResult::Success { remember_permission: false }
        } else {
            TerminalClipboardWriteResult::IoError
        }
    }
}
