//! Handle input from winit.
//!
//! Certain key combinations should send some escape sequence back to the PTY.
//! In order to figure that out, state about which modifier keys are pressed
//! needs to be tracked. Additionally, we need a bit of a state machine to
//! determine what to do when a non-modifier key is pressed.

use std::cmp::{Ordering, max, min};
use std::fmt::Debug;
use std::mem;
use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{
    ElementState, Modifiers, MouseButton, MouseScrollDelta, Touch as TouchEvent, TouchPhase,
};
use winit::keyboard::ModifiersState;
use winit::window::CursorIcon;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::{ClipboardType, TermMode};

use crate::app::scheduler::{TimerId, Topic};
use crate::app::{Event, EventType};
use crate::config::{BindingMode, MouseEvent};
use crate::platform::window::ImeInhibitor;
use crate::presentation::SizeInfo;
use crate::presentation::hints::HintMatch;
use crate::presentation::messages::{self as message_bar};

use crate::input::{ActionContext, Execute, FONT_SIZE_STEP, Processor};
use ahash::RandomState;
use alacritty_terminal::term;
use std::collections::HashSet;
const TOUCH_ZOOM_FACTOR: f32 = 0.01;

/// Interval for mouse scrolling during selection outside of the boundaries.
const SELECTION_SCROLLING_INTERVAL: Duration = Duration::from_millis(15);

/// Minimum number of pixels at the bottom/top where selection scrolling is performed.
const MIN_SELECTION_SCROLLING_HEIGHT: f64 = 5.;

/// Number of pixels for increasing the selection scrolling speed factor by one.
const SELECTION_SCROLLING_STEP: f64 = 20.;

/// Distance before a touch input is considered a drag.
const MAX_TAP_DISTANCE: f64 = 20.;

/// Threshold used for double_click/triple_click.
pub(super) const CLICK_THRESHOLD: Duration = Duration::from_millis(400);

/// Identified purpose of the touch input.
#[derive(Default, Debug)]
pub enum TouchPurpose {
    #[default]
    None,
    Select(TouchEvent),
    Scroll(TouchEvent),
    Zoom(TouchZoom),
    ZoomPendingSlot(TouchEvent),
    Tap(TouchEvent),
    Invalid(HashSet<u64, RandomState>),
}

/// Touch zooming state.
#[derive(Debug)]
pub struct TouchZoom {
    slots: (TouchEvent, TouchEvent),
    fractions: f32,
}

impl TouchZoom {
    pub fn new(slots: (TouchEvent, TouchEvent)) -> Self {
        Self { slots, fractions: Default::default() }
    }

    /// Get slot distance change since last update.
    pub fn font_delta(&mut self, slot: TouchEvent) -> f32 {
        let old_distance = self.distance();

        // Update touch slots.
        if slot.id == self.slots.0.id {
            self.slots.0 = slot;
        } else {
            self.slots.1 = slot;
        }

        // Calculate font change in `FONT_SIZE_STEP` increments.
        let delta = (self.distance() - old_distance) * TOUCH_ZOOM_FACTOR + self.fractions;
        let font_delta = (delta.abs() / FONT_SIZE_STEP).floor() * FONT_SIZE_STEP * delta.signum();
        self.fractions = delta - font_delta;

        font_delta
    }

    /// Get active touch slots.
    pub fn slots(&self) -> (TouchEvent, TouchEvent) {
        self.slots
    }

    /// Calculate distance between slots.
    fn distance(&self) -> f32 {
        let delta_x = self.slots.0.location.x - self.slots.1.location.x;
        let delta_y = self.slots.0.location.y - self.slots.1.location.y;
        delta_x.hypot(delta_y) as f32
    }
}

/// State of the mouse.
#[derive(Debug)]
pub struct Mouse {
    pub left_button_state: ElementState,
    pub middle_button_state: ElementState,
    pub right_button_state: ElementState,
    pub last_click_timestamp: Instant,
    pub last_click_button: MouseButton,
    pub click_state: ClickState,
    pub accumulated_scroll: AccumulatedScroll,
    pub cell_side: Side,
    pub block_hint_launcher: bool,
    pub hint_highlight_dirty: bool,
    pub inside_text_area: bool,
    pub x: usize,
    pub y: usize,
}

