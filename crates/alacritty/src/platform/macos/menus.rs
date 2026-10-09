//! Native commands are queued into Winit rather than mutating terminals from AppKit callbacks.
use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSMenuItemValidation, NSView,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
use winit::dpi::PhysicalPosition;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::RawWindowHandle;
use winit::window::WindowId;

use crate::app::{Event, EventType};
use crate::config::{Action, BindingKey, UiConfig};
use crate::platform::window::Window;
use crate::workspace::layout::PaneId;

pub fn is_context_click(button: MouseButton, modifiers: ModifiersState, mouse_mode: bool) -> bool {
    let secondary =
        button == MouseButton::Right || (button == MouseButton::Left && modifiers.control_key());
    secondary && (!mouse_mode || modifiers.shift_key())
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    CreateNewWindow,
    CreateNewTab,
    SplitRight,
    SplitDown,
    FocusNextPane,
    FocusPreviousPane,
    ClosePane,
    Quit,
    Copy,
    Paste,
    SelectAll,
    ClearSelection,
    SearchForward,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ClearHistory,
    ToggleFullscreen,
    Minimize,
    SelectNextTab,
    SelectPreviousTab,
    OpenConfig,
    ReloadConfig,
    MigrateGhostty,
}

impl Command {
    pub fn action(self) -> Option<Action> {
        Some(match self {
            Self::CreateNewWindow => Action::CreateNewWindow,
            Self::CreateNewTab => Action::CreateNewTab,
            Self::SplitRight => Action::SplitRight,
            Self::SplitDown => Action::SplitDown,
            Self::FocusNextPane => Action::FocusNextPane,
            Self::FocusPreviousPane => Action::FocusPreviousPane,
            Self::ClosePane => Action::ClosePane,
            Self::Quit => Action::Quit,
            Self::Copy => Action::Copy,
            Self::Paste => Action::Paste,
            Self::SelectAll => Action::SelectAll,
            Self::ClearSelection => Action::ClearSelection,
            Self::SearchForward => Action::SearchForward,
            Self::IncreaseFontSize => Action::IncreaseFontSize,
            Self::DecreaseFontSize => Action::DecreaseFontSize,
            Self::ResetFontSize => Action::ResetFontSize,
            Self::ClearHistory => Action::ClearHistory,
            Self::ToggleFullscreen => Action::ToggleFullscreen,
            Self::Minimize => Action::Minimize,
            Self::SelectNextTab => Action::SelectNextTab,
            Self::SelectPreviousTab => Action::SelectPreviousTab,
            Self::OpenConfig => Action::OpenConfig,
            _ => return None,
        })
    }
}

struct TargetState {
    proxy: EventLoopProxy<Event>,
    commands: RefCell<Vec<Command>>,
    windows: RefCell<Vec<(isize, bool)>>,
    context: Option<(WindowId, PaneId)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. This target is main-thread-only.
    #[unsafe(super = NSObject)]
    #[name = "AlacrittyMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(performAlacrittyCommand:))]
        fn perform(&self, sender: &NSMenuItem) {
            let Some(command) = self.ivars().commands.borrow().get(sender.tag() as usize).cloned() else { return };
            if let Some((window, pane)) = self.ivars().context {
                let event = Event::new(EventType::MacosAction(command), window).with_pane(pane);
                let _ = self.ivars().proxy.send_event(event);
                return;
            }
            let app = NSApplication::sharedApplication(self.mtm());
            if let Some(selector) = self.native_edit_action(command) {
                // SAFETY: Standard text-editing selectors are sent through AppKit's responder
                // chain only when a native text control, rather than a terminal, is focused.
                unsafe { app.sendAction_to_from(selector, None, Some(sender)); }
                return;
            }
            let number = app.keyWindow().map(|w| w.windowNumber());
            let _ = self.ivars().proxy.send_event(Event::new(EventType::MacosMenu(command, number), None));
        }
    }

    unsafe impl NSMenuItemValidation for Target {
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool {
            let commands = self.ivars().commands.borrow();
            commands.get(item.tag() as usize).is_some_and(|command| self.native_edit_action(*command).is_some() || match command {
                Command::CreateNewWindow | Command::OpenConfig
                    | Command::ReloadConfig | Command::MigrateGhostty => true,
                command => {
                    let number = NSApplication::sharedApplication(self.mtm()).keyWindow().map(|w| w.windowNumber());
                    self.ivars().windows.borrow().iter().any(|(id, tabs)| {
                        Some(*id) == number && (!matches!(command, Command::CreateNewTab) || *tabs)
                    })
                },
            })
        }
    }
);

impl Target {
    fn native_edit_action(&self, command: Command) -> Option<Sel> {
        let selector = match command {
            Command::Copy => sel!(copy:),
            Command::Paste => sel!(paste:),
            Command::SelectAll => sel!(selectAll:),
            _ => return None,
        };
        let window = NSApplication::sharedApplication(self.mtm()).keyWindow()?;
        if self.ivars().windows.borrow().iter().any(|(number, _)| *number == window.windowNumber())
        {
            return None;
        }
        window.firstResponder()?.respondsToSelector(selector).then_some(selector)
    }
}

