//! Terminal window context.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::Write;
use std::mem;
use std::rc::Rc;
use std::time::Instant;

use glutin::config::Config as GlutinConfig;
use glutin::display::GetGlDisplay;
#[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
use glutin::platform::x11::X11GlConfigExt;
use serde_json as json;
use winit::event::{ElementState, Event as WinitEvent, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{CursorIcon, WindowId};

use alacritty_terminal::event::{Event as TerminalEvent, OnResize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Term, TermMode};

use crate::cli::{ParsedOptions, WindowOptions};
use crate::clipboard::Clipboard;
use crate::config::UiConfig;
#[cfg(not(windows))]
use crate::daemon::foreground_process_path;
use crate::display::window::Window;
use crate::display::{Display, SizeInfo};
use crate::event::{ActionContext, Event, Mouse, SearchState, TouchPurpose};
use crate::layout::{
    Axis, FocusDirection, Hit, Layout, LayoutGeometry, LayoutMetrics, PaneId, Rect, SplitId,
};
#[cfg(unix)]
use crate::logging::LOG_TARGET_IPC_CONFIG;
use crate::message_bar::MessageBuffer;
use crate::pane::Pane;
use crate::scheduler::Scheduler;
use crate::{input, renderer};

/// Pending layout action produced by a keybinding.
#[derive(Debug, Clone, Copy)]
pub enum PendingPaneOp {
    Split(Axis),
    Close,
    Focus(FocusDirection),
    QuitWindow,
}

/// Active split-bar drag.
#[derive(Debug, Clone, Copy)]
struct SplitDrag {
    id: SplitId,
    axis: Axis,
}

/// Event context for one individual Alacritty window.
pub struct WindowContext {
    pub display: Display,
    pub dirty: bool,
    pub closing: bool,
    event_queue: Vec<WinitEvent<Event>>,
    panes: HashMap<PaneId, Pane>,
    layout: Layout,
    geometry: LayoutGeometry,
    focused: PaneId,
    split_drag: Option<SplitDrag>,
    pending_pane_op: Option<PendingPaneOp>,
    prev_bell_cmd: Option<Instant>,
    modifiers: Modifiers,
    mouse: Mouse,
    touch: TouchPurpose,
    occluded: bool,
    preserve_title: bool,
    window_config: ParsedOptions,
    config: Rc<UiConfig>,
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

        let raw_window_handle = window.raw_window_handle();
        let gl_context =
            renderer::platform::create_gl_context(&gl_display, gl_config, Some(raw_window_handle))?;

        let display = Display::new(window, gl_context, &config, tabbed)?;

        let mut window_context = Self::new(display, config, options, proxy)?;
        window_context.window_config = config_overrides;

        Ok(window_context)
    }

    fn new(
        display: Display,
        config: Rc<UiConfig>,
        options: WindowOptions,
        proxy: EventLoopProxy<Event>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut pty_config = config.pty_config();
        options.terminal_options.override_pty_config(&mut pty_config);

        let preserve_title = options.window_identity.title.is_some();

        let (layout, pane_id) = Layout::new();
        let bounds = content_bounds(&display.size_info);
        let size = SizeInfo::for_rect(
            bounds.width,
            bounds.height,
            display.size_info.cell_width(),
            display.size_info.cell_height(),
        );

        let pane = Pane::spawn(pane_id, &config, size, display.window.id(), proxy, pty_config)?;
        pane.terminal.lock().is_focused = true;

        let mut panes = HashMap::new();
        panes.insert(pane_id, pane);

        let mut window_context = Self {
            preserve_title,
            display,
            panes,
            layout,
            geometry: LayoutGeometry { leaves: Vec::new(), bars: Vec::new() },
            focused: pane_id,
            split_drag: None,
            pending_pane_op: None,
            closing: false,
            config,
            prev_bell_cmd: Default::default(),
            window_config: Default::default(),
            event_queue: Default::default(),
            modifiers: Default::default(),
            occluded: Default::default(),
            mouse: Default::default(),
            touch: Default::default(),
            dirty: Default::default(),
        };
        window_context.relayout_panes();

        Ok(window_context)
    }

    pub fn update_config(&mut self, new_config: Rc<UiConfig>) {
        let old_config = mem::replace(&mut self.config, new_config);

        self.config = self.window_config.override_config_rc(self.config.clone());

        self.display.update_config(&self.config);
        for pane in self.panes.values_mut() {
            pane.terminal.lock().set_options(self.config.term_options());
        }

        if (old_config.cursor.thickness() - self.config.cursor.thickness()).abs() > f32::EPSILON {
            self.display.pending_update.set_cursor_dirty();
        }

        if old_config.font != self.config.font {
            let scale_factor = self.display.window.scale_factor as f32;
            if self.display.font_size == old_config.font.size().scale(scale_factor) {
                self.display.font_size = self.config.font.size().scale(scale_factor);
            }

            let font = self.config.font.clone().with_size(self.display.font_size);
            self.display.pending_update.set_font(font);
        }

        self.display.window.set_theme(self.config.window.theme());

        let window_config = &old_config.window;
        if window_config.padding(1.) != self.config.window.padding(1.)
            || window_config.dynamic_padding != self.config.window.dynamic_padding
            || window_config.resize_increments != self.config.window.resize_increments
        {
            self.display.pending_update.dirty = true;
        }

        if !self.preserve_title
            && (!self.config.window.dynamic_title
                || self.display.window.title() == old_config.window.identity.title)
        {
            self.display.window.set_title(self.config.window.identity.title.clone());
        }

        let opaque = self.config.window_opacity() >= 1.;

        #[cfg(target_os = "macos")]
        self.display.window.set_has_shadow(opaque);

        #[cfg(target_os = "macos")]
        self.display.window.set_option_as_alt(self.config.window.option_as_alt());

        self.display.window.set_transparent(!opaque);
        self.display.window.set_blur(self.config.window.blur);

        self.display.hint_state.update_alphabet(self.config.hints.alphabet());

        let event = Event::new(TerminalEvent::CursorBlinkingChange.into(), None);
        self.event_queue.push(event.into());

        self.dirty = true;
    }

    #[cfg(unix)]
    pub fn config(&self) -> &UiConfig {
        &self.config
    }

    pub fn has_messages(&self) -> bool {
        self.panes.values().any(|pane| !pane.message_buffer.is_empty())
    }

    pub fn remove_message_target(&mut self, target: &str) {
        for pane in self.panes.values_mut() {
            pane.message_buffer.remove_target(target);
        }
    }

    #[cfg(unix)]
    pub fn reset_window_config(&mut self, config: Rc<UiConfig>) {
        self.remove_message_target(LOG_TARGET_IPC_CONFIG);
        self.window_config.clear();
        self.update_config(config);
    }

    #[cfg(unix)]
    pub fn add_window_config(&mut self, config: Rc<UiConfig>, options: &ParsedOptions) {
        self.remove_message_target(LOG_TARGET_IPC_CONFIG);
        self.window_config.extend_from_slice(options);
        self.update_config(config);
    }

    /// Close a pane. Returns `true` if the window still has remaining panes.
    pub fn close_pane(&mut self, pane_id: Option<PaneId>) -> bool {
        let id = pane_id.unwrap_or(self.focused);
        match self.layout.close(id) {
            Some(focus) => {
                self.panes.remove(&id);
                self.focused = focus;
                self.split_drag = None;
                if let Some(pane) = self.panes.get(&focus) {
                    pane.terminal.lock().is_focused = true;
                }
                self.relayout_panes();
                true
            },
            None => false,
        }
    }

    pub fn draw(&mut self, scheduler: &mut Scheduler) {
        self.display.window.requested_redraw = false;

        if self.occluded {
            return;
        }

        self.dirty = false;

        self.display.process_renderer_update();

        if !self.display.visual_bell.completed() {
            if self.display.window.has_frame {
                self.display.window.request_redraw();
            } else {
                self.dirty = true;
            }
        }

        let window_height = self.display.size_info.height();
        self.display.begin_frame(&self.config);

        let geometry = self.geometry.clone();
        for leaf in &geometry.leaves {
            let Some(pane) = self.panes.get_mut(&leaf.id) else {
                continue;
            };
            let terminal = pane.terminal.lock();
            self.display.draw(
                terminal,
                &pane.message_buffer,
                &self.config,
                &mut pane.search_state,
                pane.size,
                leaf.rect,
                window_height,
            );
        }

        self.display.paint_split_bars(&geometry.bars, &self.config);
        self.display.end_frame(scheduler, &self.config);
    }

    pub fn handle_event(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        event_proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        event: WinitEvent<Event>,
    ) {
        match event {
            WinitEvent::AboutToWait
            | WinitEvent::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                if self.event_queue.is_empty() {
                    return;
                }
            },
            event => {
                self.event_queue.push(event);
                return;
            },
        }

        let events: Vec<_> = self.event_queue.drain(..).collect();
        for event in events {
            if self.consume_layout_event(&event) {
                continue;
            }

            self.process_terminal_event(
                #[cfg(target_os = "macos")]
                event_loop,
                event_proxy,
                clipboard,
                scheduler,
                event,
            );

            self.apply_pending_op(event_proxy);
        }

        if self.dirty
            && self.display.window.has_frame
            && !self.occluded
            && !matches!(event, WinitEvent::WindowEvent { event: WindowEvent::RedrawRequested, .. })
        {
            self.display.window.request_redraw();
        }
    }

    fn process_terminal_event(
        &mut self,
        #[cfg(target_os = "macos")] event_loop: &ActiveEventLoop,
        event_proxy: &EventLoopProxy<Event>,
        clipboard: &mut Clipboard,
        scheduler: &mut Scheduler,
        event: WinitEvent<Event>,
    ) {
        let focused = self.focused;
        let Some(leaf) = self.geometry.leaf(focused).copied() else {
            return;
        };
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };

        let mut terminal = pane.terminal.lock();
        let old_is_searching = pane.search_state.history_index.is_some();
        let input_size = pane.size.with_origin(leaf.rect.x, leaf.rect.y);

        {
            let context = ActionContext {
                cursor_blink_timed_out: &mut pane.cursor_blink_timed_out,
                prev_bell_cmd: &mut self.prev_bell_cmd,
                message_buffer: &mut pane.message_buffer,
                inline_search_state: &mut pane.inline_search_state,
                search_state: &mut pane.search_state,
                modifiers: &mut self.modifiers,
                notifier: &mut pane.notifier,
                display: &mut self.display,
                mouse: &mut self.mouse,
                touch: &mut self.touch,
                dirty: &mut self.dirty,
                occluded: &mut self.occluded,
                terminal: &mut terminal,
                pending_pane_op: &mut self.pending_pane_op,
                input_size,
                #[cfg(not(windows))]
                master_fd: pane.master_fd,
                #[cfg(not(windows))]
                shell_pid: pane.shell_pid,
                preserve_title: self.preserve_title,
                config: &self.config,
                event_proxy,
                #[cfg(target_os = "macos")]
                event_loop,
                clipboard,
                scheduler,
            };
            let mut processor = input::Processor::new(context);
            processor.handle_event(event);
        }

        if self.display.pending_update.dirty {
            Self::submit_display_update(
                &mut terminal,
                &mut self.display,
                &mut pane.notifier,
                &pane.message_buffer,
                &mut pane.search_state,
                old_is_searching,
                &self.config,
            );
            drop(terminal);
            self.relayout_panes();
        } else {
            drop(terminal);
            if old_is_searching
                != self
                    .panes
                    .get(&focused)
                    .is_some_and(|pane| pane.search_state.history_index.is_some())
            {
                self.relayout_panes();
            }
        }

        if let Some(pane) = self.panes.get_mut(&focused) {
            let terminal = pane.terminal.lock();
            if self.dirty || self.mouse.hint_highlight_dirty {
                let saved = self.display.size_info;
                self.display.size_info = input_size;
                self.dirty |= self.display.update_highlighted_hints(
                    &terminal,
                    &self.config,
                    &self.mouse,
                    self.modifiers.state(),
                );
                self.display.size_info = saved;
                self.mouse.hint_highlight_dirty = false;
            }
        }
    }

    fn consume_layout_event(&mut self, event: &WinitEvent<Event>) -> bool {
        match event {
            WinitEvent::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. }, ..
            } => {
                let size_info = self.display.size_info;
                let (x, y): (i32, i32) = (*position).into();
                self.mouse.x = x.clamp(0, size_info.width() as i32 - 1) as usize;
                self.mouse.y = y.clamp(0, size_info.height() as i32 - 1) as usize;

                if let Some(drag) = self.split_drag {
                    let bounds = content_bounds(&self.display.size_info);
                    let metrics = self.layout_metrics();
                    if self.layout.drag_split(drag.id, bounds, metrics, x as f32, y as f32) {
                        self.relayout_panes();
                    }
                    self.display.window.set_mouse_cursor(resize_cursor(drag.axis));
                    return true;
                }

                match self.layout.hit_test(
                    content_bounds(&self.display.size_info),
                    self.layout_metrics(),
                    x as f32,
                    y as f32,
                ) {
                    Some(Hit::Bar { axis, .. }) => {
                        self.display.window.set_mouse_cursor(resize_cursor(axis));
                        true
                    },
                    _ => false,
                }
            },
            WinitEvent::WindowEvent {
                event: WindowEvent::MouseInput { state, button, .. },
                ..
            } => {
                if *button != MouseButton::Left {
                    return false;
                }

                self.mouse.left_button_state = *state;

                if *state == ElementState::Released {
                    if self.split_drag.take().is_some() {
                        self.relayout_panes();
                        return true;
                    }
                    return false;
                }

                match self.layout.hit_test(
                    content_bounds(&self.display.size_info),
                    self.layout_metrics(),
                    self.mouse.x as f32,
                    self.mouse.y as f32,
                ) {
                    Some(Hit::Bar { id, axis }) => {
                        self.split_drag = Some(SplitDrag { id, axis });
                        self.display.window.set_mouse_cursor(resize_cursor(axis));
                        true
                    },
                    Some(Hit::Pane(id)) => {
                        self.set_focused(id);
                        false
                    },
                    None => false,
                }
            },
            _ => false,
        }
    }

    fn apply_pending_op(&mut self, event_proxy: &EventLoopProxy<Event>) {
        match self.pending_pane_op.take() {
            Some(PendingPaneOp::Split(axis)) => {
                let _ = self.split_focused(axis, event_proxy.clone());
            },
            Some(PendingPaneOp::Close) => {
                if !self.close_pane(Some(self.focused)) {
                    self.closing = true;
                    if let Some(pane) = self.panes.get(&self.focused) {
                        pane.terminal.lock().exit();
                    }
                }
            },
            Some(PendingPaneOp::Focus(direction)) => {
                let bounds = content_bounds(&self.display.size_info);
                if let Some(id) =
                    self.layout.neighbor(bounds, self.layout_metrics(), self.focused, direction)
                {
                    self.set_focused(id);
                }
            },
            Some(PendingPaneOp::QuitWindow) => {
                self.closing = true;
                if let Some(pane) = self.panes.get(&self.focused) {
                    pane.terminal.lock().exit();
                }
            },
            None => (),
        }
    }

    fn split_focused(
        &mut self,
        axis: Axis,
        proxy: EventLoopProxy<Event>,
    ) -> Result<(), Box<dyn Error>> {
        let focused = self.focused;
        let Some(new_id) = self.layout.split(focused, axis) else {
            return Ok(());
        };

        let mut pty_config = self.config.pty_config();
        #[cfg(not(windows))]
        if let Some(pane) = self.panes.get(&focused) {
            pty_config.working_directory =
                foreground_process_path(pane.master_fd, pane.shell_pid).ok();
        }

        let bounds = content_bounds(&self.display.size_info);
        let geometry = self.layout.compute(bounds, self.layout_metrics());
        let leaf = geometry.leaf(new_id).copied();
        let size = leaf
            .map(|leaf| {
                SizeInfo::for_rect(
                    leaf.rect.width,
                    leaf.rect.height,
                    self.display.size_info.cell_width(),
                    self.display.size_info.cell_height(),
                )
            })
            .unwrap_or(self.display.size_info);

        let pane =
            Pane::spawn(new_id, &self.config, size, self.display.window.id(), proxy, pty_config)?;
        self.panes.insert(new_id, pane);
        self.set_focused(new_id);
        self.relayout_panes();
        Ok(())
    }

    fn set_focused(&mut self, id: PaneId) {
        if self.focused == id || !self.panes.contains_key(&id) {
            return;
        }

        if let Some(pane) = self.panes.get(&self.focused) {
            pane.terminal.lock().is_focused = false;
        }
        self.focused = id;
        if let Some(pane) = self.panes.get(&id) {
            pane.terminal.lock().is_focused = true;
        }
        self.dirty = true;
    }

    fn layout_metrics(&self) -> LayoutMetrics {
        LayoutMetrics::from_cell_size(
            self.display.size_info.cell_width(),
            self.display.size_info.cell_height(),
        )
    }

    fn relayout_panes(&mut self) {
        let bounds = content_bounds(&self.display.size_info);
        let metrics = self.layout_metrics();
        self.geometry = self.layout.compute(bounds, metrics);

        for leaf in &self.geometry.leaves {
            let Some(pane) = self.panes.get_mut(&leaf.id) else {
                continue;
            };

            let mut size = SizeInfo::for_rect(
                leaf.rect.width,
                leaf.rect.height,
                metrics.cell_width,
                metrics.cell_height,
            );
            let search_lines = usize::from(pane.search_state.history_index.is_some());
            let message_lines =
                pane.message_buffer.message().map_or(0, |message| message.text(&size).len());
            size.reserve_lines(search_lines + message_lines);

            if pane.size.screen_lines() != size.screen_lines()
                || pane.size.columns() != size.columns()
            {
                pane.notifier.on_resize(size.into());
                pane.terminal.lock().resize(size);
            }
            pane.size = size;
        }

        self.dirty = true;
    }

    pub fn id(&self) -> WindowId {
        self.display.window.id()
    }

    pub fn write_ref_test_results(&self) {
        let Some(pane) = self.panes.get(&self.focused) else {
            return;
        };
        let mut grid = pane.terminal.lock().grid().clone();
        grid.initialize_all();
        grid.truncate();

        let serialized_grid = json::to_string(&grid).expect("serialize grid");

        let size_info = &pane.size;
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

    fn submit_display_update(
        terminal: &mut Term<crate::event::EventProxy>,
        display: &mut Display,
        notifier: &mut alacritty_terminal::event_loop::Notifier,
        message_buffer: &MessageBuffer,
        search_state: &mut SearchState,
        old_is_searching: bool,
        config: &UiConfig,
    ) {
        let num_lines = terminal.screen_lines();
        let cursor_at_bottom = terminal.grid().cursor.point.line + 1 == num_lines;
        let origin_at_bottom = if terminal.mode().contains(TermMode::VI) {
            terminal.vi_mode_cursor.point.line == num_lines - 1
        } else {
            search_state.direction == Direction::Left
        };

        display.handle_update(terminal, notifier, message_buffer, search_state, config, false);

        let new_is_searching = search_state.history_index.is_some();
        if !old_is_searching && new_is_searching {
            let display_offset = terminal.grid().display_offset();
            if display_offset == 0 && cursor_at_bottom && !origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(1));
            } else if display_offset != 0 && origin_at_bottom {
                terminal.scroll_display(Scroll::Delta(-1));
            }
        }
    }
}

fn content_bounds(size: &SizeInfo) -> Rect {
    Rect {
        x: size.padding_x(),
        y: size.padding_y(),
        width: (size.width() - 2. * size.padding_x()).max(0.),
        height: (size.height() - 2. * size.padding_y()).max(0.),
    }
}

fn resize_cursor(axis: Axis) -> CursorIcon {
    match axis {
        Axis::Vertical => CursorIcon::ColResize,
        Axis::Horizontal => CursorIcon::RowResize,
    }
}
