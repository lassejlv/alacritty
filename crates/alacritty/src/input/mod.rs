//! Handle input from winit.
//!
//! Certain key combinations should send some escape sequence back to the PTY.
//! In order to figure that out, state about which modifier keys are pressed
//! needs to be tracked. Additionally, we need a bit of a state machine to
//! determine what to do when a non-modifier key is pressed.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::marker::PhantomData;

use winit::event::Modifiers;
#[cfg(target_os = "macos")]
use winit::event_loop::ActiveEventLoop;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::index::{Direction, Point, Side};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::{ClipboardType, Term};

use crate::app::scheduler::Scheduler;
use crate::clipboard::Clipboard;
use crate::config::UiConfig;
use crate::input::mouse::{Mouse, TouchPurpose};
use crate::platform::window::Window;
use crate::presentation::hints::HintMatch;
use crate::presentation::messages::Message;
use crate::presentation::search::InlineSearchState;
use crate::presentation::{Display, SizeInfo};

mod events;
pub mod keyboard;
pub mod mouse;
pub(crate) use crate::app::commands::Execute;

/// Font size change interval in px.
pub const FONT_SIZE_STEP: f32 = 1.;

/// Processes input from winit.
///
/// An escape sequence may be emitted in case specific keys or key combinations
/// are activated.
pub struct Processor<T: EventListener, A: ActionContext<T>> {
    pub ctx: A,
    _phantom: PhantomData<T>,
}

pub trait ActionContext<T: EventListener> {
    fn write_to_pty<B: Into<Cow<'static, [u8]>>>(&self, _data: B) {}
    fn pane_id(&self) -> crate::workspace::layout::PaneId {
        0
    }
    fn pane_command(&mut self, _command: crate::workspace::layout::PaneCommand) {}
    fn mark_dirty(&mut self) {}
    fn size_info(&self) -> SizeInfo;
    fn copy_selection(&mut self, _ty: ClipboardType) {}
    fn start_selection(&mut self, _ty: SelectionType, _point: Point, _side: Side) {}
    fn toggle_selection(&mut self, _ty: SelectionType, _point: Point, _side: Side) {}
    fn update_selection(&mut self, _point: Point, _side: Side) {}
    fn clear_selection(&mut self) {}
    fn selection_is_empty(&self) -> bool;
    fn mouse_mut(&mut self) -> &mut Mouse;
    fn mouse(&self) -> &Mouse;
    fn touch_purpose(&mut self) -> &mut TouchPurpose;
    fn modifiers(&mut self) -> &mut Modifiers;
    fn scroll(&mut self, _scroll: Scroll) {}
    fn window(&mut self) -> &mut Window;
    fn display(&mut self) -> &mut Display;
    fn terminal(&self) -> &Term<T>;
    fn terminal_mut(&mut self) -> &mut Term<T>;
    fn spawn_new_instance(&mut self) {}
    #[cfg(target_os = "macos")]
    fn open_config(&mut self) {}
    #[cfg(target_os = "macos")]
    fn create_new_window(&mut self, _tabbing_id: Option<String>) {}
    #[cfg(not(target_os = "macos"))]
    fn create_new_window(&mut self) {}
    fn change_font_size(&mut self, _delta: f32) {}
    fn reset_font_size(&mut self) {}
    fn pop_message(&mut self) {}
    fn message(&self) -> Option<&Message>;
    fn config(&self) -> &UiConfig;
    #[cfg(target_os = "macos")]
    fn event_loop(&self) -> &ActiveEventLoop;
    fn mouse_mode(&self) -> bool;
    fn clipboard_mut(&mut self) -> &mut Clipboard;
    fn scheduler_mut(&mut self) -> &mut Scheduler;
    fn start_search(&mut self, _direction: Direction) {}
    fn start_seeded_search(&mut self, _direction: Direction, _text: String) {}
    fn confirm_search(&mut self) {}
    fn cancel_search(&mut self) {}
    fn search_input(&mut self, _c: char) {}
    fn search_pop_word(&mut self) {}
    fn search_history_previous(&mut self) {}
    fn search_history_next(&mut self) {}
    fn search_next(&mut self, origin: Point, direction: Direction, side: Side) -> Option<Match>;
    fn advance_search_origin(&mut self, _direction: Direction) {}
    fn search_direction(&self) -> Direction;
    fn search_active(&self) -> bool;
    fn on_typing_start(&mut self) {}
    fn toggle_vi_mode(&mut self) {}
    fn inline_search_state(&mut self) -> &mut InlineSearchState;
    fn start_inline_search(&mut self, _direction: Direction, _stop_short: bool) {}
    fn inline_search_next(&mut self) {}
    fn inline_search_input(&mut self, _text: &str) {}
    fn inline_search_previous(&mut self) {}
    fn hint_input(&mut self, _character: char) {}
    fn trigger_hint(&mut self, _hint: &HintMatch) {}
    fn expand_selection(&mut self) {}
    fn semantic_word(&self, point: Point) -> String;
    fn on_terminal_input_start(&mut self) {}
    fn paste(&mut self, _text: &str, _bracketed: bool) {}