pub struct Menus {
    target: Retained<Target>,
    items: Vec<Retained<NSMenuItem>>,
    shortcuts: Vec<(String, NSEventModifierFlags)>,
}

impl Menus {
    pub fn new(proxy: EventLoopProxy<Event>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let bar = NSApplication::sharedApplication(mtm).mainMenu()?;
        let app_menu = bar.itemAtIndex(0)?.submenu()?;
        let allocated = Target::alloc(mtm).set_ivars(TargetState {
            proxy,
            commands: RefCell::default(),
            windows: RefCell::default(),
            context: None,
        });
        // SAFETY: NSObject's init initializes the allocated main-thread target.
        let target = unsafe { msg_send![super(allocated), init] };
        let mut menus = Self { target, items: Vec::new(), shortcuts: Vec::new() };
        let cmd = NSEventModifierFlags::Command;
        let shift = cmd | NSEventModifierFlags::Shift;
        let control = cmd | NSEventModifierFlags::Control;
        menus.add(&app_menu, "Open Config…", ",", cmd, Command::OpenConfig);
        menus.add(&app_menu, "Reload Config", ",", shift, Command::ReloadConfig);
        menus.add(&app_menu, "Migrate Ghostty Config…", "", cmd, Command::MigrateGhostty);

        // Place configuration commands before Services/Hide/Quit in the application menu.
        for (index, item) in menus.items.iter().enumerate() {
            app_menu.removeItem(item);
            app_menu.insertItem_atIndex(item, (index + 1) as isize);
        }
        let file = Self::submenu(&bar, "File", mtm);
        menus.add(&file, "New Window", "n", cmd, Command::CreateNewWindow);
        menus.add(&file, "New Tab", "t", cmd, Command::CreateNewTab);
        file.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&file, "Split Right", "d", cmd, Command::SplitRight);
        menus.add(&file, "Split Down", "d", shift, Command::SplitDown);
        file.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&file, "Close Pane", "w", cmd, Command::ClosePane);
        menus.add(&file, "Close Tab", "w", shift, Command::Quit);

        let edit = Self::submenu(&bar, "Edit", mtm);
        menus.add(&edit, "Copy", "c", cmd, Command::Copy);
        menus.add(&edit, "Paste", "v", cmd, Command::Paste);
        menus.add(&edit, "Select All", "a", cmd, Command::SelectAll);
        edit.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&edit, "Find…", "f", cmd, Command::SearchForward);

        let view = Self::submenu(&bar, "View", mtm);
        menus.add(&view, "Increase Font Size", "=", cmd, Command::IncreaseFontSize);
        menus.add(&view, "Decrease Font Size", "-", cmd, Command::DecreaseFontSize);
        menus.add(&view, "Reset Font Size", "0", cmd, Command::ResetFontSize);
        view.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&view, "Clear Scrollback", "", cmd, Command::ClearHistory);
        menus.add(&view, "Toggle Full Screen", "f", control, Command::ToggleFullscreen);

        let window = Self::submenu(&bar, "Window", mtm);
        menus.add(
            &window,
            "Next Pane",
            "\u{f703}",
            cmd | NSEventModifierFlags::Option,
            Command::FocusNextPane,
        );
        menus.add(
            &window,
            "Previous Pane",
            "\u{f702}",
            cmd | NSEventModifierFlags::Option,
            Command::FocusPreviousPane,
        );
        window.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&window, "Minimize", "m", cmd, Command::Minimize);
        menus.add(&window, "Next Tab", "]", shift, Command::SelectNextTab);
        menus.add(&window, "Previous Tab", "[", shift, Command::SelectPreviousTab);
        Some(menus)
    }

    pub fn set_windows(&self, windows: Vec<(isize, bool)>) {
        *self.target.ivars().windows.borrow_mut() = windows;
    }

    /// Show a native menu with commands bound to the pane that was clicked.
    pub fn popup(
        window: &Window,
        pane: PaneId,
        position: PhysicalPosition<f64>,
        has_selection: bool,
        config: &UiConfig,
        proxy: EventLoopProxy<Event>,
    ) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let RawWindowHandle::AppKit(handle) = window.raw_window_handle() else { return };
        // SAFETY: Winit owns the live NSView, and this method runs on the main thread.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let allocated = Target::alloc(mtm).set_ivars(TargetState {
            proxy,
            commands: RefCell::default(),
            windows: RefCell::default(),
            context: Some((window.id(), pane)),
        });
        // SAFETY: NSObject's init initializes the allocated main-thread target.
        let target = unsafe { msg_send![super(allocated), init] };
        let mut menus = Self { target, items: Vec::new(), shortcuts: Vec::new() };
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Terminal"));
        menu.setAutoenablesItems(false);
        let cmd = NSEventModifierFlags::Command;
        menus.add(&menu, "Copy", "c", cmd, Command::Copy);
        menus.add(&menu, "Paste", "v", cmd, Command::Paste);
        menus.add(&menu, "Select All", "a", cmd, Command::SelectAll);
        menus.add(&menu, "Clear Selection", "", cmd, Command::ClearSelection);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&menu, "Find…", "f", cmd, Command::SearchForward);
        menus.add(&menu, "Clear Scrollback", "", cmd, Command::ClearHistory);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menus.add(&menu, "Split Right", "d", cmd, Command::SplitRight);
        menus.add(&menu, "Split Down", "d", cmd | NSEventModifierFlags::Shift, Command::SplitDown);
        menus.add(&menu, "New Tab", "t", cmd, Command::CreateNewTab);
        menus.add(&menu, "New Window", "n", cmd, Command::CreateNewWindow);
        for (item, command) in menus.items.iter().zip(menus.target.ivars().commands.borrow().iter())
        {
            let enabled = match command {
                Command::Copy | Command::ClearSelection => has_selection,
                Command::CreateNewTab => {
                    config.window.decorations != crate::config::window::Decorations::None
                },
                _ => true,
            };
            item.setEnabled(enabled);
        }
        menus.configure_shortcuts(config);

        let bounds = view.bounds();
        let x = bounds.origin.x + position.x / window.scale_factor;
        let y = position.y / window.scale_factor;
        let y = bounds.origin.y + if view.isFlipped() { y } else { bounds.size.height - y };
        // AppKit tracks the menu synchronously. Keep its target alive until tracking finishes.
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), Some(view));
    }

    /// Don't let native accelerators swallow a user's remapped terminal shortcuts.
    pub fn configure_shortcuts(&self, config: &UiConfig) {
        for ((item, (key, flags)), command) in
            self.items.iter().zip(&self.shortcuts).zip(self.target.ivars().commands.borrow().iter())
        {
            let mut mods = ModifiersState::empty();
            if flags.contains(NSEventModifierFlags::Command) {
                mods |= ModifiersState::SUPER;
            }
            if flags.contains(NSEventModifierFlags::Shift) {
                mods |= ModifiersState::SHIFT;
            }
            if flags.contains(NSEventModifierFlags::Option) {
                mods |= ModifiersState::ALT;
            }
            if flags.contains(NSEventModifierFlags::Control) {
                mods |= ModifiersState::CONTROL;
            }
            let matching: Vec<_> = config
                .key_bindings()
                .iter()
                .filter(|binding| {
                    binding.mods == mods
                        && match &binding.trigger {
                            BindingKey::Keycode { key: Key::Character(character), .. } => {
                                character.as_str().eq_ignore_ascii_case(key)
                            },
                            BindingKey::Keycode {
                                key: Key::Named(NamedKey::ArrowRight), ..
                            } => key == "\u{f703}",
                            BindingKey::Keycode {
                                key: Key::Named(NamedKey::ArrowLeft), ..
                            } => key == "\u{f702}",
                            _ => false,
                        }
                })
                .collect();
            let enabled = match command.action() {
                Some(action) => matching.iter().any(|binding| binding.action == action),
                None => matching.is_empty(),
            };
            item.setKeyEquivalent(&NSString::from_str(if enabled { key } else { "" }));
        }
    }

    fn submenu(bar: &NSMenu, title: &str, mtm: MainThreadMarker) -> Retained<NSMenu> {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(title));
        item.setSubmenu(Some(&menu));
        bar.addItem(&item);
        menu
    }

    fn add(
        &mut self,
        menu: &NSMenu,
        title: &str,
        key: &str,
        modifiers: NSEventModifierFlags,
        command: Command,
    ) {
        let mtm = self.target.mtm();
        // SAFETY: The retained target implements this selector with an NSMenuItem sender.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(performAlacrittyCommand:)),
                &NSString::from_str(key),
            )
        };
        item.setTag(self.target.ivars().commands.borrow().len() as isize);
        self.target.ivars().commands.borrow_mut().push(command);
        item.setKeyEquivalentModifierMask(modifiers);
        // SAFETY: Target is retained by Menus until all item targets are cleared in Drop.
        unsafe { item.setTarget(Some(&self.target)) };
        menu.addItem(&item);
        self.items.push(item);
        self.shortcuts.push((key.into(), modifiers));
    }
}

impl Drop for Menus {
    fn drop(&mut self) {
        for item in &self.items {
            // SAFETY: Clear AppKit's non-owning references before releasing the target.
            unsafe { item.setTarget(None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_click_preserves_application_mouse_reporting() {
        assert!(is_context_click(MouseButton::Right, ModifiersState::empty(), false));
        assert!(!is_context_click(MouseButton::Right, ModifiersState::empty(), true));
        assert!(is_context_click(MouseButton::Right, ModifiersState::SHIFT, true));
        assert!(is_context_click(MouseButton::Left, ModifiersState::CONTROL, false));
        assert!(!is_context_click(MouseButton::Left, ModifiersState::CONTROL, true));
        assert!(!is_context_click(MouseButton::Left, ModifiersState::empty(), false));
        assert!(!is_context_click(MouseButton::Middle, ModifiersState::empty(), false));
    }

    #[test]
    fn selection_menu_commands_use_terminal_actions() {
        assert_eq!(Command::SelectAll.action(), Some(Action::SelectAll));
        assert_eq!(Command::ClearSelection.action(), Some(Action::ClearSelection));
    }
}
