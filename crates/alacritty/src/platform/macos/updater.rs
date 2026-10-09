//! Sparkle's native updater, loaded only from a packaged app's embedded framework.

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenuItem};
use objc2_foundation::{NSBundle, NSString, ns_string};

/// Keep Sparkle and its menu target alive for the entire application event loop.
pub struct Updater {
    _controller: Retained<AnyObject>,
    menu_item: Retained<NSMenuItem>,
}

impl Updater {
    pub fn new() -> Option<Self> {
        let mtm = MainThreadMarker::new().expect("Sparkle must start on the main thread");
        let host = NSBundle::mainBundle();
        // Command-line/debug binaries are not independently installable app bundles.
        host.objectForInfoDictionaryKey(ns_string!("SUFeedURL"))?;
        let frameworks = host.privateFrameworksPath()?;
        let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
        let Some(framework) = NSBundle::bundleWithPath(&path) else {
            log::error!("Updates unavailable: the embedded Sparkle framework is missing");
            return None;
        };

        // SAFETY: Load only the pinned framework embedded inside this app and signed with
        // the app's Developer ID. All AppKit/Sparkle calls below run on the main thread.
        if let Err(err) = unsafe { framework.loadAndReturnError() } {
            log::error!("Unable to load Sparkle: {err}");
            return None;
        }
        let Some(class) = AnyClass::get(c"SPUStandardUpdaterController") else {
            log::error!("Updates unavailable: Sparkle's updater controller is missing");
            return None;
        };
        let menu = NSApplication::sharedApplication(mtm).mainMenu()?.itemAtIndex(0)?.submenu()?;

        // SAFETY: These are the documented Sparkle 2 controller selectors and signatures.
        // `init` transfers the allocation into an owned controller; nil delegates are supported.
        // The controller owns its updater and standard user interface.
        let controller: Retained<AnyObject> = unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class, alloc];
            msg_send![allocated, initWithStartingUpdater: false,
                updaterDelegate: None::<&AnyObject>, userDriverDelegate: None::<&AnyObject>]
        };
        // SAFETY: The menu invokes checkForUpdates: with its sender. Sparkle also implements
        // menu validation to disable the action while a check or installation is in progress.
        let menu_item = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Check for Updates…"),
                Some(sel!(checkForUpdates:)),
                ns_string!(""),
            );
            item.setTarget(Some(&controller));
            item
        };
        menu.insertItem_atIndex(&menu_item, 1.min(menu.numberOfItems()));

        // SAFETY: The controller has been initialized above and is retained in Self. Start
        // only after Winit has created its application menu and finished launching AppKit.
        unsafe {
            let _: () = msg_send![&*controller, startUpdater];
        }
        log::info!("Sparkle updater started; installation requires user confirmation");
        Some(Self { _controller: controller, menu_item })
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        // SAFETY: AppKit's target reference is non-owning; clear it before releasing Sparkle.
        unsafe { self.menu_item.setTarget(None) };
    }
}
