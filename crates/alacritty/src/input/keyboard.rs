use winit::event::{ElementState, KeyEvent};
#[cfg(target_os = "macos")]
use winit::keyboard::ModifiersKeyState;
use winit::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
#[cfg(target_os = "macos")]
use winit::platform::macos::OptionAsAlt;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::input::keyboard::{self, Modifiers};
use alacritty_terminal::term::TermMode;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

use crate::app::context::TYPING_SEARCH_DELAY;
use crate::app::scheduler::{TimerId, Topic};
use crate::config::{Action, BindingKey, BindingMode, KeyBinding};
use crate::input::{ActionContext, Execute, Processor};
use crate::platform::window::ImeInhibitor;

impl<T: EventListener, A: ActionContext<T>> Processor<T, A> {
    /// Process key input.
    pub fn key_input(&mut self, key: KeyEvent) {
        // IME input will be applied on commit and shouldn't trigger key bindings.
        if self.ctx.display().ime.preedit().is_some() {
            return;
        }

        let mode = *self.ctx.terminal().mode();
        let mods = self.ctx.modifiers().state();

        if key.state == ElementState::Released {
            if self.ctx.inline_search_state().char_pending {
                self.ctx.window().set_ime_inhibitor(ImeInhibitor::VI, false);
            }
            self.key_release(key, mode, mods);
            return;
        }

        let text = key.text_with_all_modifiers().unwrap_or_default();

        // All key bindings are disabled while a hint is being selected.
        if self.ctx.display().hint_state.active() {
            for character in text.chars() {
                self.ctx.hint_input(character);
            }
            return;
        }

        // First key after inline search is captured.
        let inline_state = self.ctx.inline_search_state();
        if inline_state.char_pending {
            self.ctx.inline_search_input(text);
            return;
        }

        // Reset search delay when the user is still typing.
        self.reset_search_delay();

        // Key bindings suppress the character input.
        if self.process_key_bindings(&key) {
            return;
        }

        if self.ctx.search_active() {
            for character in text.chars() {
                self.ctx.search_input(character);
            }

            return;
        }

        // Vi mode on its own doesn't have any input, the search input was done before.
        if mode.contains(TermMode::VI) {
            return;
        }

        let is_modifier_key = Self::is_modifier_key(&key);
        let bytes = keyboard::encode(&self.terminal_key_event(&key, mods), mode);

        // Write only if we have something to write.
        if !bytes.is_empty() {
            // Don't clear selection/scroll down when writing escaped modifier keys.
            if !is_modifier_key {
                self.ctx.on_terminal_input_start();
            }
            self.ctx.write_to_pty(bytes);
        }
    }

    /// Whether Option/Alt composes text instead of acting as a modifier.
    fn alt_composes(&mut self) -> bool {
        #[cfg(not(target_os = "macos"))]
        return false;

        #[cfg(target_os = "macos")]
        {
            let option_as_alt = self.ctx.config().window.option_as_alt();
            let modifiers = self.ctx.modifiers();
            !(option_as_alt == OptionAsAlt::Both
                || (option_as_alt == OptionAsAlt::OnlyLeft
                    && modifiers.lalt_state() == ModifiersKeyState::Pressed)
                || (option_as_alt == OptionAsAlt::OnlyRight
                    && modifiers.ralt_state() == ModifiersKeyState::Pressed))
        }
    }

