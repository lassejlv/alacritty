//! Native commands are queued into Winit rather than mutating terminals from AppKit callbacks.
use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSMenuItemValidation,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{Key, ModifiersState};

use crate::config::{Action, BindingKey, UiConfig};
use crate::event::{Event, EventType};

#[derive(Clone, Copy, Debug)]
pub enum Command {
    CreateNewWindow,
    CreateNewTab,
    Quit,
    Copy,
    Paste,
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
            Self::Quit => Action::Quit,
            Self::Copy => Action::Copy,
            Self::Paste => Action::Paste,
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
            let app = NSApplication::sharedApplication(self.mtm());
            if let Some(selector) = self.native_edit_action(command) {
                // SAFETY: Standard copy:/paste: selectors are sent through AppKit's responder
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
        menus.add(&file, "Close Tab", "w", cmd, Command::Quit);

        let edit = Self::submenu(&bar, "Edit", mtm);
        menus.add(&edit, "Copy", "c", cmd, Command::Copy);
        menus.add(&edit, "Paste", "v", cmd, Command::Paste);
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
        menus.add(&window, "Minimize", "m", cmd, Command::Minimize);
        menus.add(&window, "Next Tab", "]", shift, Command::SelectNextTab);
        menus.add(&window, "Previous Tab", "[", shift, Command::SelectPreviousTab);
        Some(menus)
    }

    pub fn set_windows(&self, windows: Vec<(isize, bool)>) {
        *self.target.ivars().windows.borrow_mut() = windows;
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
            let matching: Vec<_> = config.key_bindings().iter().filter(|binding| {
                binding.mods == mods && matches!(&binding.trigger,
                    BindingKey::Keycode { key: Key::Character(character), .. } if character.as_str().eq_ignore_ascii_case(key))
            }).collect();
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
