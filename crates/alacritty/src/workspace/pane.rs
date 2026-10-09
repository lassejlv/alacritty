//! UI state for a pane and its terminal session.

use std::error::Error;
use std::time::Instant;

#[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
use glutin::platform::x11::X11GlConfigExt;
use log::info;
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use alacritty_session::Session;
use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Term;

use crate::app::{Event, EventProxy};
use crate::cli::WindowOptions;
use crate::config::UiConfig;
use crate::input::mouse::{Mouse, TouchPurpose};
use crate::presentation::SizeInfo;
use crate::presentation::search::{InlineSearchState, SearchState};
use crate::workspace::layout::PaneId;

/// A shell and its terminal/input state. Dropping a pane shuts down only its PTY.
pub(super) struct Pane {
    pub(super) id: PaneId,
    pub(super) kitty_clipboard: alacritty_terminal::clipboard::KittyClipboardHostState,
    pub(super) notifications: alacritty_terminal::protocols::notifications::Notifications,
    pub(super) native_notifications: crate::platform::notifications::NativeNotifications,
    pub(super) title: String,
    pub(super) session: Session<EventProxy>,
    pub(super) cursor_blink_timed_out: bool,
    pub(super) prev_bell_cmd: Option<Instant>,
    pub(super) inline_search_state: InlineSearchState,
    pub(super) search_state: SearchState,
    pub(super) mouse: Mouse,
    pub(super) touch: TouchPurpose,
    pub(super) preserve_title: bool,
    _shell_integration: Option<tempfile::TempDir>,
}

impl Pane {
    pub(super) fn reset_notifications(&mut self) {
        use alacritty_terminal::protocols::notifications::Effect;
        for effect in self.notifications.reset() {
            if let Effect::Close(serial) = effect {
                self.native_notifications.close(serial);
            }
        }
    }

    pub(super) fn window_title(&self, config: &UiConfig) -> String {
        if self.preserve_title || !config.window.dynamic_title {
            return self.title.clone();
        }
        let terminal = self.session.terminal.lock();
        crate::presentation::status::window_title(terminal.program_status(), Some(&self.title))
            .unwrap_or_else(|| self.title.clone())
    }

    pub(super) fn new(
        id: PaneId,
        size_info: SizeInfo,
        window_id: WindowId,
        config: &UiConfig,
        options: WindowOptions,
        proxy: EventLoopProxy<Event>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut pty_config = config.pty_config();
        options.terminal_options.override_pty_config(&mut pty_config);
        let shell_integration = if config.terminal.shell_integration {
            match crate::app::shell_integration::prepare(&mut pty_config) {
                Ok(integration) => integration,
                Err(err) => {
                    log::warn!("Unable to prepare shell integration: {err}");
                    None
                },
            }
        } else {
            None
        };

        let preserve_title = options.window_identity.title.is_some();

        info!("PTY dimensions: {:?} x {:?}", size_info.screen_lines(), size_info.columns());

        let event_proxy = EventProxy::new(proxy.clone(), window_id).with_pane(id);

        // Create the terminal.
        //
        // This object contains all of the state about what's being displayed. It's
        // wrapped in a clonable mutex since both the I/O loop and display need to
        // access it.
        let mut terminal = Term::new(config.term_options(), &size_info, event_proxy.clone());
        terminal.set_graphics_cell_size(size_info.cell_width(), size_info.cell_height());
        let session = Session::new(
            terminal,
            &pty_config,
            size_info.into(),
            window_id.into(),
            event_proxy.clone(),
            config.debug.ref_test,
        )?;

        // Start cursor blinking, in case `Focused` isn't sent on startup.
        if config.cursor.style().blinking {
            event_proxy.send_event(TerminalEvent::CursorBlinkingChange.into());
        }

        Ok(Self {
            id,
            kitty_clipboard: Default::default(),
            notifications: Default::default(),
            native_notifications: crate::platform::notifications::NativeNotifications::new(
                window_id, id, proxy,
            ),
            title: options
                .window_identity
                .title
                .unwrap_or_else(|| config.window.identity.title.clone()),
            preserve_title,
            _shell_integration: shell_integration,
            session,
            cursor_blink_timed_out: false,
            prev_bell_cmd: None,
            inline_search_state: Default::default(),
            search_state: Default::default(),
            mouse: Default::default(),
            touch: Default::default(),
        })
    }
}
