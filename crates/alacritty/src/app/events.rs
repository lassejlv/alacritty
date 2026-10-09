//! Events exchanged between platform callbacks, PTY workers and windows.

use std::fmt::Debug;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;

use winit::event::Event as WinitEvent;
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use alacritty_terminal::event::{Event as TerminalEvent, EventListener};
use alacritty_terminal::grid::Scroll;

#[cfg(unix)]
use crate::cli::IpcConfig;
use crate::cli::WindowOptions;
use crate::presentation::messages::Message;

/// Alacritty events.
#[derive(Debug, Clone)]
pub struct Event {
    /// Limit event to a specific window.
    pub(crate) window_id: Option<WindowId>,

    /// Event payload.
    pub(crate) payload: EventType,

    /// Route PTY and timer events to their originating pane; commands use current focus.
    pub(crate) pane_id: Option<crate::workspace::layout::PaneId>,
}

impl Event {
    pub fn with_pane(mut self, pane_id: crate::workspace::layout::PaneId) -> Self {
        self.pane_id = Some(pane_id);
        self
    }

    pub fn new<I: Into<Option<WindowId>>>(payload: EventType, window_id: I) -> Self {
        Self { window_id: window_id.into(), payload, pane_id: None }
    }
}

impl From<Event> for WinitEvent<Event> {
    fn from(event: Event) -> Self {
        WinitEvent::UserEvent(event)
    }
}

/// Alacritty events.
#[derive(Debug, Clone)]
pub enum EventType {
    Terminal(TerminalEvent),
    Pane(crate::workspace::layout::PaneCommand),
    ConfigReload(PathBuf),
    #[cfg(target_os = "macos")]
    MacosMenu(crate::platform::macos::menus::Command, Option<isize>),
    #[cfg(target_os = "macos")]
    MacosAction(crate::platform::macos::menus::Command),
    Message(Message),
    Scroll(Scroll),
    CreateWindow(WindowOptions),
    #[cfg(unix)]
    IpcConfig(IpcConfig),
    #[cfg(unix)]
    IpcGetConfig(Arc<UnixStream>),
    BlinkCursor,
    BlinkCursorTimeout,
    GraphicsAnimation,
    NotificationFeedback(crate::platform::notifications::Feedback),
    NotificationExpiry,
    SearchNext,
    #[cfg(unix)]
    Shutdown,
    Frame,
}

impl From<TerminalEvent> for EventType {
    fn from(event: TerminalEvent) -> Self {
        Self::Terminal(event)
    }
}

#[derive(Debug, Clone)]
pub struct EventProxy {
    proxy: EventLoopProxy<Event>,
    window_id: WindowId,
    pane_id: crate::workspace::layout::PaneId,
}

impl EventProxy {
    pub fn with_pane(mut self, pane_id: crate::workspace::layout::PaneId) -> Self {
        self.pane_id = pane_id;
        self
    }

    pub fn new(proxy: EventLoopProxy<Event>, window_id: WindowId) -> Self {
        Self { proxy, window_id, pane_id: 0 }
    }

    /// Send an event to the event loop.
    pub fn send_event(&self, event: EventType) {
        let _ = self.proxy.send_event(Event::new(event, self.window_id).with_pane(self.pane_id));
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: TerminalEvent) {
        let _ =
            self.proxy.send_event(Event::new(event.into(), self.window_id).with_pane(self.pane_id));
    }
}