impl Default for Mouse {
    fn default() -> Mouse {
        Mouse {
            last_click_timestamp: Instant::now(),
            last_click_button: MouseButton::Left,
            left_button_state: ElementState::Released,
            middle_button_state: ElementState::Released,
            right_button_state: ElementState::Released,
            click_state: ClickState::None,
            cell_side: Side::Left,
            hint_highlight_dirty: Default::default(),
            block_hint_launcher: Default::default(),
            inside_text_area: Default::default(),
            accumulated_scroll: Default::default(),
            x: Default::default(),
            y: Default::default(),
        }
    }
}

impl Mouse {
    /// Convert mouse pixel coordinates to viewport point.
    ///
    /// If the coordinates are outside of the terminal grid, like positions inside the padding, the
    /// coordinates will be clamped to the closest grid coordinates.
    #[inline]
    pub fn point(&self, size: &SizeInfo, display_offset: usize) -> Point {
        let col = self.x.saturating_sub(size.padding_x() as usize) / (size.cell_width() as usize);
        let col = min(Column(col), size.last_column());

        let line = self.y.saturating_sub(size.padding_y() as usize) / (size.cell_height() as usize);
        let line = min(line, size.bottommost_line().0 as usize);

        term::viewport_to_point(display_offset, Point::new(line, col))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ClickState {
    None,
    Click,
    DoubleClick,
    TripleClick,
}

/// The amount of scroll accumulated from the pointer events.
#[derive(Default, Debug)]
pub struct AccumulatedScroll {
    /// Scroll we should perform along `x` axis.
    pub x: f64,

    /// Scroll we should perform along `y` axis.
    pub y: f64,
}

impl<T: EventListener, A: ActionContext<T>> Processor<T, A> {
    pub fn new(ctx: A) -> Self {
        Self { ctx, _phantom: Default::default() }
    }

    #[inline]
    pub fn mouse_moved(&mut self, position: PhysicalPosition<f64>) {
        let size_info = self.ctx.size_info();

        let (x, y) = position.into();

        let lmb_pressed = self.ctx.mouse().left_button_state == ElementState::Pressed;
        let rmb_pressed = self.ctx.mouse().right_button_state == ElementState::Pressed;
        if !self.ctx.selection_is_empty() && (lmb_pressed || rmb_pressed) {
            self.update_selection_scrolling(y);
        }

        let display_offset = self.ctx.terminal().grid().display_offset();
        let old_point = self.ctx.mouse().point(&size_info, display_offset);

        let x = x.clamp(0, size_info.width() as i32 - 1) as usize;
        let y = y.clamp(0, size_info.height() as i32 - 1) as usize;
        self.ctx.mouse_mut().x = x;
        self.ctx.mouse_mut().y = y;

        let inside_text_area = size_info.contains_point(x, y);
        let cell_side = self.cell_side(x);

        let point = self.ctx.mouse().point(&size_info, display_offset);
        let cell_changed = old_point != point;

        // If the mouse hasn't changed cells, do nothing.
        if !cell_changed
            && self.ctx.mouse().cell_side == cell_side
            && self.ctx.mouse().inside_text_area == inside_text_area
        {
            return;
        }

        self.ctx.mouse_mut().inside_text_area = inside_text_area;
        self.ctx.mouse_mut().cell_side = cell_side;

        // Update mouse state and check for URL change.
        let mouse_state = self.cursor_state();
        self.ctx.window().set_mouse_cursor(mouse_state);

        // Prompt hint highlight update.
        self.ctx.mouse_mut().hint_highlight_dirty = true;

        // Don't launch URLs if mouse has moved.
        self.ctx.mouse_mut().block_hint_launcher = true;

        if (lmb_pressed || rmb_pressed)
            && (self.ctx.modifiers().state().shift_key() || !self.ctx.mouse_mode())
        {
            self.ctx.update_selection(point, cell_side);
        } else if cell_changed
            && self.ctx.terminal().mode().intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG)
        {
            if lmb_pressed {
                self.mouse_report(32, ElementState::Pressed);
            } else if self.ctx.mouse().middle_button_state == ElementState::Pressed {
                self.mouse_report(33, ElementState::Pressed);
            } else if self.ctx.mouse().right_button_state == ElementState::Pressed {
                self.mouse_report(34, ElementState::Pressed);
            } else if self.ctx.terminal().mode().contains(TermMode::MOUSE_MOTION) {
                self.mouse_report(35, ElementState::Pressed);
            }
        }
    }

    /// Check which side of a cell an X coordinate lies on.
    fn cell_side(&self, x: usize) -> Side {
        let size_info = self.ctx.size_info();

        let cell_x =
            x.saturating_sub(size_info.padding_x() as usize) % size_info.cell_width() as usize;
        let half_cell_width = (size_info.cell_width() / 2.0) as usize;

        let additional_padding =
            (size_info.width() - size_info.padding_x() * 2.) % size_info.cell_width();
        let end_of_grid = size_info.width() - size_info.padding_x() - additional_padding;

        if cell_x > half_cell_width
            // Edge case when mouse leaves the window.
            || x as f32 >= end_of_grid
        {
            Side::Right
        } else {
            Side::Left
        }
    }

    fn mouse_report(&mut self, button: u8, state: ElementState) {
        let display_offset = self.ctx.terminal().grid().display_offset();
        let point = self.ctx.mouse().point(&self.ctx.size_info(), display_offset);

        // Assure the mouse point is not in the scrollback.
        if point.line < 0 {
            return;
        }

        // Calculate modifiers value.
        let mut mods = 0;
        let modifiers = self.ctx.modifiers().state();
        if modifiers.shift_key() {
            mods += 4;
        }
        if modifiers.alt_key() {
            mods += 8;
        }
        if modifiers.control_key() {
            mods += 16;
        }

        // Report mouse events.
        if self.ctx.terminal().mode().contains(TermMode::SGR_MOUSE) {
            self.sgr_mouse_report(point, button + mods, state);
        } else if let ElementState::Released = state {
            self.normal_mouse_report(point, 3 + mods);
        } else {
            self.normal_mouse_report(point, button + mods);
        }
    }

    fn normal_mouse_report(&mut self, point: Point, button: u8) {
        let Point { line, column } = point;
        let utf8 = self.ctx.terminal().mode().contains(TermMode::UTF8_MOUSE);

        let max_point = if utf8 { 2015 } else { 223 };

        if line >= max_point || column >= max_point {
            return;
        }

        let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

        let mouse_pos_encode = |pos: usize| -> Vec<u8> {
            let pos = 32 + 1 + pos;
            let first = 0xC0 + pos / 64;
            let second = 0x80 + (pos & 63);
            vec![first as u8, second as u8]
        };

        if utf8 && column >= Column(95) {
            msg.append(&mut mouse_pos_encode(column.0));
        } else {
            msg.push(32 + 1 + column.0 as u8);
        }

        if utf8 && line >= 95 {
            msg.append(&mut mouse_pos_encode(line.0 as usize));
        } else {
            msg.push(32 + 1 + line.0 as u8);
        }

        self.ctx.write_to_pty(msg);
    }

    fn sgr_mouse_report(&mut self, point: Point, button: u8, state: ElementState) {
        let c = match state {
            ElementState::Pressed => 'M',
            ElementState::Released => 'm',
        };

        let msg = format!("\x1b[<{};{};{}{}", button, point.column + 1, point.line + 1, c);
        self.ctx.write_to_pty(msg.into_bytes());
    }

    fn on_mouse_press(&mut self, button: MouseButton) {
        // Handle mouse mode.
        if !self.ctx.modifiers().state().shift_key() && self.ctx.mouse_mode() {
            self.ctx.mouse_mut().click_state = ClickState::None;

            let code = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                // Can't properly report more than three buttons..
                MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => return,
            };

            self.mouse_report(code, ElementState::Pressed);
        } else {
            // Calculate time since the last click to handle double/triple clicks.
            let now = Instant::now();
            let elapsed = now - self.ctx.mouse().last_click_timestamp;
            self.ctx.mouse_mut().last_click_timestamp = now;

            // Update multi-click state.
            self.ctx.mouse_mut().click_state = match self.ctx.mouse().click_state {
                // Reset click state if button has changed.
                _ if button != self.ctx.mouse().last_click_button => {
                    self.ctx.mouse_mut().last_click_button = button;
                    ClickState::Click
                },
                ClickState::Click if elapsed < CLICK_THRESHOLD => ClickState::DoubleClick,
                ClickState::DoubleClick if elapsed < CLICK_THRESHOLD => ClickState::TripleClick,
                _ => ClickState::Click,
            };

            // Load mouse point, treating message bar and padding as the closest cell.
            let display_offset = self.ctx.terminal().grid().display_offset();
            let point = self.ctx.mouse().point(&self.ctx.size_info(), display_offset);

            if let MouseButton::Left = button {
                self.on_left_click(point)
            }
        }
    }