    /// Convert a winit key event for the terminal's keyboard encoder.
    fn terminal_key_event<'a>(
        &mut self,
        key: &'a KeyEvent,
        mods: ModifiersState,
    ) -> keyboard::KeyEvent<'a> {
        let single_char = |text: &str| {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        };
        let logical_key = match key.logical_key.as_ref() {
            Key::Named(named) => {
                named_key(named).map_or(keyboard::Key::Other, keyboard::Key::Named)
            },
            Key::Character(text) => {
                single_char(text).map_or(keyboard::Key::Other, keyboard::Key::Character)
            },
            _ => keyboard::Key::Other,
        };
        let unshifted = match key.key_without_modifiers() {
            Key::Character(text) => single_char(&text),
            _ => None,
        };

        let mut modifiers = Modifiers::empty();
        modifiers.set(Modifiers::SHIFT, mods.shift_key());
        modifiers.set(Modifiers::ALT, mods.alt_key());
        modifiers.set(Modifiers::CONTROL, mods.control_key());
        modifiers.set(Modifiers::SUPER, mods.super_key());

        keyboard::KeyEvent {
            action: match key.state {
                ElementState::Released => keyboard::KeyAction::Release,
                ElementState::Pressed if key.repeat => keyboard::KeyAction::Repeat,
                ElementState::Pressed => keyboard::KeyAction::Press,
            },
            key: logical_key,
            unshifted,
            text: Some(key.text_with_all_modifiers().unwrap_or_default()),
            location: match key.location {
                KeyLocation::Standard => keyboard::KeyLocation::Standard,
                KeyLocation::Left => keyboard::KeyLocation::Left,
                KeyLocation::Right => keyboard::KeyLocation::Right,
                KeyLocation::Numpad => keyboard::KeyLocation::Numpad,
            },
            mods: modifiers,
            alt_composes: self.alt_composes(),
        }
    }

    fn is_modifier_key(key: &KeyEvent) -> bool {
        matches!(
            key.logical_key.as_ref(),
            Key::Named(NamedKey::Shift)
                | Key::Named(NamedKey::Control)
                | Key::Named(NamedKey::Alt)
                | Key::Named(NamedKey::Super)
        )
    }

    /// Attempt to find a binding and execute its action.
    ///
    /// The provided mode, mods, and key must match what is allowed by a binding
    /// for its action to be executed.
    fn process_key_bindings(&mut self, key: &KeyEvent) -> bool {
        let mode = BindingMode::new(self.ctx.terminal().mode(), self.ctx.search_active());
        let mods = self.ctx.modifiers().state();

        // Don't suppress char if no bindings were triggered.
        let mut suppress_chars = None;

        // We don't want the key without modifier, because it means something else most of
        // the time. However what we want is to manually lowercase the character to account
        // for both small and capital letters on regular characters at the same time.
        let logical_key = if let Key::Character(ch) = key.logical_key.as_ref() {
            // Match `Alt` bindings without `Alt` being applied, otherwise they use the
            // composed chars, which are not intuitive to bind.
            //
            // On Windows, the `Ctrl + Alt` mangles `logical_key` to unidentified values, thus
            // preventing them from being used in bindings
            //
            // For more see https://github.com/rust-windowing/winit/issues/2945.
            if (cfg!(target_os = "macos") || (cfg!(windows) && mods.control_key()))
                && mods.alt_key()
            {
                key.key_without_modifiers()
            } else {
                Key::Character(ch.to_lowercase().into())
            }
        } else {
            key.logical_key.clone()
        };

        // Get the action of a key binding.
        let mut binding_action = |binding: &KeyBinding| {
            let key = match (&binding.trigger, &logical_key) {
                (BindingKey::Scancode(_), _) => BindingKey::Scancode(key.physical_key),
                (_, code) => {
                    BindingKey::Keycode { key: code.clone(), location: key.location.into() }
                },
            };

            if binding.is_triggered_by(mode, mods, &key) {
                // Pass through the key if any of the bindings has the `ReceiveChar` action.
                *suppress_chars.get_or_insert(true) &= binding.action != Action::ReceiveChar;

                // Binding was triggered; run the action.
                Some(binding.action.clone())
            } else {
                None
            }
        };

        // Trigger matching key bindings.
        for i in 0..self.ctx.config().key_bindings().len() {
            let binding = &self.ctx.config().key_bindings()[i];
            if let Some(action) = binding_action(binding) {
                action.execute(&mut self.ctx);
            }
        }

        // Trigger key bindings for hints.
        for i in 0..self.ctx.config().hints.enabled.len() {
            let hint = &self.ctx.config().hints.enabled[i];
            let binding = match hint.binding.as_ref() {
                Some(binding) => binding.key_binding(hint),
                None => continue,
            };

            if let Some(action) = binding_action(binding) {
                action.execute(&mut self.ctx);
            }
        }

        suppress_chars.unwrap_or(false)
    }

    /// Handle key release.
    fn key_release(&mut self, key: KeyEvent, mode: TermMode, mods: ModifiersState) {
        if !mode.contains(TermMode::REPORT_EVENT_TYPES)
            || mode.contains(TermMode::VI)
            || self.ctx.search_active()
            || self.ctx.display().hint_state.active()
        {
            return;
        }

        let bytes = keyboard::encode(&self.terminal_key_event(&key, mods), mode);
        if !bytes.is_empty() {
            self.ctx.write_to_pty(bytes);
        }
    }

    /// Reset search delay.
    fn reset_search_delay(&mut self) {
        if self.ctx.search_active() {
            let timer_id =
                TimerId::for_pane(Topic::DelayedSearch, self.ctx.window().id(), self.ctx.pane_id());
            let scheduler = self.ctx.scheduler_mut();
            if let Some(timer) = scheduler.unschedule(timer_id) {
                scheduler.schedule(timer.event, TYPING_SEARCH_DELAY, false, timer.id);
            }
        }
    }
}

