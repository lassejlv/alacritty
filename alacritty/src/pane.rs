//! Per-pane terminal and PTY state.

use std::error::Error;
#[cfg(not(windows))]
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;

use log::info;
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{self, Options as PtyOptions};

use crate::config::UiConfig;
use crate::display::SizeInfo;
use crate::event::{Event, EventProxy, InlineSearchState, SearchState};
use crate::layout::PaneId;
use crate::message_bar::MessageBuffer;

/// One terminal pane: PTY, grid, and UI state that is not shared with siblings.
pub struct Pane {
    pub terminal: Arc<FairMutex<Term<EventProxy>>>,
    pub notifier: Notifier,
    pub search_state: SearchState,
    pub inline_search_state: InlineSearchState,
    pub message_buffer: MessageBuffer,
    pub cursor_blink_timed_out: bool,
    pub size: SizeInfo,
    #[cfg(not(windows))]
    pub master_fd: RawFd,
    #[cfg(not(windows))]
    pub shell_pid: u32,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        config: &UiConfig,
        size: SizeInfo,
        window_id: WindowId,
        proxy: EventLoopProxy<Event>,
        pty_config: PtyOptions,
    ) -> Result<Self, Box<dyn Error>> {
        info!(
            "PTY dimensions for pane {}: {:?} x {:?}",
            id.raw(),
            size.screen_lines(),
            size.columns()
        );

        let event_proxy = EventProxy::new(proxy, window_id, id);
        let terminal = Term::new(config.term_options(), &size, event_proxy.clone());
        let terminal = Arc::new(FairMutex::new(terminal));

        let pty = tty::new(&pty_config, size.into(), window_id.into())?;

        #[cfg(not(windows))]
        let master_fd = pty.file().as_raw_fd();
        #[cfg(not(windows))]
        let shell_pid = pty.child().id();

        let event_loop = PtyEventLoop::new(
            Arc::clone(&terminal),
            event_proxy.clone(),
            pty,
            pty_config.drain_on_exit,
            config.debug.ref_test,
        )?;
        let loop_tx = event_loop.channel();
        let _io_thread = event_loop.spawn();

        if config.cursor.style().blinking {
            event_proxy.send_event(TerminalEvent::CursorBlinkingChange.into());
        }

        Ok(Self {
            terminal,
            notifier: Notifier(loop_tx),
            search_state: Default::default(),
            inline_search_state: Default::default(),
            message_buffer: Default::default(),
            cursor_blink_timed_out: Default::default(),
            size,
            #[cfg(not(windows))]
            master_fd,
            #[cfg(not(windows))]
            shell_pid,
        })
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