    fn paste_clipboard(&mut self, ty: ClipboardType) {
        let text = self.clipboard_mut().load(ty);
        self.paste(&text, true);
    }
    fn spawn_daemon<I, S>(&self, _program: &str, _args: I)
    where
        I: IntoIterator<Item = S> + Debug + Copy,
        S: AsRef<OsStr>,
    {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, BindingMode};
    use crate::input::mouse::{CLICK_THRESHOLD, ClickState};
    use std::time::{Duration, Instant};
    use winit::event::{ElementState, MouseButton};
    use winit::keyboard::ModifiersState;

    use winit::event::{DeviceId, Event as WinitEvent, WindowEvent};
    use winit::keyboard::Key;
    use winit::window::WindowId;

    use alacritty_terminal::event::Event as TerminalEvent;

    use crate::config::Binding;
    use crate::presentation::messages::MessageBuffer;

    const KEY: Key<&'static str> = Key::Character("0");

    struct MockEventProxy;
    impl EventListener for MockEventProxy {}

    struct ActionContext<'a, T> {
        pub terminal: &'a mut Term<T>,
        pub size_info: &'a SizeInfo,
        pub mouse: &'a mut Mouse,
        pub clipboard: &'a mut Clipboard,
        pub message_buffer: &'a mut MessageBuffer,
        pub modifiers: Modifiers,
        config: &'a UiConfig,
        inline_search_state: &'a mut InlineSearchState,
    }

    impl<T: EventListener> super::ActionContext<T> for ActionContext<'_, T> {
        fn search_next(
            &mut self,
            _origin: Point,
            _direction: Direction,
            _side: Side,
        ) -> Option<Match> {
            None
        }

        fn search_direction(&self) -> Direction {
            Direction::Right
        }

        fn inline_search_state(&mut self) -> &mut InlineSearchState {
            self.inline_search_state
        }

        fn search_active(&self) -> bool {
            false
        }

        fn terminal(&self) -> &Term<T> {
            self.terminal
        }

        fn terminal_mut(&mut self) -> &mut Term<T> {
            self.terminal
        }

        fn size_info(&self) -> SizeInfo {
            *self.size_info
        }

        fn selection_is_empty(&self) -> bool {
            true
        }

        fn scroll(&mut self, scroll: Scroll) {
            self.terminal.scroll_display(scroll);
        }

        fn mouse_mode(&self) -> bool {
            false
        }

        #[inline]
        fn mouse_mut(&mut self) -> &mut Mouse {
            self.mouse
        }

        #[inline]
        fn mouse(&self) -> &Mouse {
            self.mouse
        }

        #[inline]
        fn touch_purpose(&mut self) -> &mut TouchPurpose {
            unimplemented!();
        }

        fn modifiers(&mut self) -> &mut Modifiers {
            &mut self.modifiers
        }

        fn window(&mut self) -> &mut Window {
            unimplemented!();
        }

        fn display(&mut self) -> &mut Display {
            unimplemented!();
        }

        fn pop_message(&mut self) {
            self.message_buffer.pop();
        }

        fn message(&self) -> Option<&Message> {
            self.message_buffer.message()
        }

        fn config(&self) -> &UiConfig {
            self.config
        }

        fn clipboard_mut(&mut self) -> &mut Clipboard {
            self.clipboard
        }

        #[cfg(target_os = "macos")]
        fn event_loop(&self) -> &ActiveEventLoop {
            unimplemented!();
        }

        fn scheduler_mut(&mut self) -> &mut Scheduler {
            unimplemented!();
        }

        fn semantic_word(&self, _point: Point) -> String {
            unimplemented!();
        }
    }

    #[test]
    fn select_all_includes_scrollback_and_content_below_viewport() {
        let cfg = UiConfig::default();
        let size = SizeInfo::new(50., 20., 10., 10., 0., 0., false);
        let mut terminal = Term::new(cfg.term_options(), &size, MockEventProxy);
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut terminal, b"one\r\ntwo\r\nthree");
        terminal.scroll_display(Scroll::Top);
        let mut clipboard = Clipboard::new_nop();
        let mut mouse = Mouse::default();
        let mut message_buffer = MessageBuffer::default();
        let mut inline_search_state = InlineSearchState::default();
        let mut context = ActionContext {
            terminal: &mut terminal,
            size_info: &size,
            mouse: &mut mouse,
            clipboard: &mut clipboard,
            modifiers: Default::default(),
            message_buffer: &mut message_buffer,
            inline_search_state: &mut inline_search_state,
            config: &cfg,
        };

        Action::SelectAll.execute(&mut context);

        assert_eq!(context.terminal.selection_to_string().unwrap().trim_end(), "one\ntwo\nthree");
    }

    #[test]
    fn select_all_on_alternate_screen_excludes_primary_history() {
        let cfg = UiConfig::default();
        let size = SizeInfo::new(50., 20., 10., 10., 0., 0., false);
        let mut terminal = Term::new(cfg.term_options(), &size, MockEventProxy);
        let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
        parser.advance(&mut terminal, b"one\r\ntwo\r\nthree\x1b[?1049h\x1b[HALT");
        terminal.scroll_display(Scroll::Top);
        let mut clipboard = Clipboard::new_nop();
        let mut mouse = Mouse::default();
        let mut message_buffer = MessageBuffer::default();
        let mut inline_search_state = InlineSearchState::default();
        let mut context = ActionContext {
            terminal: &mut terminal,
            size_info: &size,
            mouse: &mut mouse,
            clipboard: &mut clipboard,
            modifiers: Default::default(),
            message_buffer: &mut message_buffer,
            inline_search_state: &mut inline_search_state,
            config: &cfg,
        };

        Action::SelectAll.execute(&mut context);

        assert_eq!(context.terminal.selection_to_string().unwrap().trim_end(), "ALT");
    }

    macro_rules! test_clickstate {
        {
            name: $name:ident,
            initial_state: $initial_state:expr,
            initial_button: $initial_button:expr,
            input: $input:expr,
            end_state: $end_state:expr,
            input_delay: $input_delay:expr,
        } => {
            #[test]
            fn $name() {
                let mut clipboard = Clipboard::new_nop();
                let cfg = UiConfig::default();
                let size = SizeInfo::new(
                    21.0,
                    51.0,
                    3.0,
                    3.0,
                    0.,
                    0.,
                    false,
                );

                let mut terminal = Term::new(cfg.term_options(), &size, MockEventProxy);

                let mut mouse = Mouse {
                    click_state: $initial_state,
                    last_click_button: $initial_button,
                    last_click_timestamp: Instant::now() - $input_delay,
                    ..Mouse::default()
                };

                let mut inline_search_state = InlineSearchState::default();
                let mut message_buffer = MessageBuffer::default();

                let context = ActionContext {
                    terminal: &mut terminal,
                    mouse: &mut mouse,
                    size_info: &size,
                    clipboard: &mut clipboard,
                    modifiers: Default::default(),
                    message_buffer: &mut message_buffer,
                    inline_search_state: &mut inline_search_state,
                    config: &cfg,
                };

                let mut processor = Processor::new(context);

                let event: WinitEvent::<TerminalEvent> = $input;
                if let WinitEvent::WindowEvent {
                    event: WindowEvent::MouseInput {
                        state,
                        button,
                        ..
                    },
                    ..
                } = event
                {
                    processor.mouse_input(state, button);
                };

                assert_eq!(processor.ctx.mouse.click_state, $end_state);
            }
        }
    }

    macro_rules! test_process_binding {
        {
            name: $name:ident,
            binding: $binding:expr,
            triggers: $triggers:expr,
            mode: $mode:expr,
            mods: $mods:expr,
        } => {
            #[test]
            fn $name() {
                if $triggers {
                    assert!($binding.is_triggered_by($mode, $mods, &KEY));
                } else {
                    assert!(!$binding.is_triggered_by($mode, $mods, &KEY));
                }
            }
        }
    }

    test_clickstate! {
        name: single_click,
        initial_state: ClickState::None,
        initial_button: MouseButton::Other(0),
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: Duration::ZERO,
    }

    test_clickstate! {
        name: single_right_click,
        initial_state: ClickState::None,
        initial_button: MouseButton::Other(0),
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: Duration::ZERO,
    }

    test_clickstate! {
        name: single_middle_click,
        initial_state: ClickState::None,
        initial_button: MouseButton::Other(0),
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Middle,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: Duration::ZERO,
    }

    test_clickstate! {
        name: double_click,
        initial_state: ClickState::Click,
        initial_button: MouseButton::Left,
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::DoubleClick,
        input_delay: Duration::ZERO,
    }

    test_clickstate! {
        name: double_click_failed,
        initial_state: ClickState::Click,
        initial_button: MouseButton::Left,
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: CLICK_THRESHOLD,
    }

    test_clickstate! {
        name: triple_click,
        initial_state: ClickState::DoubleClick,
        initial_button: MouseButton::Left,
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                device_id:  DeviceId::dummy(),
            },
            window_id:  WindowId::dummy(),
        },
        end_state: ClickState::TripleClick,
        input_delay: Duration::ZERO,
    }

    test_clickstate! {
        name: triple_click_failed,
        initial_state: ClickState::DoubleClick,
        initial_button: MouseButton::Left,
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: CLICK_THRESHOLD,
    }

    test_clickstate! {
        name: multi_click_separate_buttons,
        initial_state: ClickState::DoubleClick,
        initial_button: MouseButton::Left,
        input: WinitEvent::WindowEvent {
            event: WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                device_id: DeviceId::dummy(),
            },
            window_id: WindowId::dummy(),
        },
        end_state: ClickState::Click,
        input_delay: Duration::ZERO,
    }

    test_process_binding! {
        name: process_binding_nomode_shiftmod_require_shift,
        binding: Binding { trigger: KEY, mods: ModifiersState::SHIFT, action: Action::from("\x1b[1;2D"), mode: BindingMode::empty(), notmode: BindingMode::empty() },
        triggers: true,
        mode: BindingMode::empty(),
        mods: ModifiersState::SHIFT,
    }

    test_process_binding! {
        name: process_binding_nomode_nomod_require_shift,
        binding: Binding { trigger: KEY, mods: ModifiersState::SHIFT, action: Action::from("\x1b[1;2D"), mode: BindingMode::empty(), notmode: BindingMode::empty() },
        triggers: false,
        mode: BindingMode::empty(),
        mods: ModifiersState::empty(),
    }

    test_process_binding! {
        name: process_binding_nomode_controlmod,
        binding: Binding { trigger: KEY, mods: ModifiersState::CONTROL, action: Action::from("\x1b[1;5D"), mode: BindingMode::empty(), notmode: BindingMode::empty() },
        triggers: true,
        mode: BindingMode::empty(),
        mods: ModifiersState::CONTROL,
    }

    test_process_binding! {
        name: process_binding_nomode_nomod_require_not_appcursor,
        binding: Binding { trigger: KEY, mods: ModifiersState::empty(), action: Action::from("\x1b[D"), mode: BindingMode::empty(), notmode: BindingMode::APP_CURSOR },
        triggers: true,
        mode: BindingMode::empty(),
        mods: ModifiersState::empty(),
    }

    test_process_binding! {
        name: process_binding_appcursormode_nomod_require_appcursor,
        binding: Binding { trigger: KEY, mods: ModifiersState::empty(), action: Action::from("\x1bOD"), mode: BindingMode::APP_CURSOR, notmode: BindingMode::empty() },
        triggers: true,
        mode: BindingMode::APP_CURSOR,
        mods: ModifiersState::empty(),
    }

    test_process_binding! {
        name: process_binding_nomode_nomod_require_appcursor,
        binding: Binding { trigger: KEY, mods: ModifiersState::empty(), action: Action::from("\x1bOD"), mode: BindingMode::APP_CURSOR, notmode: BindingMode::empty() },
        triggers: false,
        mode: BindingMode::empty(),
        mods: ModifiersState::empty(),
    }

    test_process_binding! {
        name: process_binding_appcursormode_appkeypadmode_nomod_require_appcursor,
        binding: Binding { trigger: KEY, mods: ModifiersState::empty(), action: Action::from("\x1bOD"), mode: BindingMode::APP_CURSOR, notmode: BindingMode::empty() },
        triggers: true,
        mode: BindingMode::APP_CURSOR | BindingMode::APP_KEYPAD,
        mods: ModifiersState::empty(),
    }

    test_process_binding! {
        name: process_binding_fail_with_extra_mods,
        binding: Binding { trigger: KEY, mods: ModifiersState::SUPER, action: Action::from("arst"), mode: BindingMode::empty(), notmode: BindingMode::empty() },
        triggers: false,
        mode: BindingMode::empty(),
        mods: ModifiersState::ALT | ModifiersState::SUPER,
    }
}