/// Map winit's named keys to the terminal encoder's keys.
fn named_key(named: NamedKey) -> Option<keyboard::NamedKey> {
    use keyboard::NamedKey as K;

    Some(match named {
        NamedKey::Enter => K::Enter,
        NamedKey::Tab => K::Tab,
        NamedKey::Backspace => K::Backspace,
        NamedKey::Escape => K::Escape,
        NamedKey::Space => K::Space,
        NamedKey::Insert => K::Insert,
        NamedKey::Delete => K::Delete,
        NamedKey::Home => K::Home,
        NamedKey::End => K::End,
        NamedKey::PageUp => K::PageUp,
        NamedKey::PageDown => K::PageDown,
        NamedKey::ArrowUp => K::ArrowUp,
        NamedKey::ArrowDown => K::ArrowDown,
        NamedKey::ArrowLeft => K::ArrowLeft,
        NamedKey::ArrowRight => K::ArrowRight,
        NamedKey::F1 => K::F(1),
        NamedKey::F2 => K::F(2),
        NamedKey::F3 => K::F(3),
        NamedKey::F4 => K::F(4),
        NamedKey::F5 => K::F(5),
        NamedKey::F6 => K::F(6),
        NamedKey::F7 => K::F(7),
        NamedKey::F8 => K::F(8),
        NamedKey::F9 => K::F(9),
        NamedKey::F10 => K::F(10),
        NamedKey::F11 => K::F(11),
        NamedKey::F12 => K::F(12),
        NamedKey::F13 => K::F(13),
        NamedKey::F14 => K::F(14),
        NamedKey::F15 => K::F(15),
        NamedKey::F16 => K::F(16),
        NamedKey::F17 => K::F(17),
        NamedKey::F18 => K::F(18),
        NamedKey::F19 => K::F(19),
        NamedKey::F20 => K::F(20),
        NamedKey::F21 => K::F(21),
        NamedKey::F22 => K::F(22),
        NamedKey::F23 => K::F(23),
        NamedKey::F24 => K::F(24),
        NamedKey::F25 => K::F(25),
        NamedKey::F26 => K::F(26),
        NamedKey::F27 => K::F(27),
        NamedKey::F28 => K::F(28),
        NamedKey::F29 => K::F(29),
        NamedKey::F30 => K::F(30),
        NamedKey::F31 => K::F(31),
        NamedKey::F32 => K::F(32),
        NamedKey::F33 => K::F(33),
        NamedKey::F34 => K::F(34),
        NamedKey::F35 => K::F(35),
        NamedKey::CapsLock => K::CapsLock,
        NamedKey::ScrollLock => K::ScrollLock,
        NamedKey::NumLock => K::NumLock,
        NamedKey::PrintScreen => K::PrintScreen,
        NamedKey::Pause => K::Pause,
        NamedKey::ContextMenu => K::ContextMenu,
        NamedKey::Shift => K::Shift,
        NamedKey::Control => K::Control,
        NamedKey::Alt => K::Alt,
        NamedKey::Super => K::Super,
        NamedKey::Hyper => K::Hyper,
        NamedKey::Meta => K::Meta,
        NamedKey::MediaPlay => K::MediaPlay,
        NamedKey::MediaPause => K::MediaPause,
        NamedKey::MediaPlayPause => K::MediaPlayPause,
        NamedKey::MediaStop => K::MediaStop,
        NamedKey::MediaFastForward => K::MediaFastForward,
        NamedKey::MediaRewind => K::MediaRewind,
        NamedKey::MediaTrackNext => K::MediaTrackNext,
        NamedKey::MediaTrackPrevious => K::MediaTrackPrevious,
        NamedKey::MediaRecord => K::MediaRecord,
        NamedKey::AudioVolumeDown => K::AudioVolumeDown,
        NamedKey::AudioVolumeUp => K::AudioVolumeUp,
        NamedKey::AudioVolumeMute => K::AudioVolumeMute,
        _ => return None,
    })
}