    /// Handle left click selection and vi mode cursor movement.
    fn on_left_click(&mut self, point: Point) {
        let side = self.ctx.mouse().cell_side;
        let control = self.ctx.modifiers().state().control_key();

        match self.ctx.mouse().click_state {
            ClickState::Click => {
                // Don't launch URLs if this click cleared the selection.
                self.ctx.mouse_mut().block_hint_launcher = !self.ctx.selection_is_empty();

                self.ctx.clear_selection();

                // Start new empty selection.
                if control {
                    self.ctx.start_selection(SelectionType::Block, point, side);
                } else {
                    self.ctx.start_selection(SelectionType::Simple, point, side);
                }
            },
            ClickState::DoubleClick if !control => {
                self.ctx.mouse_mut().block_hint_launcher = true;
                self.ctx.start_selection(SelectionType::Semantic, point, side);
            },
            ClickState::TripleClick if !control => {
                self.ctx.mouse_mut().block_hint_launcher = true;
                self.ctx.start_selection(SelectionType::Lines, point, side);
            },
            _ => (),
        };

        // Move vi mode cursor to mouse click position.
        if self.ctx.terminal().mode().contains(TermMode::VI) && !self.ctx.search_active() {
            self.ctx.terminal_mut().vi_mode_cursor.point = point;
            self.ctx.mark_dirty();
        }
    }

