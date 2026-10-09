//! Route Winit events into keyboard, mouse and terminal actions.

use std::time::{Duration, Instant};

use winit::event::{Event as WinitEvent, Ime, WindowEvent};

use alacritty_session::events::Notifier;
use alacritty_terminal::event::Event as TerminalEvent;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::NamedColor;

use crate::app::scheduler::{TimerId, Topic};
use crate::input::{self, ActionContext as _};
use crate::platform::window::ImeInhibitor;
use crate::presentation::Preedit;
use crate::presentation::color::Rgb;

use crate::app::{ActionContext, Event, EventProxy, EventType};
const BELL_CMD_COOLDOWN: Duration = Duration::from_millis(100);

impl input::Processor<EventProxy, ActionContext<'_, Notifier, EventProxy>> {
    /// Handle events from winit.
    pub fn handle_event(&mut self, event: WinitEvent<Event>) {
        match event {
            WinitEvent::UserEvent(Event { payload, .. }) => match payload {
                #[cfg(target_os = "macos")]
                EventType::MacosAction(command) => {
                    use crate::input::Execute;
                    if let Some(action) = command.action() {
                        action.execute(&mut self.ctx);
                    }
                    self.ctx.mark_dirty();
                },
                #[cfg(target_os = "macos")]
                EventType::MacosMenu(..) => (),
                EventType::GraphicsAnimation => self.ctx.mark_dirty(),
                EventType::SearchNext => self.ctx.goto_match(None),
                EventType::Scroll(scroll) => self.ctx.scroll(scroll),
                EventType::BlinkCursor => {
                    // Only change state when timeout isn't reached, since we could get
                    // BlinkCursor and BlinkCursorTimeout events at the same time.
                    if !*self.ctx.cursor_blink_timed_out {
                        self.ctx.display.cursor_hidden ^= true;
                        *self.ctx.dirty = true;
                    }
                },
                EventType::BlinkCursorTimeout => {
                    // Disable blinking after timeout reached.
                    let timer_id = TimerId::for_pane(
                        Topic::BlinkCursor,
                        self.ctx.display.window.id(),
                        self.ctx.pane_id,
                    );
                    self.ctx.scheduler.unschedule(timer_id);
                    *self.ctx.cursor_blink_timed_out = true;
                    self.ctx.display.cursor_hidden = false;
                    *self.ctx.dirty = true;
                },
                // Add message only if it's not already queued.
                EventType::Message(message) if !self.ctx.message_buffer.is_queued(&message) => {
                    self.ctx.message_buffer.push(message);
                    self.ctx.display.pending_update.dirty = true;
                },
                EventType::Terminal(event) => match event {
                    TerminalEvent::Title(title) => {
                        if !self.ctx.preserve_title && self.ctx.config.window.dynamic_title {
                            self.ctx.window().set_title(title);
                        }
                    },
                    TerminalEvent::ResetTitle => {
                        let window_config = &self.ctx.config.window;
                        if !self.ctx.preserve_title && window_config.dynamic_title {
                            self.ctx.display.window.set_title(window_config.identity.title.clone());
                        }
                    },
                    TerminalEvent::Bell => {
                        // Set window urgency hint when window is not focused.
                        let focused = self.ctx.terminal.is_focused;
                        if !focused && self.ctx.terminal.mode().contains(TermMode::URGENCY_HINTS) {
                            self.ctx.window().set_urgent(true);
                        }

                        // Ring visual bell.
                        self.ctx.display.visual_bell.ring();

                        // Execute bell command.
                        if let Some(bell_command) = &self.ctx.config.bell.command
                            && self
                                .ctx
                                .prev_bell_cmd
                                .is_none_or(|i| i.elapsed() >= BELL_CMD_COOLDOWN)
                        {
                            self.ctx.spawn_daemon(bell_command.program(), bell_command.args());

                            *self.ctx.prev_bell_cmd = Some(Instant::now());
                        }
                    },
                    TerminalEvent::ClipboardStore(clipboard_type, content) => {
                        if self.ctx.terminal.is_focused {
                            self.ctx.clipboard.store(clipboard_type, content);
                        }
                    },
                    TerminalEvent::KittyClipboard(osc) => {
                        let mut host = crate::clipboard::kitty::Host {
                            clipboard: self.ctx.clipboard,
                            permission: self.ctx.config.terminal.osc52.0,
                            focused: self.ctx.terminal.is_focused,
                        };
                        let replies = self.ctx.kitty_clipboard.handle_osc(osc, &mut host);
                        for reply in replies {
                            self.ctx.write_to_pty(reply);
                        }
                    },
                    TerminalEvent::KittyClipboardMode(enabled) => {
                        self.ctx.kitty_clipboard.set_paste_events_enabled(enabled)
                    },
                    TerminalEvent::KittyClipboardReset => self.ctx.kitty_clipboard.reset(),
                    TerminalEvent::ClipboardLoad(clipboard_type, format) => {
                        if self.ctx.terminal.is_focused {
                            let text = format(self.ctx.clipboard.load(clipboard_type).as_str());
                            self.ctx.write_to_pty(text.into_bytes());
                        }
                    },
                    TerminalEvent::ColorRequest(index, format) => {
                        let color = match self.ctx.terminal().colors()[index] {
                            Some(color) => Rgb(color),
                            // Ignore cursor color requests unless it was changed.
                            None if index == NamedColor::Cursor as usize => return,
                            None => self.ctx.display.colors[index],
                        };
                        self.ctx.write_to_pty(format(color.0).into_bytes());
                    },
                    TerminalEvent::TextAreaSizeRequest(format) => {
                        let text = format(self.ctx.size_info().into());
                        self.ctx.write_to_pty(text.into_bytes());
                    },
                    TerminalEvent::PtyWrite(text) => self.ctx.write_to_pty(text.into_bytes()),
                    TerminalEvent::MouseCursorDirty => self.reset_mouse_cursor(),
                    TerminalEvent::CursorBlinkingChange => self.ctx.update_cursor_blinking(),
                    TerminalEvent::Exit
                    | TerminalEvent::ChildExit(_)
                    | TerminalEvent::Wakeup
                    | TerminalEvent::ProgramStatusChanged => (),
                },
                #[cfg(unix)]
                EventType::IpcConfig(_) | EventType::IpcGetConfig(..) | EventType::Shutdown => (),
                EventType::Message(_)
                | EventType::ConfigReload(_)
                | EventType::Pane(_)
                | EventType::CreateWindow(_)
                | EventType::Frame => (),
            },
            WinitEvent::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::CloseRequested => {
                        // User asked to close the window, so no need to hold it.
                        self.ctx.window().hold = false;
                        self.ctx.terminal.exit();
                    },
                    // WindowContext rescales all panes together.
                    WindowEvent::ScaleFactorChanged { .. } => (),
                    WindowEvent::Resized(size) => {
                        // Ignore resize events to zero in any dimension, to avoid issues with Winit
                        // and the ConPTY. A 0x0 resize will also occur when the window is minimized
                        // on Windows.
                        if size.width == 0 || size.height == 0 {
                            return;
                        }

                        self.ctx.display.pending_update.set_dimensions(size);
                    },
                    WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                        self.key_input(event);
                    },
                    WindowEvent::ModifiersChanged(modifiers) => self.modifiers_input(modifiers),
                    WindowEvent::MouseInput { state, button, .. } => {
                        self.ctx.window().set_mouse_visible(true);
                        self.mouse_input(state, button);
                    },
                    WindowEvent::CursorMoved { position, .. } => {
                        self.ctx.window().set_mouse_visible(true);
                        self.mouse_moved(position);
                    },
                    WindowEvent::MouseWheel { delta, phase, .. } => {
                        self.ctx.window().set_mouse_visible(true);
                        self.mouse_wheel_input(delta, phase);
                    },
                    WindowEvent::Touch(touch) => self.touch(touch),
                    WindowEvent::Focused(is_focused) => {
                        self.ctx.terminal.is_focused = is_focused;

                        // When the unfocused hollow is used we must redraw on focus change.
                        if self.ctx.config.cursor.unfocused_hollow {
                            *self.ctx.dirty = true;
                        }

                        // Reset the urgency hint when gaining focus.
                        if is_focused {
                            self.ctx.window().set_urgent(false);
                        }

                        self.ctx.update_cursor_blinking();
                        self.on_focus_change(is_focused);

                        // Ensure IME is disabled while unfocused.
                        self.ctx.window().set_ime_inhibitor(ImeInhibitor::FOCUS, !is_focused);
                    },
                    WindowEvent::Occluded(occluded) => {
                        *self.ctx.occluded = occluded;
                    },
                    WindowEvent::DroppedFile(path) => {
                        let path: String = path.to_string_lossy().into();
                        self.ctx.paste(&(path + " "), true);
                    },
                    WindowEvent::CursorLeft { .. } => {
                        self.ctx.mouse.inside_text_area = false;

                        if self.ctx.display().highlighted_hint.is_some() {
                            *self.ctx.dirty = true;
                        }
                    },
                    WindowEvent::Ime(ime) => match ime {
                        Ime::Commit(text) => {
                            *self.ctx.dirty = true;
                            // Don't use bracketed paste for single char input.
                            self.ctx.paste(&text, text.chars().count() > 1);
                            self.ctx.update_cursor_blinking();
                        },
                        Ime::Preedit(text, cursor_offset) => {
                            let preedit =
                                (!text.is_empty()).then(|| Preedit::new(text, cursor_offset));

                            if self.ctx.display.ime.preedit() != preedit.as_ref() {
                                self.ctx.display.ime.set_preedit(preedit);
                                self.ctx.update_cursor_blinking();
                                *self.ctx.dirty = true;
                            }
                        },
                        Ime::Enabled => {
                            self.ctx.display.ime.set_enabled(true);
                            *self.ctx.dirty = true;
                        },
                        Ime::Disabled => {
                            self.ctx.display.ime.set_enabled(false);
                            *self.ctx.dirty = true;
                        },
                    },
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
                    | WindowEvent::RedrawRequested
                    | WindowEvent::Moved(_) => (),
                }
            },
            WinitEvent::Suspended
            | WinitEvent::NewEvents { .. }
            | WinitEvent::DeviceEvent { .. }
            | WinitEvent::LoopExiting
            | WinitEvent::Resumed
            | WinitEvent::MemoryWarning
            | WinitEvent::AboutToWait => (),
        }
    }
}
