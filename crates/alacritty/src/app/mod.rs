//! Desktop application lifecycle and native event dispatch.

use crate::config::monitor::ConfigMonitor;
use glutin::config::GetGlConfig;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error;
use std::mem;
use std::rc::Rc;

use ahash::RandomState;
use glutin::config::Config as GlutinConfig;
use glutin::display::GetGlDisplay;
use log::{error, info};
use winit::application::ApplicationHandler;
use winit::event::{Event as WinitEvent, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop, EventLoopProxy};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::WindowId;

use alacritty_terminal::event::Event as TerminalEvent;

#[cfg(unix)]
use crate::cli::ParsedOptions;
use crate::cli::{Options as CliOptions, WindowOptions};
use crate::clipboard::Clipboard;
use crate::config::{self, UiConfig};
#[cfg(unix)]
use crate::ipc::server::{self as ipc, SocketReply};
use crate::logging::{LOG_TARGET_CONFIG, LOG_TARGET_WINIT};
use crate::workspace::window::WindowContext;

pub mod commands;
pub mod context;
pub mod events;
pub mod scheduler;
pub mod shell_integration;
mod startup;

pub use context::ActionContext;
pub use events::{Event, EventProxy, EventType};
pub use scheduler::Scheduler;
pub use startup::run;

/// The event processor.
///
/// Stores some state from received events and dispatches actions when they are
/// triggered.
pub struct Processor {
    pub config_monitor: Option<ConfigMonitor>,

    clipboard: Clipboard,
    scheduler: Scheduler,
    initial_window_options: Option<WindowOptions>,
    initial_window_error: Option<Box<dyn Error>>,
    windows: HashMap<WindowId, WindowContext, RandomState>,
    proxy: EventLoopProxy<Event>,
    gl_config: Option<GlutinConfig>,
    #[cfg(unix)]
    global_ipc_options: ParsedOptions,
    cli_options: CliOptions,
    config: Rc<UiConfig>,
    system_progress: crate::platform::progress::SystemProgress,
    progress_dirty: bool,
    #[cfg(target_os = "macos")]
    updater: Option<crate::platform::macos::updater::Updater>,
    #[cfg(target_os = "macos")]
    menus: Option<crate::platform::macos::menus::Menus>,
}

impl Processor {
    /// Create a new event processor.
    pub fn new(
        config: UiConfig,
        cli_options: CliOptions,
        event_loop: &EventLoop<Event>,
    ) -> Processor {
        let proxy = event_loop.create_proxy();
        let scheduler = Scheduler::new(proxy.clone());
        let initial_window_options = Some(cli_options.window_options.clone());

        // Disable all device events, since we don't care about them.
        event_loop.listen_device_events(DeviceEvents::Never);

        // SAFETY: Since this takes a pointer to the winit event loop, it MUST be dropped first,
        // which is done in `loop_exiting`.
        let clipboard = unsafe { Clipboard::new(event_loop.display_handle().unwrap().as_raw()) };

        // Create a config monitor.
        //
        // The monitor watches the config file for changes and reloads it. Pending
        // config changes are processed in the main loop.
        let mut config_monitor = None;
        if config.live_config_reload() {
            config_monitor =
                ConfigMonitor::new(config.config_paths.clone(), event_loop.create_proxy());
        }

        Processor {
            initial_window_options,
            initial_window_error: None,
            cli_options,
            proxy,
            scheduler,
            gl_config: None,
            config: Rc::new(config),
            clipboard,
            windows: Default::default(),
            #[cfg(unix)]
            global_ipc_options: Default::default(),
            config_monitor,
            system_progress: Default::default(),
            progress_dirty: false,
            #[cfg(target_os = "macos")]
            updater: None,
            #[cfg(target_os = "macos")]
            menus: None,
        }
    }