    fn on_mouse_release(&mut self, button: MouseButton) {
        if !self.ctx.modifiers().state().shift_key() && self.ctx.mouse_mode() {
            let code = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                // Can't properly report more than three buttons.
                MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => return,
            };
            self.mouse_report(code, ElementState::Released);
            return;
        }

        // Trigger hints highlighted by the mouse.
        let hint = self.ctx.display().highlighted_hint.take();
        if let Some(hint) = hint.as_ref().filter(|_| button == MouseButton::Left) {
            self.ctx.trigger_hint(hint);
        }
        self.ctx.display().highlighted_hint = hint;

        let timer_id = TimerId::for_pane(
            Topic::SelectionScrolling,
            self.ctx.window().id(),
            self.ctx.pane_id(),
        );
        self.ctx.scheduler_mut().unschedule(timer_id);

        if let MouseButton::Left | MouseButton::Right = button {
            // Copy selection on release, to prevent flooding the display server.
            self.ctx.copy_selection(ClipboardType::Selection);
        }
    }

    pub fn mouse_wheel_input(&mut self, delta: MouseScrollDelta, phase: TouchPhase) {
        let multiplier = self.ctx.config().scrolling.multiplier;
        match delta {
            MouseScrollDelta::LineDelta(columns, lines) => {
                let new_scroll_px_x = columns * self.ctx.size_info().cell_width();
                let new_scroll_px_y = lines * self.ctx.size_info().cell_height();
                self.scroll_terminal(
                    new_scroll_px_x as f64,
                    new_scroll_px_y as f64,
                    multiplier as f64,
                );
            },
            MouseScrollDelta::PixelDelta(mut lpos) => {
                match phase {
                    TouchPhase::Started => {
                        // Reset offset to zero.
                        self.ctx.mouse_mut().accumulated_scroll = Default::default();
                    },
                    TouchPhase::Moved => {
                        // When the angle between (x, 0) and (x, y) is lower than ~25 degrees
                        // (cosine is larger that 0.9) we consider this scrolling as horizontal.
                        if lpos.x.abs() / lpos.x.hypot(lpos.y) > 0.9 {
                            lpos.y = 0.;
                        } else {
                            lpos.x = 0.;
                        }

                        self.scroll_terminal(lpos.x, lpos.y, multiplier as f64);
                    },
                    _ => (),
                }
            },
        }
    }

    fn scroll_terminal(&mut self, new_scroll_x_px: f64, new_scroll_y_px: f64, multiplier: f64) {
        const MOUSE_WHEEL_UP: u8 = 64;
        const MOUSE_WHEEL_DOWN: u8 = 65;
        const MOUSE_WHEEL_LEFT: u8 = 66;
        const MOUSE_WHEEL_RIGHT: u8 = 67;

        let width = f64::from(self.ctx.size_info().cell_width());
        let height = f64::from(self.ctx.size_info().cell_height());

        let multiplier = if self.ctx.mouse_mode() { 1. } else { multiplier };

        self.ctx.mouse_mut().accumulated_scroll.x += new_scroll_x_px * multiplier;
        self.ctx.mouse_mut().accumulated_scroll.y += new_scroll_y_px * multiplier;

        let lines = (self.ctx.mouse().accumulated_scroll.y / height).abs() as usize;
        let columns = (self.ctx.mouse().accumulated_scroll.x / width).abs() as usize;

        let is_scroll_up = new_scroll_y_px > 0.;
        let event = if is_scroll_up { MouseEvent::WheelUp } else { MouseEvent::WheelDown };

        if lines != 0 && self.process_mouse_bindings(event) {
            // Repeat for remaining number of lines.
            for _ in 1..lines {
                self.process_mouse_bindings(event);
            }
        } else if self.ctx.mouse_mode() {
            let code = if is_scroll_up { MOUSE_WHEEL_UP } else { MOUSE_WHEEL_DOWN };
            for _ in 0..lines {
                self.mouse_report(code, ElementState::Pressed);
            }

            let code = if new_scroll_x_px > 0. { MOUSE_WHEEL_LEFT } else { MOUSE_WHEEL_RIGHT };
            for _ in 0..columns {
                self.mouse_report(code, ElementState::Pressed);
            }
        } else if self
            .ctx
            .terminal()
            .mode()
            .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !self.ctx.modifiers().state().shift_key()
        {
            // The chars here are the same as for the respective arrow keys.
            let line_cmd = if is_scroll_up { b'A' } else { b'B' };
            let column_cmd = if new_scroll_x_px > 0. { b'D' } else { b'C' };

            let mut content = Vec::with_capacity(3 * (lines + columns));

            for _ in 0..lines {
                content.push(0x1b);
                content.push(b'O');
                content.push(line_cmd);
            }

            for _ in 0..columns {
                content.push(0x1b);
                content.push(b'O');
                content.push(column_cmd);
            }

            self.ctx.write_to_pty(content);
        } else if lines != 0 {
            let lines = if is_scroll_up { lines as i32 } else { -(lines as i32) };
            self.ctx.scroll(Scroll::Delta(lines));
        }

        self.ctx.mouse_mut().accumulated_scroll.x %= width;
        self.ctx.mouse_mut().accumulated_scroll.y %= height;
    }

    pub fn on_focus_change(&mut self, is_focused: bool) {
        if self.ctx.terminal().mode().contains(TermMode::FOCUS_IN_OUT) {
            let chr = if is_focused { "I" } else { "O" };

            let msg = format!("\x1b[{chr}");
            self.ctx.write_to_pty(msg.into_bytes());
        }
    }

    /// Handle touch input.
    pub fn touch(&mut self, touch: TouchEvent) {
        match touch.phase {
            TouchPhase::Started => self.on_touch_start(touch),
            TouchPhase::Moved => self.on_touch_motion(touch),
            TouchPhase::Ended | TouchPhase::Cancelled => self.on_touch_end(touch),
        }
    }

    /// Handle beginning of touch input.
    pub fn on_touch_start(&mut self, touch: TouchEvent) {
        // Inhibit IME on touch while not focused, forcing a touch tap while focused to enable IME.
        if !self.ctx.terminal().is_focused {
            self.ctx.window().set_ime_inhibitor(ImeInhibitor::TOUCH, true);
        }

        let touch_purpose = self.ctx.touch_purpose();
        *touch_purpose = match mem::take(touch_purpose) {
            TouchPurpose::None => TouchPurpose::Tap(touch),
            TouchPurpose::Tap(start) => TouchPurpose::Zoom(TouchZoom::new((start, touch))),
            TouchPurpose::ZoomPendingSlot(slot) => {
                TouchPurpose::Zoom(TouchZoom::new((slot, touch)))
            },
            TouchPurpose::Zoom(zoom) => {
                let slots = zoom.slots();
                let mut set = HashSet::default();
                set.insert(slots.0.id);
                set.insert(slots.1.id);
                TouchPurpose::Invalid(set)
            },
            TouchPurpose::Scroll(event) | TouchPurpose::Select(event) => {
                let mut set = HashSet::default();
                set.insert(event.id);
                TouchPurpose::Invalid(set)
            },
            TouchPurpose::Invalid(mut slots) => {
                slots.insert(touch.id);
                TouchPurpose::Invalid(slots)
            },
        };
    }

    /// Handle touch input movement.
    pub fn on_touch_motion(&mut self, touch: TouchEvent) {
        let touch_purpose = self.ctx.touch_purpose();
        match touch_purpose {
            TouchPurpose::None => (),
            // Handle transition from tap to scroll/select.
            TouchPurpose::Tap(start) => {
                let delta_x = touch.location.x - start.location.x;
                let delta_y = touch.location.y - start.location.y;
                if delta_x.abs() > MAX_TAP_DISTANCE {
                    // Update gesture state.
                    let start_location = start.location;
                    *touch_purpose = TouchPurpose::Select(*start);

                    // Start simulated mouse input.
                    self.mouse_moved(start_location);
                    self.mouse_input(ElementState::Pressed, MouseButton::Left);

                    // Apply motion since touch start.
                    self.on_touch_motion(touch);
                } else if delta_y.abs() > MAX_TAP_DISTANCE {
                    // Update gesture state.
                    *touch_purpose = TouchPurpose::Scroll(*start);

                    // Apply motion since touch start.
                    self.on_touch_motion(touch);
                }
            },
            TouchPurpose::Zoom(zoom) => {
                let font_delta = zoom.font_delta(touch);
                self.ctx.change_font_size(font_delta);
            },
            TouchPurpose::Scroll(last_touch) => {
                // Calculate delta and update last touch position.
                let delta_y = touch.location.y - last_touch.location.y;
                *touch_purpose = TouchPurpose::Scroll(touch);

                // Use a fixed scroll factor for touchscreens, to accurately track finger motion.
                self.scroll_terminal(0., delta_y, 1.0);
            },
            TouchPurpose::Select(_) => self.mouse_moved(touch.location),
            TouchPurpose::ZoomPendingSlot(_) | TouchPurpose::Invalid(_) => (),
        }
    }

    /// Handle end of touch input.
    pub fn on_touch_end(&mut self, touch: TouchEvent) {
        // Finalize the touch motion up to the release point.
        self.on_touch_motion(touch);

        let touch_purpose = self.ctx.touch_purpose();
        match touch_purpose {
            // Simulate LMB clicks.
            TouchPurpose::Tap(start) => {
                let start_location = start.location;
                *touch_purpose = Default::default();

                self.mouse_moved(start_location);
                self.mouse_input(ElementState::Pressed, MouseButton::Left);
                self.mouse_input(ElementState::Released, MouseButton::Left);

                self.ctx.window().set_ime_inhibitor(ImeInhibitor::TOUCH, false);
            },
            // Transition zoom to pending state once a finger was released.
            TouchPurpose::Zoom(zoom) => {
                let slots = zoom.slots();
                let remaining = if slots.0.id == touch.id { slots.1 } else { slots.0 };
                *touch_purpose = TouchPurpose::ZoomPendingSlot(remaining);
            },
            TouchPurpose::ZoomPendingSlot(_) => *touch_purpose = Default::default(),
            // Reset touch state once all slots were released.
            TouchPurpose::Invalid(slots) => {
                slots.remove(&touch.id);
                if slots.is_empty() {
                    *touch_purpose = Default::default();
                }
            },
            // Release simulated LMB.
            TouchPurpose::Select(_) => {
                *touch_purpose = Default::default();
                self.mouse_input(ElementState::Released, MouseButton::Left);
            },
            // Reset touch state on scroll finish.
            TouchPurpose::Scroll(_) => *touch_purpose = Default::default(),
            TouchPurpose::None => (),
        }
    }

    /// Reset mouse cursor based on modifier and terminal state.
    #[inline]
    pub fn reset_mouse_cursor(&mut self) {
        let mouse_state = self.cursor_state();
        self.ctx.window().set_mouse_cursor(mouse_state);
    }

    /// Modifier state change.
    pub fn modifiers_input(&mut self, modifiers: Modifiers) {
        *self.ctx.modifiers() = modifiers;

        // Prompt hint highlight update.
        self.ctx.mouse_mut().hint_highlight_dirty = true;

        // Update mouse state and check for URL change.
        let mouse_state = self.cursor_state();
        self.ctx.window().set_mouse_cursor(mouse_state);
    }

    pub fn mouse_input(&mut self, state: ElementState, button: MouseButton) {
        match button {
            MouseButton::Left => self.ctx.mouse_mut().left_button_state = state,
            MouseButton::Middle => self.ctx.mouse_mut().middle_button_state = state,
            MouseButton::Right => self.ctx.mouse_mut().right_button_state = state,
            _ => (),
        }

        // Skip normal mouse events if the message bar has been clicked.
        if self.message_bar_cursor_state() == Some(CursorIcon::Pointer)
            && state == ElementState::Pressed
        {
            let size = self.ctx.size_info();

            let current_lines = self.ctx.message().map_or(0, |m| m.text(&size).len());

            self.ctx.clear_selection();
            self.ctx.pop_message();

            // Reset cursor when message bar height changed or all messages are gone.
            let new_lines = self.ctx.message().map_or(0, |m| m.text(&size).len());

            let new_icon = match current_lines.cmp(&new_lines) {
                Ordering::Less => CursorIcon::Default,
                Ordering::Equal => CursorIcon::Pointer,
                Ordering::Greater => {
                    if self.ctx.mouse_mode() {
                        CursorIcon::Default
                    } else {
                        CursorIcon::Text
                    }
                },
            };

            self.ctx.window().set_mouse_cursor(new_icon);
        } else {
            match state {
                ElementState::Pressed => {
                    // Process mouse press before bindings to update the `click_state`.
                    self.on_mouse_press(button);
                    self.process_mouse_bindings(MouseEvent::Button(button));
                },
                ElementState::Released => self.on_mouse_release(button),
            }
        }
    }

    /// Attempt to find a binding and execute its action.
    ///
    /// The provided mode, mods, and key must match what is allowed by a binding
    /// for its action to be executed.
    fn process_mouse_bindings(&mut self, event: MouseEvent) -> bool {
        let mode = BindingMode::new(self.ctx.terminal().mode(), self.ctx.search_active());
        let mouse_mode = self.ctx.mouse_mode();
        let mods = self.ctx.modifiers().state();
        let mouse_bindings = self.ctx.config().mouse_bindings().to_owned();

        // If mouse mode is active, also look for bindings without shift.
        let fallback_allowed = mouse_mode && mods.contains(ModifiersState::SHIFT);
        let mut match_found: bool = false;

        for binding in &mouse_bindings {
            // Don't trigger normal bindings in mouse mode unless Shift is pressed.
            if binding.is_triggered_by(mode, mods, &event) && (fallback_allowed || !mouse_mode) {
                binding.action.execute(&mut self.ctx);
                match_found = true;
            }
        }

        if fallback_allowed && !match_found {
            let fallback_mods = mods & !ModifiersState::SHIFT;
            for binding in &mouse_bindings {
                if binding.is_triggered_by(mode, fallback_mods, &event) {
                    binding.action.execute(&mut self.ctx);
                    match_found = true;
                }
            }
        }

        match_found
    }

    /// Check mouse icon state in relation to the message bar.
    fn message_bar_cursor_state(&self) -> Option<CursorIcon> {
        // Since search is above the message bar, the button is offset by search's height.
        let search_height = usize::from(self.ctx.search_active());

        // Calculate Y position of the end of the last terminal line.
        let size = self.ctx.size_info();
        let terminal_end = size.padding_y() as usize
            + size.cell_height() as usize * (size.screen_lines() + search_height);

        let mouse = self.ctx.mouse();
        let display_offset = self.ctx.terminal().grid().display_offset();
        let point = self.ctx.mouse().point(&self.ctx.size_info(), display_offset);

        if self.ctx.message().is_none() || (mouse.y <= terminal_end) {
            None
        } else if mouse.y <= terminal_end + size.cell_height() as usize
            && point.column + message_bar::CLOSE_BUTTON_TEXT.len() >= size.columns()
        {
            Some(CursorIcon::Pointer)
        } else {
            Some(CursorIcon::Default)
        }
    }

    /// Icon state of the cursor.
    fn cursor_state(&mut self) -> CursorIcon {
        let display_offset = self.ctx.terminal().grid().display_offset();
        let point = self.ctx.mouse().point(&self.ctx.size_info(), display_offset);
        let hyperlink = self.ctx.terminal().grid()[point].hyperlink();

        // Function to check if mouse is on top of a hint.
        let hint_highlighted = |hint: &HintMatch| hint.should_highlight(point, hyperlink.as_ref());

        if let Some(mouse_state) = self.message_bar_cursor_state() {
            mouse_state
        } else if self.ctx.display().highlighted_hint.as_ref().is_some_and(hint_highlighted) {
            CursorIcon::Pointer
        } else if !self.ctx.modifiers().state().shift_key() && self.ctx.mouse_mode() {
            CursorIcon::Default
        } else {
            CursorIcon::Text
        }
    }

    /// Handle automatic scrolling when selecting above/below the window.
    fn update_selection_scrolling(&mut self, mouse_y: i32) {
        let scale_factor = self.ctx.window().scale_factor;
        let size = self.ctx.size_info();
        let window_id = self.ctx.window().id();
        let pane_id = self.ctx.pane_id();
        let scheduler = self.ctx.scheduler_mut();

        // Scale constants by DPI.
        let min_height = (MIN_SELECTION_SCROLLING_HEIGHT * scale_factor) as i32;
        let step = (SELECTION_SCROLLING_STEP * scale_factor) as i32;

        // Compute the height of the scrolling areas.
        let end_top = max(min_height, size.padding_y() as i32);
        let text_area_bottom = size.padding_y() + size.screen_lines() as f32 * size.cell_height();
        let start_bottom = min(size.height() as i32 - min_height, text_area_bottom as i32);

        // Get distance from closest window boundary.
        let delta = if mouse_y < end_top {
            end_top - mouse_y + step
        } else if mouse_y >= start_bottom {
            start_bottom - mouse_y - step
        } else {
            scheduler.unschedule(TimerId::for_pane(Topic::SelectionScrolling, window_id, pane_id));
            return;
        };

        // Scale number of lines scrolled based on distance to boundary.
        let event = Event::new(EventType::Scroll(Scroll::Delta(delta / step)), Some(window_id))
            .with_pane(pane_id);

        // Schedule event.
        let timer_id = TimerId::for_pane(Topic::SelectionScrolling, window_id, pane_id);
        scheduler.unschedule(timer_id);
        scheduler.schedule(event, SELECTION_SCROLLING_INTERVAL, true, timer_id);
    }
}
