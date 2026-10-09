//! Terminal window context.

use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::mem;
#[cfg(not(windows))]
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use glutin::config::Config as GlutinConfig;
use glutin::display::GetGlDisplay;
#[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
use glutin::platform::x11::X11GlConfigExt;
use log::{debug, error, info};
use serde_json as json;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, Event as WinitEvent, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{CursorIcon, WindowId};

use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::event_loop::{EventLoop as PtyEventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::tty;

use crate::cli::{ParsedOptions, WindowOptions};
use crate::clipboard::Clipboard;
use crate::config::UiConfig;
use crate::display::window::Window;
use crate::display::{Display, PaneDisplay, SizeInfo};
use crate::event::{
    ActionContext, Event, EventProxy, InlineSearchState, Mouse, SearchState, TouchPurpose,
};
#[cfg(unix)]
use crate::logging::LOG_TARGET_IPC_CONFIG;
use crate::message_bar::MessageBuffer;
use crate::panes::{Axis, Layout, PaneCommand, PaneId, Rect};
use crate::scheduler::Scheduler;
use crate::{input, renderer};

/// Event context for one individual Alacritty window.
pub struct WindowContext {
    pub message_buffer: MessageBuffer,
    pub display: Display,
    pub dirty: bool,
    event_queue: Vec<WinitEvent<Event>>,
    pane: Pane,
    inactive: Vec<(Pane, PaneDisplay)>,
    layout: Layout,
    next_pane_id: PaneId,
    layout_dirty: bool,
    pointer: PhysicalPosition<f64>,
    divider_drag: Option<Vec<bool>>,
    focused: bool,
    modifiers: Modifiers,
    occluded: bool,
    window_config: ParsedOptions,
    config: Rc<UiConfig>,
}

/// A shell and its terminal/input state. Dropping a pane shuts down only its PTY.
struct Pane {
    id: PaneId,
    kitty_clipboard: alacritty_terminal::clipboard::KittyClipboardHostState,
    title: String,
    terminal: Arc<FairMutex<Term<EventProxy>>>,
    cursor_blink_timed_out: bool,
    prev_bell_cmd: Option<Instant>,
    inline_search_state: InlineSearchState,
    search_state: SearchState,
    notifier: Notifier,
    mouse: Mouse,
    touch: TouchPurpose,
    preserve_title: bool,
    #[cfg(not(windows))]
    master_fd: RawFd,
    #[cfg(not(windows))]
    shell_pid: u32,
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

impl WindowContext {
    /// Create initial window context that does bootstrapping the graphics API we're going to use.
    pub fn initial(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<Event>,
        config: Rc<UiConfig>,
        mut options: WindowOptions,
    ) -> Result<Self, Box<dyn Error>> {
        let raw_display_handle = event_loop.display_handle().unwrap().as_raw();

        let mut identity = config.window.identity.clone();
        options.window_identity.override_identity_config(&mut identity);

        // Windows has different order of GL platform initialization compared to any other platform;
        // it requires the window first.
        #[cfg(windows)]
        let window = Window::new(event_loop, &config, &identity, &mut options)?;
        #[cfg(windows)]
        let raw_window_handle = Some(window.raw_window_handle());

        #[cfg(not(windows))]
        let raw_window_handle = None;

        let gl_display = renderer::platform::create_gl_display(
            raw_display_handle,
            raw_window_handle,
            config.debug.prefer_egl,
        )?;
        let gl_config = renderer::platform::pick_gl_config(&gl_display, raw_window_handle)?;

        #[cfg(not(windows))]
        let window = Window::new(
            event_loop,
            &config,
            &identity,
            &mut options,
            #[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
            gl_config.x11_visual(),
        )?;

        // Create context.
        let gl_context =
            renderer::platform::create_gl_context(&gl_display, &gl_config, raw_window_handle)?;

        let display = Display::new(window, gl_context, &config, false)?;

        Self::new(display, config, options, proxy)
    }

    /// Create additional context with the graphics platform other windows are using.
    pub fn additional(
        gl_config: &GlutinConfig,
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<Event>,
        config: Rc<UiConfig>,
        mut options: WindowOptions,
        config_overrides: ParsedOptions,
    ) -> Result<Self, Box<dyn Error>> {
        let gl_display = gl_config.display();

        let mut identity = config.window.identity.clone();
        options.window_identity.override_identity_config(&mut identity);

        // Check if new window will be opened as a tab.
        // This must be done before `Window::new()`, which unsets `window_tabbing_id`.
        #[cfg(target_os = "macos")]
        let tabbed = options.window_tabbing_id.is_some();
        #[cfg(not(target_os = "macos"))]
        let tabbed = false;

        let window = Window::new(
            event_loop,
            &config,
            &identity,
            &mut options,
            #[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
            gl_config.x11_visual(),
        )?;

        // Create context.
        let raw_window_handle = window.raw_window_handle();
        let gl_context =
            renderer::platform::create_gl_context(&gl_display, gl_config, Some(raw_window_handle))?;

        let display = Display::new(window, gl_context, &config, tabbed)?;

        let mut window_context = Self::new(display, config, options, proxy)?;

        // Set the config overrides at startup.
        //
        // These are already applied to `config`, so no update is necessary.
        window_context.window_config = config_overrides;

        Ok(window_context)
    }

    /// Create a new terminal window context.
    fn new(
        display: Display,
        config: Rc<UiConfig>,
        options: WindowOptions,
        proxy: EventLoopProxy<Event>,
    ) -> Result<Self, Box<dyn Error>> {
        let pane = Pane::new(0, display.size_info, display.window.id(), &config, options, proxy)?;
        Ok(Self {
            display,
            config,
            pane,
            inactive: Vec::new(),
            layout: Layout::Leaf(0),
            next_pane_id: 1,
            layout_dirty: false,
            pointer: PhysicalPosition::new(0., 0.),
            divider_drag: None,
            focused: true,
            message_buffer: Default::default(),
            window_config: Default::default(),
            event_queue: Default::default(),
            modifiers: Default::default(),
            occluded: false,
            dirty: true,
        })
    }

    /// Update the terminal window to the latest config.
    pub fn update_config(&mut self, new_config: Rc<UiConfig>) {
        let old_config = mem::replace(&mut self.config, new_config);

        // Apply ipc config if there are overrides.
        self.config = self.window_config.override_config_rc(self.config.clone());

        let active = self.pane.id;
        let ids: Vec<_> = self.inactive.iter().map(|(p, _)| p.id).collect();
        for id in ids {
            self.load_pane(id);
            self.pane.terminal.lock().set_options(self.config.term_options());
            self.display.visual_bell.update_config(&self.config.bell);
            self.display.hint_state.update_alphabet(self.config.hints.alphabet());
            self.display.pending_update.dirty = true;
            if !self.pane.preserve_title
                && (!self.config.window.dynamic_title
                    || self.pane.title == old_config.window.identity.title)
            {
                self.pane.title = self.config.window.identity.title.clone();
            }
        }
        self.load_pane(active);
        self.display.update_config(&self.config);
        self.pane.terminal.lock().set_options(self.config.term_options());

        // Reload cursor if its thickness has changed.
        if (old_config.cursor.thickness() - self.config.cursor.thickness()).abs() > f32::EPSILON {
            self.display.pending_update.set_cursor_dirty();
        }

        if old_config.font != self.config.font {
            let scale_factor = self.display.window.scale_factor as f32;
            // Do not update font size if it has been changed at runtime.
            if self.display.font_size == old_config.font.size().scale(scale_factor) {
                self.display.font_size = self.config.font.size().scale(scale_factor);
            }

            let font = self.config.font.clone().with_size(self.display.font_size);
            self.display.pending_update.set_font(font);
        }

        // Always reload the theme to account for auto-theme switching.
        self.display.window.set_theme(self.config.window.theme());

        // Update display if either padding options or resize increments were changed.
        let window_config = &old_config.window;
        if window_config.padding(1.) != self.config.window.padding(1.)
            || window_config.dynamic_padding != self.config.window.dynamic_padding
            || window_config.resize_increments != self.config.window.resize_increments
        {
            self.display.pending_update.dirty = true;
        }

        // Update title on config reload according to the following table.
        //
        // │cli │ dynamic_title │ current_title == old_config ││ set_title │
        // │ Y  │       _       │              _              ││     N     │
        // │ N  │       Y       │              Y              ││     Y     │
        // │ N  │       Y       │              N              ││     N     │
        // │ N  │       N       │              _              ││     Y     │
        if !self.pane.preserve_title
            && (!self.config.window.dynamic_title
                || self.display.window.title() == old_config.window.identity.title)
        {
            self.pane.title = self.config.window.identity.title.clone();
            self.display.window.set_title(self.pane.title.clone());
        }

        let opaque = self.config.window_opacity() >= 1.;

        // Disable shadows for transparent windows on macOS.
        #[cfg(target_os = "macos")]
        self.display.window.set_has_shadow(opaque);

        #[cfg(target_os = "macos")]
        self.display.window.set_option_as_alt(self.config.window.option_as_alt());

        // Change opacity and blur state.
        self.display.window.set_transparent(!opaque);
        self.display.window.set_blur(self.config.window.blur);
        #[cfg(target_os = "macos")]
        self.display.window.set_titlebar_color(self.config.colors.primary.background);

        // Update hint keys.
        self.display.hint_state.update_alphabet(self.config.hints.alphabet());

        // Update cursor blinking.
        let event = Event::new(TerminalEvent::CursorBlinkingChange.into(), None);
        self.event_queue.push(event.into());
        self.layout_dirty = true;
        self.dirty = true;
    }

    /// Get reference to the window's configuration.
    #[cfg(unix)]
    pub fn config(&self) -> &UiConfig {
        &self.config
    }

    /// Clear the window config overrides.
    #[cfg(unix)]
    pub fn reset_window_config(&mut self, config: Rc<UiConfig>) {
        // Clear previous window errors.
        self.message_buffer.remove_target(LOG_TARGET_IPC_CONFIG);

        self.window_config.clear();

        // Reload current config to pull new IPC config.
        self.update_config(config);
    }

    /// Add new window config overrides.
    #[cfg(unix)]
    pub fn add_window_config(&mut self, config: Rc<UiConfig>, options: &ParsedOptions) {
        // Clear previous window errors.
        self.message_buffer.remove_target(LOG_TARGET_IPC_CONFIG);

        self.window_config.extend_from_slice(options);

        // Reload current config to pull new IPC config.
        self.update_config(config);
    }

    fn bounds(&self) -> Rect {
        Rect {
            width: self.display.window_size.width as f32,
            height: self.display.window_size.height as f32,
            ..Rect::default()
        }
    }

    fn gap(&self) -> f32 {
        (4. * self.display.window.scale_factor as f32).round()
    }

    fn rects(&self) -> Vec<(PaneId, Rect)> {
        self.layout.rects(self.bounds(), self.gap())
    }

    fn minimum_pane_size(&self) -> (f32, f32) {
        let size = self.display.size_info;
        (
            size.cell_width() * 12. + 2. * size.padding_x(),
            size.cell_height() * 4. + 2. * size.padding_y(),
        )
    }

    /// Temporarily load a pane for processing/rendering without changing keyboard focus.
    fn load_pane(&mut self, id: PaneId) -> bool {
        if self.pane.id != id {
            let Some(index) = self.inactive.iter().position(|(pane, _)| pane.id == id) else {
                return false;
            };
            let (pane, display) = &mut self.inactive[index];
            mem::swap(&mut self.pane, pane);
            self.display.swap_pane(display);
        }
        if let Some((_, rect)) = self.rects().into_iter().find(|(pane, _)| *pane == id) {
            self.display.window.pane_offset = (rect.x, rect.y);
        }
        true
    }

    fn relayout(&mut self) {
        self.layout_dirty = false;
        let active = self.pane.id;
        let cell = (self.display.size_info.cell_width(), self.display.size_info.cell_height());
        self.display.composite = !self.inactive.is_empty();
        for (id, rect) in self.rects() {
            self.load_pane(id);
            self.display.resize_pane(rect, cell, &self.config);
            let mut terminal = self.pane.terminal.lock();
            let searching = self.pane.search_state.history_index.is_some();
            Self::submit_display_update(
                &mut terminal,
                &mut self.display,
                &mut self.pane.notifier,
                &self.message_buffer,
                &mut self.pane.search_state,
                searching,
                &self.config,
            );
        }
        self.load_pane(active);
        self.dirty = true;
    }

    /// Returns true if this exit was handled without closing the native tab.
    pub fn remove_exited_pane(&mut self, id: Option<PaneId>, scheduler: &mut Scheduler) -> bool {
        let id = id.unwrap_or(self.pane.id);
        if id != self.pane.id && !self.inactive.iter().any(|(p, _)| p.id == id) {
            return true;
        }
        if self.inactive.is_empty() {
            return false;
        }
        let active = self.pane.id;
        if active == id {
            let rects = self.rects();
            let index = rects.iter().position(|(p, _)| *p == id).unwrap();
            self.load_pane(rects[(index + 1) % rects.len()].0);
        }
        debug!("Closed pane {id}");
        self.inactive.retain(|(pane, _)| pane.id != id);
        self.layout.remove(id);
        self.divider_drag = None;
        scheduler.unschedule_pane(self.id(), id);
        self.pane.terminal.lock().is_focused = self.focused;
        if active == id {
            self.display.window.set_title(self.pane.title.clone());
            self.event_queue.push(WinitEvent::WindowEvent {
                window_id: self.id(),
                event: WindowEvent::Focused(self.focused),
            });
        }
        self.relayout();
        self.display.window.request_redraw();
        true
    }

    fn focus_pane(
        &mut self,
        id: PaneId,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
    ) {
        if id == self.pane.id {
            return;
        }
        if !self.inactive.iter().any(|(p, _)| p.id == id) {
            return;
        }
        self.process_event(
            #[cfg(target_os = "macos")]
            event_loop,
            proxy,
            clipboard,
            scheduler,
            WinitEvent::WindowEvent { window_id: self.id(), event: WindowEvent::Focused(false) },
        );
        scheduler.unschedule(crate::scheduler::TimerId::for_pane(
            crate::scheduler::Topic::SelectionScrolling,
            self.id(),
            self.pane.id,
        ));
        self.pane.mouse.left_button_state = ElementState::Released;
        self.pane.mouse.middle_button_state = ElementState::Released;
        self.pane.mouse.right_button_state = ElementState::Released;
        self.display.ime.set_preedit(None);
        self.load_pane(id);
        debug!("Focused pane {id}");
        self.display.window.set_title(self.pane.title.clone());
        self.process_event(
            #[cfg(target_os = "macos")]
            event_loop,
            proxy,
            clipboard,
            scheduler,
            WinitEvent::WindowEvent {
                window_id: self.id(),
                event: WindowEvent::Focused(self.focused),
            },
        );
        self.dirty = true;
    }

    fn pane_command(
        &mut self,
        command: PaneCommand,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
    ) {
        match command {
            PaneCommand::Split(axis) => {
                let Some((_, rect)) = self.rects().into_iter().find(|(id, _)| *id == self.pane.id)
                else {
                    return;
                };
                let minimum = self.minimum_pane_size();
                let enough = match axis {
                    Axis::Horizontal => rect.width >= 2. * minimum.0 + self.gap(),
                    Axis::Vertical => rect.height >= 2. * minimum.1 + self.gap(),
                };
                if !enough {
                    return;
                }
                #[allow(unused_mut)]
                let mut options = WindowOptions::default();
                #[cfg(not(windows))]
                {
                    options.terminal_options.working_directory =
                        crate::daemon::foreground_process_path(
                            self.pane.master_fd,
                            self.pane.shell_pid,
                        )
                        .ok();
                }
                let id = self.next_pane_id;
                let pane = match Pane::new(
                    id,
                    self.display.size_info,
                    self.id(),
                    &self.config,
                    options,
                    proxy.clone(),
                ) {
                    Ok(pane) => pane,
                    Err(err) => {
                        error!("Could not create split pane: {err}");
                        return;
                    },
                };
                debug!("Created pane {id} in {:?}", self.id());
                self.next_pane_id += 1;
                pane.terminal.lock().is_focused = false;
                self.inactive.push((pane, PaneDisplay::new(self.display.size_info, &self.config)));
                self.layout.split(self.pane.id, id, axis);
                self.relayout();
                self.focus_pane(
                    id,
                    #[cfg(target_os = "macos")]
                    event_loop,
                    proxy,
                    clipboard,
                    scheduler,
                );
            },
            PaneCommand::Next | PaneCommand::Previous => {
                let rects = self.rects();
                let index = rects.iter().position(|(id, _)| *id == self.pane.id).unwrap();
                let delta = if matches!(command, PaneCommand::Next) { 1 } else { rects.len() - 1 };
                let id = rects[(index + delta) % rects.len()].0;
                self.focus_pane(
                    id,
                    #[cfg(target_os = "macos")]
                    event_loop,
                    proxy,
                    clipboard,
                    scheduler,
                );
            },
            PaneCommand::Close => {
                if !self.remove_exited_pane(Some(self.pane.id), scheduler) {
                    self.display.window.hold = false;
                    self.pane.terminal.lock().exit();
                }
            },
            PaneCommand::CloseTab => {
                self.display.window.hold = false;
                self.pane.terminal.lock().exit();
                for (pane, _) in &self.inactive {
                    pane.terminal.lock().exit();
                }
            },
        }
    }

    /// Route native input and PTY events to a pane, keeping all GL work in draw().
    pub fn handle_event(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        event: WinitEvent<Event>,
    ) {
        let redraw =
            matches!(event, WinitEvent::WindowEvent { event: WindowEvent::RedrawRequested, .. });
        if !redraw && !matches!(event, WinitEvent::AboutToWait) {
            self.event_queue.push(event);
            return;
        }
        for mut event in mem::take(&mut self.event_queue) {
            let active = self.pane.id;
            let mut restore_active = matches!(event, WinitEvent::UserEvent(_));
            if let WinitEvent::UserEvent(user) = &event {
                let id = user.pane_id.unwrap_or(active);
                if !self.load_pane(id) {
                    continue;
                }
                if let crate::event::EventType::Pane(command) = user.payload {
                    self.pane_command(
                        command,
                        #[cfg(target_os = "macos")]
                        event_loop,
                        proxy,
                        clipboard,
                        scheduler,
                    );
                    continue;
                }
                if let crate::event::EventType::Terminal(terminal_event) = &user.payload {
                    match terminal_event {
                        TerminalEvent::Title(title)
                            if !self.pane.preserve_title && self.config.window.dynamic_title =>
                        {
                            self.pane.title = title.clone();
                            if id != active {
                                self.load_pane(active);
                                continue;
                            }
                        },
                        TerminalEvent::ResetTitle
                            if !self.pane.preserve_title && self.config.window.dynamic_title =>
                        {
                            self.pane.title = self.config.window.identity.title.clone();
                            if id != active {
                                self.load_pane(active);
                                continue;
                            }
                        },
                        TerminalEvent::MouseCursorDirty if id != active => {
                            self.load_pane(active);
                            continue;
                        },
                        _ => (),
                    }
                }
            }
            if let WinitEvent::WindowEvent { event: native, .. } = &mut event {
                match native {
                    WindowEvent::Resized(size) => {
                        if size.width > 0 && size.height > 0 {
                            self.display.window_size = *size;
                            self.relayout();
                        }
                        continue;
                    },
                    WindowEvent::CloseRequested => {
                        self.pane_command(
                            PaneCommand::CloseTab,
                            #[cfg(target_os = "macos")]
                            event_loop,
                            proxy,
                            clipboard,
                            scheduler,
                        );
                        continue;
                    },
                    WindowEvent::Focused(focused) => {
                        self.focused = *focused;
                    },
                    WindowEvent::CursorMoved { position, .. } => {
                        self.pointer = *position;
                        if let Some(path) = &self.divider_drag {
                            let (bounds, gap, minimum) =
                                (self.bounds(), self.gap(), self.minimum_pane_size());
                            self.layout.drag(
                                path,
                                bounds,
                                gap,
                                (position.x as f32, position.y as f32),
                                minimum,
                            );
                            self.relayout();
                            continue;
                        }
                        let pressed = self.pane.mouse.left_button_state == ElementState::Pressed
                            || self.pane.mouse.right_button_state == ElementState::Pressed;
                        if !pressed {
                            if let Some((_, axis)) = self.layout.divider_at(
                                self.bounds(),
                                self.gap(),
                                position.x as f32,
                                position.y as f32,
                            ) {
                                self.display.window.set_mouse_cursor(match axis {
                                    Axis::Horizontal => CursorIcon::ColResize,
                                    Axis::Vertical => CursorIcon::RowResize,
                                });
                                continue;
                            }
                        }
                        if !pressed {
                            if let Some((id, _)) = self
                                .rects()
                                .into_iter()
                                .find(|(_, r)| r.contains(position.x as f32, position.y as f32))
                            {
                                self.pane.mouse.inside_text_area = id == active;
                                self.load_pane(id);
                                restore_active = true;
                            }
                            let mouse_mode =
                                self.pane.terminal.lock().mode().intersects(TermMode::MOUSE_MODE);
                            self.display.window.set_mouse_cursor(if mouse_mode {
                                CursorIcon::Default
                            } else {
                                CursorIcon::Text
                            });
                        }
                        let rect =
                            self.rects().into_iter().find(|(id, _)| *id == self.pane.id).unwrap().1;
                        position.x -= rect.x as f64;
                        position.y -= rect.y as f64;
                    },
                    WindowEvent::MouseInput { state, button: MouseButton::Left, .. }
                        if *state == ElementState::Released && self.divider_drag.is_some() =>
                    {
                        self.divider_drag = None;
                        continue;
                    },
                    WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                        if let Some((path, _)) = self.layout.divider_at(
                            self.bounds(),
                            self.gap(),
                            self.pointer.x as f32,
                            self.pointer.y as f32,
                        ) {
                            if *button == MouseButton::Left {
                                self.divider_drag = Some(path);
                            }
                            continue;
                        }
                        if let Some((id, rect)) = self
                            .rects()
                            .into_iter()
                            .find(|(_, r)| r.contains(self.pointer.x as f32, self.pointer.y as f32))
                        {
                            self.focus_pane(
                                id,
                                #[cfg(target_os = "macos")]
                                event_loop,
                                proxy,
                                clipboard,
                                scheduler,
                            );
                            self.pane.mouse.x = (self.pointer.x - rect.x as f64).max(0.) as usize;
                            self.pane.mouse.y = (self.pointer.y - rect.y as f64).max(0.) as usize;
                            self.pane.mouse.inside_text_area = true;
                        }
                    },
                    WindowEvent::MouseWheel { .. } => {
                        if let Some((id, _)) = self
                            .rects()
                            .into_iter()
                            .find(|(_, r)| r.contains(self.pointer.x as f32, self.pointer.y as f32))
                        {
                            self.load_pane(id);
                            restore_active = true;
                        } else {
                            continue;
                        }
                    },
                    WindowEvent::Touch(touch) => {
                        let rect =
                            self.rects().into_iter().find(|(id, _)| *id == self.pane.id).unwrap().1;
                        touch.location.x -= rect.x as f64;
                        touch.location.y -= rect.y as f64;
                    },
                    _ => (),
                }
            }
            let metrics =
                (self.display.size_info.cell_width(), self.display.size_info.cell_height());
            self.process_event(
                #[cfg(target_os = "macos")]
                event_loop,
                proxy,
                clipboard,
                scheduler,
                event,
            );
            if metrics
                != (self.display.size_info.cell_width(), self.display.size_info.cell_height())
            {
                self.relayout();
            }
            if restore_active {
                self.load_pane(active);
            }
        }
        if self.layout_dirty {
            self.relayout();
        }
        if self.dirty && self.display.window.has_frame && !self.occluded && !redraw {
            self.display.window.request_redraw();
        }
    }

    /// Draw the window.
    pub fn draw(&mut self, scheduler: &mut Scheduler) {
        self.display.window.requested_redraw = false;
        if self.occluded {
            return;
        }
        self.dirty = false;
        self.display.process_renderer_update();
        self.display.begin_panes(&self.config);
        let active = self.pane.id;
        for (id, rect) in self.rects() {
            self.load_pane(id);
            self.display.pane_viewport(rect);
            if !self.display.visual_bell.completed() {
                self.dirty = true;
            }
            let terminal = self.pane.terminal.lock();
            let next_animation = self.display.draw(
                terminal,
                &self.message_buffer,
                &self.config,
                &mut self.pane.search_state,
                id == active,
            );
            let timer = crate::scheduler::TimerId::for_pane(
                crate::scheduler::Topic::GraphicsAnimation,
                self.id(),
                id,
            );
            scheduler.unschedule(timer);
            if let Some(deadline) = next_animation {
                let event =
                    Event::new(crate::event::EventType::GraphicsAnimation, self.id()).with_pane(id);
                scheduler.schedule(
                    event,
                    deadline.saturating_duration_since(Instant::now()),
                    false,
                    timer,
                );
            }
        }
        self.load_pane(active);
        self.display.present(scheduler);
    }

    /// Process events for this terminal window.
    fn process_event(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        event_proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        event: WinitEvent<Event>,
    ) {
        let mut terminal = self.pane.terminal.lock();

        let old_is_searching = self.pane.search_state.history_index.is_some();

        let context = ActionContext {
            pane_id: self.pane.id,
            kitty_clipboard: &mut self.pane.kitty_clipboard,
            cursor_blink_timed_out: &mut self.pane.cursor_blink_timed_out,
            prev_bell_cmd: &mut self.pane.prev_bell_cmd,
            message_buffer: &mut self.message_buffer,
            inline_search_state: &mut self.pane.inline_search_state,
            search_state: &mut self.pane.search_state,
            modifiers: &mut self.modifiers,
            notifier: &mut self.pane.notifier,
            display: &mut self.display,
            mouse: &mut self.pane.mouse,
            touch: &mut self.pane.touch,
            dirty: &mut self.dirty,
            occluded: &mut self.occluded,
            terminal: &mut terminal,
            #[cfg(not(windows))]
            master_fd: self.pane.master_fd,
            #[cfg(not(windows))]
            shell_pid: self.pane.shell_pid,
            preserve_title: self.pane.preserve_title,
            config: &self.config,
            event_proxy,
            #[cfg(target_os = "macos")]
            event_loop,
            clipboard,
            scheduler,
        };
        let mut processor = input::Processor::new(context);

        processor.handle_event(event);

        // Process DisplayUpdate events.
        if self.display.pending_update.dirty {
            self.layout_dirty = true;
            Self::submit_display_update(
                &mut terminal,
                &mut self.display,
                &mut self.pane.notifier,
                &self.message_buffer,
                &mut self.pane.search_state,
                old_is_searching,
                &self.config,
            );
            self.dirty = true;
        }

        if self.dirty || self.pane.mouse.hint_highlight_dirty {
            self.dirty |= self.display.update_highlighted_hints(
                &terminal,
                &self.config,
                &self.pane.mouse,
                self.modifiers.state(),
            );
            self.pane.mouse.hint_highlight_dirty = false;
        }
    }

    /// ID of this terminal context.
    pub fn id(&self) -> WindowId {
        self.display.window.id()
    }

    /// Write the ref test results to the disk.
    pub fn write_ref_test_results(&self) {
        // Dump grid state.
        let mut grid = self.pane.terminal.lock().grid().clone();
        grid.initialize_all();
        grid.truncate();

        let serialized_grid = json::to_string(&grid).expect("serialize grid");

        let size_info = &self.display.size_info;
        let size = TermSize::new(size_info.columns(), size_info.screen_lines());
        let serialized_size = json::to_string(&size).expect("serialize size");

        let serialized_config = format!("{{\"history_size\":{}}}", grid.history_size());

        File::create("./grid.json")
            .and_then(|mut f| f.write_all(serialized_grid.as_bytes()))
            .expect("write grid.json");

        File::create("./size.json")
            .and_then(|mut f| f.write_all(serialized_size.as_bytes()))
            .expect("write size.json");

        File::create("./config.json")
            .and_then(|mut f| f.write_all(serialized_config.as_bytes()))
            .expect("write config.json");
    }

    /// Submit the pending changes to the `Display`.
    fn submit_display_update(
        terminal: &mut Term<EventProxy>,
        display: &mut Display,
        notifier: &mut Notifier,
        message_buffer: &MessageBuffer,
        search_state: &mut SearchState,
        old_is_searching: bool,
        config: &UiConfig,
    ) {
        // Compute cursor positions before resize.
        let num_lines = terminal.screen_lines();
        let cursor_at_bottom = terminal.grid().cursor.point.line + 1 == num_lines;
        let origin_at_bottom = if terminal.mode().contains(TermMode::VI) {
            terminal.vi_mode_cursor.point.line == num_lines - 1
        } else {
            search_state.direction == Direction::Left
        };

        display.handle_update(terminal, notifier, message_buffer, search_state, config);

        let new_is_searching = search_state.history_index.is_some();
        if !old_is_searching && new_is_searching {
            // Scroll on search start to make sure origin is visible with minimal viewport motion.
            let display_offset = terminal.grid().display_offset();
            if display_offset == 0 && cursor_at_bottom && !origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(1));
            } else if display_offset != 0 && origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(-1));
            }
        }
    }
}

