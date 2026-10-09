//! Native config dialogs. Migration only writes after an explicit preview/import action.
use std::cell::Cell;
use std::path::PathBuf;

thread_local! { static MIGRATING: Cell<bool> = const { Cell::new(false) }; }

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAlert, NSApplication, NSOpenPanel, NSScrollView, NSTextView, NSWorkspace};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL, ns_string};
use winit::event_loop::EventLoopProxy;

use crate::config::{self, UiConfig};
use crate::event::{Event, EventType};
use crate::macos::ghostty;

pub fn open_config(config: &UiConfig, proxy: &EventLoopProxy<Event>) {
    match config::editable_config(config) {
        Ok(path) => {
            let _ = proxy.send_event(Event::new(EventType::ConfigReload(path.clone()), None));
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            if !NSWorkspace::sharedWorkspace().openURL(&url) {
                show_error("No application could open this config file.");
            }
        },
        Err(error) => show_error(&error.to_string()),
    }
}

pub fn migrate_ghostty(config: &UiConfig, proxy: &EventLoopProxy<Event>) {
    if MIGRATING.with(|active| active.replace(true)) {
        return;
    }
    let paths = config.config_paths.clone();
    let proxy = proxy.clone();
    // Let Winit finish dispatching the menu event before entering AppKit's modal loop.
    dispatch2::DispatchQueue::main().exec_async(move || {
        migrate_dialog(&paths, &proxy);
        MIGRATING.with(|active| active.set(false));
    });
}

fn migrate_dialog(paths: &[PathBuf], proxy: &EventLoopProxy<Event>) {
    let mtm = MainThreadMarker::new().expect("Config dialogs must run on the main thread");
    let home = match home::home_dir() {
        Some(home) => home,
        None => {
            show_error("Your home directory could not be found.");
            return;
        },
    };
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let ghostty = xdg.join("ghostty");
    let support = home.join("Library/Application Support/com.mitchellh.ghostty");
    let candidates = [
        support.join("config.ghostty"),
        support.join("config"),
        ghostty.join("config.ghostty"),
        ghostty.join("config"),
    ];
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setTitle(Some(ns_string!("Migrate Ghostty Config")));
    panel.setMessage(Some(ns_string!("Choose a Ghostty config to preview. Your existing Alacritty config will be backed up before import.")));
    panel.setPrompt(Some(ns_string!("Preview")));
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
            &path.parent().unwrap().to_string_lossy(),
        ))));
        panel.setNameFieldStringValue(&NSString::from_str(
            &path.file_name().unwrap().to_string_lossy(),
        ));
    }
    let response = panel.runModal();
    panel.orderOut(None);
    if response != 1 {
        return;
    }
    let Some(source) =
        panel.URL().and_then(|url| url.path()).map(|path| PathBuf::from(path.to_string()))
    else {
        return;
    };
    let themes = vec![
        ghostty.join("themes"),
        source.parent().unwrap_or(&ghostty).join("themes"),
        home.join("Applications/Ghostty.app/Contents/Resources/ghostty/themes"),
        PathBuf::from("/Applications/Ghostty.app/Contents/Resources/ghostty/themes"),
    ];
    let dark = NSApplication::sharedApplication(mtm)
        .effectiveAppearance()
        .name()
        .to_string()
        .contains("Dark");
    let migration = match ghostty::convert(&source, &themes, dark) {
        Ok(migration) => migration,
        Err(error) => {
            show_error(&error);
            return;
        },
    };
    let destination =
        paths.first().cloned().unwrap_or_else(|| xdg.join("alacritty/alacritty.toml"));
    let prepared = match migration.prepare(destination) {
        Ok(prepared) => prepared,
        Err(error) => {
            show_error(&error);
            return;
        },
    };
    let report = format!(
        "Source: {}\nDestination: {}\n\n{}",
        source.display(),
        prepared.path.display(),
        migration.report()
    );
    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Migrate Ghostty Config"));
    alert.setInformativeText(&NSString::from_str(&format!("{} settings can be imported. Matching settings will be replaced; unrelated Alacritty settings will be kept. Review any unsupported settings below.", migration.mapped)));
    if migration.mapped > 0 {
        alert.addButtonWithTitle(ns_string!("Import"));
    }
    alert.addButtonWithTitle(ns_string!("Cancel"));
    let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(600., 330.));
    // Both views are owned here and on the main thread; the alert retains the scroll
    // view and the scroll view retains its document for the entire modal presentation.
    let scroll = {
        let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
        text.setString(&NSString::from_str(&report));
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setDocumentView(Some(&text));
        scroll
    };
    alert.setAccessoryView(Some(&scroll));
    if alert.runModal() != 1000 || migration.mapped == 0 {
        return;
    }
    match prepared.apply() {
        Ok(backup) => {
            let _ =
                proxy.send_event(Event::new(EventType::ConfigReload(prepared.path.clone()), None));
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("Ghostty settings imported"));
            let backup =
                backup.map(|path| format!("\n\nBackup: {}", path.display())).unwrap_or_default();
            alert.setInformativeText(&NSString::from_str(&format!("{} settings imported into {}. {} compatibility notes were shown in the preview. Settings that affect startup apply to new terminals.{backup}", migration.mapped, prepared.path.display(), migration.notes.len())));
            alert.addButtonWithTitle(ns_string!("Done"));
            alert.addButtonWithTitle(ns_string!("Open Config"));
            if alert.runModal() == 1001 {
                let url =
                    NSURL::fileURLWithPath(&NSString::from_str(&prepared.path.to_string_lossy()));
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        },
        Err(error) => show_error(&error),
    }
}

fn show_error(message: &str) {
    let alert = NSAlert::new(MainThreadMarker::new().unwrap());
    alert.setMessageText(ns_string!("Config could not be changed"));
    alert.setInformativeText(&NSString::from_str(message));
    alert.addButtonWithTitle(ns_string!("OK"));
    alert.runModal();
}