    /// Create initial window and load GL platform.
    ///
    /// This will initialize the OpenGL Api and pick a config that
    /// will be used for the rest of the windows.
    pub fn create_initial_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_options: WindowOptions,
    ) -> Result<(), Box<dyn Error>> {
        let window_context = WindowContext::initial(
            event_loop,
            self.proxy.clone(),
            self.config.clone(),
            window_options,
        )?;

        self.gl_config = Some(window_context.display.gl_context().config());
        self.windows.insert(window_context.id(), window_context);

        #[cfg(target_os = "macos")]
        {
            if self.menus.is_none() {
                self.menus = crate::platform::macos::menus::Menus::new(self.proxy.clone());
                self.updater = crate::platform::macos::updater::Updater::new();
            }
            self.refresh_menu_windows();
        }

        Ok(())
    }

    /// Create a new terminal window.
    pub fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        options: WindowOptions,
        #[cfg(target_os = "macos")] parent_id: Option<WindowId>,
    ) -> Result<(), Box<dyn Error>> {
        #[cfg(target_os = "macos")]
        let tabbing_id = options.window_tabbing_id.clone();
        let gl_config = self.gl_config.as_ref().unwrap();

        // Override config with CLI/IPC options.
        let mut config_overrides = options.config_overrides();
        #[cfg(unix)]
        config_overrides.extend_from_slice(&self.global_ipc_options);
        let mut config = self.config.clone();
        config = config_overrides.override_config_rc(config);

        let window_context = WindowContext::additional(
            gl_config,
            event_loop,
            self.proxy.clone(),
            config,
            options,
            config_overrides,
        )?;

        #[cfg(target_os = "macos")]
        if let Some(tabbing_id) = tabbing_id {
            let parent = parent_id.and_then(|id| self.windows.get(&id)).or_else(|| {
                self.windows
                    .values()
                    .find(|window| window.display.window.tabbing_id() == tabbing_id)
            });
            if let Some(parent) = parent {
                window_context.display.window.join_tab_group(&parent.display.window);
            }
        }
        self.windows.insert(window_context.id(), window_context);
        #[cfg(target_os = "macos")]
        self.refresh_menu_windows();
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn refresh_menu_windows(&self) {
        if let Some(menus) = &self.menus {
            let config = self
                .windows
                .values()
                .find(|window| window.display.window.has_focus())
                .map(|window| window.config())
                .unwrap_or(&self.config);
            menus.configure_shortcuts(config);
            menus.set_windows(
                self.windows
                    .values()
                    .filter_map(|window| {
                        window.display.window.native_window_number().map(|number| {
                            (
                                number,
                                window.config().window.decorations
                                    != crate::config::window::Decorations::None,
                            )
                        })
                    })
                    .collect(),
            );
        }
    }

    /// Run the event loop.
    ///
    /// The result is exit code generate from the loop.
    pub fn run(&mut self, event_loop: EventLoop<Event>) -> Result<(), Box<dyn Error>> {
        let result = event_loop.run_app(self);
        match self.initial_window_error.take() {
            Some(initial_window_error) => Err(initial_window_error),
            _ => result.map_err(Into::into),
        }
    }

    /// Check if an event is irrelevant and can be skipped.
    fn skip_window_event(event: &WindowEvent) -> bool {
        matches!(
            event,
            WindowEvent::KeyboardInput { is_synthetic: true, .. }
                | WindowEvent::ActivationTokenDone { .. }
                | WindowEvent::DoubleTapGesture { .. }
                | WindowEvent::TouchpadPressure { .. }
                | WindowEvent::RotationGesture { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::PinchGesture { .. }
                | WindowEvent::AxisMotion { .. }
                | WindowEvent::PanGesture { .. }
                | WindowEvent::HoveredFileCancelled
                | WindowEvent::Destroyed
                | WindowEvent::ThemeChanged(_)
                | WindowEvent::HoveredFile(_)
                | WindowEvent::Moved(_)
        )
    }
}

impl ApplicationHandler<Event> for Processor {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if cause != StartCause::Init || self.cli_options.daemon {
            return;
        }

        if let Some(window_options) = self.initial_window_options.take()
            && let Err(err) = self.create_initial_window(event_loop, window_options)
        {
            self.initial_window_error = Some(err);
            event_loop.exit();
            return;
        }

        info!("Initialisation complete");
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.config.debug.print_events {
            info!(target: LOG_TARGET_WINIT, "{event:?}");
        }

        #[cfg(target_os = "macos")]
        if matches!(event, WindowEvent::Focused(true))
            && let (Some(menus), Some(window)) = (&self.menus, self.windows.get(&window_id))
        {
            menus.configure_shortcuts(window.config());
        }

        // Ignore all events we do not care about.
        if Self::skip_window_event(&event) {
            return;
        }

        let window_context = match self.windows.get_mut(&window_id) {
            Some(window_context) => window_context,
            None => return,
        };

        let is_redraw = matches!(event, WindowEvent::RedrawRequested);

        window_context.handle_event(
            #[cfg(target_os = "macos")]
            _event_loop,
            &self.proxy,
            &mut self.clipboard,
            &mut self.scheduler,
            WinitEvent::WindowEvent { window_id, event },
        );

        if is_redraw {
            window_context.draw(&mut self.scheduler);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        if self.config.debug.print_events {
            info!(target: LOG_TARGET_WINIT, "{event:?}");
        }

        // Pane progress, exits and layout changes can all change native progress.
        self.progress_dirty |= matches!(
            event.payload,
            EventType::Terminal(TerminalEvent::ProgressChanged | TerminalEvent::Exit)
                | EventType::Pane(_)
        );

        // Handle events which don't mandate the WindowId.
        match (event.payload, event.window_id.as_ref()) {
            // Process IPC config update.
            #[cfg(unix)]
            (EventType::IpcConfig(ipc_config), window_id) => {
                // Try and parse options as toml.
                let mut options = ParsedOptions::from_options(&ipc_config.options);

                // Override IPC config for each window with matching ID.
                for (_, window_context) in self
                    .windows
                    .iter_mut()
                    .filter(|(id, _)| window_id.is_none() || window_id == Some(*id))
                {
                    if ipc_config.reset {
                        window_context.reset_window_config(self.config.clone());
                    } else {
                        window_context.add_window_config(self.config.clone(), &options);
                    }
                }

                // Persist global options for future windows.
                if window_id.is_none() {
                    if ipc_config.reset {
                        self.global_ipc_options.clear();
                    } else {
                        self.global_ipc_options.append(&mut options);
                    }
                }
            },
            // Process IPC config requests.
            #[cfg(unix)]
            (EventType::IpcGetConfig(stream), window_id) => {
                // Get the config for the requested window ID.
                let config = match self.windows.iter().find(|(id, _)| window_id == Some(*id)) {
                    Some((_, window_context)) => window_context.config(),
                    None => &self.global_ipc_options.override_config_rc(self.config.clone()),
                };

                // Convert config to JSON format.
                let config_json = match serde_json::to_string(&config) {
                    Ok(config_json) => config_json,
                    Err(err) => {
                        error!("Failed config serialization: {err}");
                        return;
                    },
                };

                // Send JSON config to the socket.
                if let Ok(mut stream) = stream.try_clone() {
                    ipc::send_reply(&mut stream, SocketReply::GetConfig(config_json));
                }
            },
            (EventType::ConfigReload(path), _) => {
                // Clear config logs from message bar for all terminals.
                for window_context in self.windows.values_mut() {
                    if !window_context.message_buffer.is_empty() {
                        window_context.message_buffer.remove_target(LOG_TARGET_CONFIG);
                        window_context.display.pending_update.dirty = true;
                    }
                }

                // Load config and update each terminal.
                if let Ok(config) = config::reload(&path, &mut self.cli_options) {
                    self.config = Rc::new(config);

                    // Restart config monitor if imports changed.
                    if let Some(monitor) = self.config_monitor.take() {
                        let paths = &self.config.config_paths;
                        self.config_monitor = if monitor.needs_restart(paths) {
                            monitor.shutdown();
                            ConfigMonitor::new(paths.clone(), self.proxy.clone())
                        } else {
                            Some(monitor)
                        };
                    } else if self.config.live_config_reload() {
                        self.config_monitor = ConfigMonitor::new(
                            self.config.config_paths.clone(),
                            self.proxy.clone(),
                        );
                    }

                    for window_context in self.windows.values_mut() {
                        window_context.update_config(self.config.clone());
                    }
                    #[cfg(target_os = "macos")]
                    self.refresh_menu_windows();
                }
            },
            #[cfg(target_os = "macos")]
            (EventType::MacosMenu(command, number), _) => {
                use crate::platform::macos::menus::Command;
                let window_id = self.windows.iter().find_map(|(id, window)| {
                    (window.display.window.native_window_number() == number).then_some(*id)
                });
                match command {
                    command if command.action().is_some() && window_id.is_some() => {
                        let _ = self
                            .proxy
                            .send_event(Event::new(EventType::MacosAction(command), window_id));
                    },
                    Command::CreateNewWindow => {
                        let _ = self.proxy.send_event(Event::new(
                            EventType::CreateWindow(WindowOptions::default()),
                            None,
                        ));
                    },
                    Command::OpenConfig => {
                        crate::platform::macos::config_ui::open_config(&self.config, &self.proxy);
                    },
                    Command::ReloadConfig => {
                        if let Some(path) = self.config.config_paths.first() {
                            let _ = self.proxy.send_event(Event::new(
                                EventType::ConfigReload(path.clone()),
                                None,
                            ));
                        }
                    },
                    Command::MigrateGhostty => {
                        crate::platform::macos::config_ui::migrate_ghostty(
                            &self.config,
                            &self.proxy,
                        );
                    },
                    _ => (),
                }
            },
            // Create a new terminal window.
            (EventType::CreateWindow(options), _parent_id) => {
                // XXX Ensure that no context is current when creating a new window,
                // otherwise it may lock the backing buffer of the
                // surface of current context when asking
                // e.g. EGL on Wayland to create a new context.
                for window_context in self.windows.values_mut() {
                    window_context.display.make_not_current();
                }

                if self.gl_config.is_none() {
                    // Handle initial window creation in daemon mode.
                    if let Err(err) = self.create_initial_window(event_loop, options) {
                        self.initial_window_error = Some(err);
                        event_loop.exit();
                    }
                } else if let Err(err) = self.create_window(
                    event_loop,
                    options,
                    #[cfg(target_os = "macos")]
                    _parent_id.copied(),
                ) {
                    error!("Could not open window: {err:?}");
                }
            },
            // Shutdown all windows.
            #[cfg(unix)]
            (EventType::Shutdown, _) => event_loop.exit(),
            // Process events affecting all windows.
            (payload, None) => {
                let event = WinitEvent::UserEvent(Event::new(payload, None));
                for window_context in self.windows.values_mut() {
                    window_context.handle_event(
                        #[cfg(target_os = "macos")]
                        event_loop,
                        &self.proxy,
                        &mut self.clipboard,
                        &mut self.scheduler,
                        event.clone(),
                    );
                }
            },
            (EventType::Terminal(TerminalEvent::Wakeup), Some(window_id)) => {
                if let Some(window_context) = self.windows.get_mut(window_id) {
                    window_context.dirty = true;
                    if window_context.display.window.has_frame {
                        window_context.display.window.request_redraw();
                    }
                }
            },
            (EventType::Terminal(TerminalEvent::Exit), Some(window_id)) => {
                if let Some(window) = self.windows.get_mut(window_id)
                    && !window.display.window.hold
                    && window.remove_exited_pane(event.pane_id, &mut self.scheduler)
                {
                    return;
                }
                // Remove the closed terminal.
                let window_context = match self.windows.entry(*window_id) {
                    // Don't exit when terminal exits if user asked to hold the window.
                    Entry::Occupied(window_context)
                        if !window_context.get().display.window.hold =>
                    {
                        window_context.remove()
                    },
                    _ => return,
                };

                #[cfg(target_os = "macos")]
                self.refresh_menu_windows();

                // Unschedule pending events.
                self.scheduler.unschedule_window(window_context.id());

                // Shutdown if no more terminals are open.
                if self.windows.is_empty() && !self.cli_options.daemon {
                    // Write ref tests of last window to disk.
                    if self.config.debug.ref_test {
                        window_context.write_ref_test_results();
                    }

                    event_loop.exit();
                }
            },
            // NOTE: This event bypasses batching to minimize input latency.
            (EventType::Frame, Some(window_id)) => {
                if let Some(window_context) = self.windows.get_mut(window_id) {
                    window_context.display.window.has_frame = true;
                    if window_context.dirty {
                        window_context.display.window.request_redraw();
                    }
                }
            },
            (payload, Some(window_id)) => {
                if let Some(window_context) = self.windows.get_mut(window_id) {
                    window_context.handle_event(
                        #[cfg(target_os = "macos")]
                        event_loop,
                        &self.proxy,
                        &mut self.clipboard,
                        &mut self.scheduler,
                        WinitEvent::UserEvent(Event {
                            payload,
                            window_id: Some(*window_id),
                            pane_id: event.pane_id,
                        }),
                    );
                }
            },
        };
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.config.debug.print_events {
            info!(target: LOG_TARGET_WINIT, "About to wait");
        }

        // Dispatch event to all windows.
        for window_context in self.windows.values_mut() {
            window_context.handle_event(
                #[cfg(target_os = "macos")]
                event_loop,
                &self.proxy,
                &mut self.clipboard,
                &mut self.scheduler,
                WinitEvent::AboutToWait,
            );
        }

        if mem::take(&mut self.progress_dirty) {
            let windows: Vec<_> = self
                .windows
                .values()
                .map(|window| (window.display.window.raw_window_handle(), window.progress()))
                .collect();
            self.system_progress.update(&windows);
        }

        // Update the scheduler after event processing to ensure
        // the event loop deadline is as accurate as possible.
        let control_flow = match self.scheduler.update() {
            Some(instant) => ControlFlow::WaitUntil(instant),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(control_flow);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.config.debug.print_events {
            info!("Exiting the event loop");
        }

        match self.gl_config.take().map(|config| config.display()) {
            #[cfg(not(target_os = "macos"))]
            Some(glutin::display::Display::Egl(display)) => {
                // Ensure that all the windows are dropped, so the destructors for
                // Renderer and contexts ran.
                self.windows.clear();

                // SAFETY: the display is being destroyed after destroying all the
                // windows, thus no attempt to access the EGL state will be made.
                unsafe {
                    display.terminate();
                }
            },
            _ => (),
        }

        // SAFETY: The clipboard must be dropped before the event loop, so use the nop clipboard
        // as a safe placeholder.
        self.clipboard = Clipboard::new_nop();
    }
}