impl Pane {
    fn new(
        id: PaneId,
        size_info: SizeInfo,
        window_id: WindowId,
        config: &UiConfig,
        options: WindowOptions,
        proxy: EventLoopProxy<Event>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut pty_config = config.pty_config();
        options.terminal_options.override_pty_config(&mut pty_config);

        let preserve_title = options.window_identity.title.is_some();

        info!("PTY dimensions: {:?} x {:?}", size_info.screen_lines(), size_info.columns());

        let event_proxy = EventProxy::new(proxy, window_id).with_pane(id);

        // Create the terminal.
        //
        // This object contains all of the state about what's being displayed. It's
        // wrapped in a clonable mutex since both the I/O loop and display need to
        // access it.
        let mut terminal = Term::new(config.term_options(), &size_info, event_proxy.clone());
        terminal.set_graphics_cell_size(size_info.cell_width(), size_info.cell_height());
        let terminal = Arc::new(FairMutex::new(terminal));

        // Create the PTY.
        //
        // The PTY forks a process to run the shell on the slave side of the
        // pseudoterminal. A file descriptor for the master side is retained for
        // reading/writing to the shell.
        let pty = tty::new(&pty_config, size_info.into(), window_id.into())?;

        #[cfg(not(windows))]
        let master_fd = pty.file().as_raw_fd();
        #[cfg(not(windows))]
        let shell_pid = pty.child().id();

        // Create the pseudoterminal I/O loop.
        //
        // PTY I/O is ran on another thread as to not occupy cycles used by the
        // renderer and input processing. Note that access to the terminal state is
        // synchronized since the I/O loop updates the state, and the display
        // consumes it periodically.
        let event_loop = PtyEventLoop::new(
            Arc::clone(&terminal),
            event_proxy.clone(),
            pty,
            pty_config.drain_on_exit,
            config.debug.ref_test,
        )?;

        // The event loop channel allows write requests from the event processor
        // to be sent to the pty loop and ultimately written to the pty.
        let loop_tx = event_loop.channel();

        // Kick off the I/O thread.
        let _io_thread = event_loop.spawn();

        // Start cursor blinking, in case `Focused` isn't sent on startup.
        if config.cursor.style().blinking {
            event_proxy.send_event(TerminalEvent::CursorBlinkingChange.into());
        }

        Ok(Self {
            id,
            kitty_clipboard: Default::default(),
            title: options
                .window_identity
                .title
                .unwrap_or_else(|| config.window.identity.title.clone()),
            preserve_title,
            terminal,
            #[cfg(not(windows))]
            master_fd,
            #[cfg(not(windows))]
            shell_pid,
            notifier: Notifier(loop_tx),
            cursor_blink_timed_out: false,
            prev_bell_cmd: None,
            inline_search_state: Default::default(),
            search_state: Default::default(),
            mouse: Default::default(),
            touch: Default::default(),
        })
    }
}
