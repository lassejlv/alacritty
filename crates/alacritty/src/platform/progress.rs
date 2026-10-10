//! Native progress indicators mirroring OSC 9;4 pane progress.
//!
//! macOS draws one bar on the Dock icon for the whole app, Windows shows progress on each
//! window's taskbar button, and other platforms have no native indicator.

use std::cmp::Ordering;

use alacritty_terminal::protocols::progress::{Progress, ProgressState};
use winit::raw_window_handle::RawWindowHandle;

/// Combine pane progress, preferring errors, then paused, normal and indeterminate work.
///
/// Panes in the same state report the least complete one.
pub fn combine(progress: impl IntoIterator<Item = Progress>) -> Progress {
    progress.into_iter().fold(Progress::default(), |combined, progress| {
        match rank(progress.state).cmp(&rank(combined.state)) {
            Ordering::Greater => progress,
            Ordering::Equal => {
                Progress { percent: combined.percent.min(progress.percent), ..combined }
            },
            Ordering::Less => combined,
        }
    })
}

fn rank(state: ProgressState) -> u8 {
    match state {
        ProgressState::Hidden => 0,
        ProgressState::Indeterminate => 1,
        ProgressState::Normal => 2,
        ProgressState::Paused => 3,
        ProgressState::Error => 4,
    }
}

/// Native progress shown for all windows.
#[derive(Default)]
pub struct SystemProgress {
    #[cfg(target_os = "macos")]
    dock: dock::Dock,
    #[cfg(windows)]
    taskbar: taskbar::Taskbar,
}

impl SystemProgress {
    /// Show each window's combined pane progress.
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_variables))]
    pub fn update(&mut self, windows: &[(RawWindowHandle, Progress)]) {
        #[cfg(target_os = "macos")]
        self.dock.show(combine(windows.iter().map(|(_, progress)| *progress)));
        #[cfg(windows)]
        self.taskbar.show(windows);
    }
}

#[cfg(target_os = "macos")]
mod dock {
    use alacritty_terminal::protocols::progress::{Progress, ProgressState};
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2_app_kit::{NSApplication, NSBezierPath, NSColor, NSImage};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    /// Progress bar drawn over the application's Dock icon.
    #[derive(Default)]
    pub struct Dock {
        icon: Option<Retained<NSImage>>,
        shown: Progress,
    }

    impl Dock {
        pub fn show(&mut self, progress: Progress) {
            let Some(mtm) = MainThreadMarker::new() else { return };
            if progress == self.shown {
                return;
            }
            self.shown = progress;

            let app = NSApplication::sharedApplication(mtm);
            if progress.state == ProgressState::Hidden {
                // SAFETY: Passing `None` restores the bundle's icon.
                unsafe { app.setApplicationIconImage(None) };
                return;
            }

            // Keep the original icon, since the application icon is replaced below.
            if self.icon.is_none() {
                self.icon = app.applicationIconImage();
            }
            let Some(icon) = self.icon.clone() else { return };

            let size = icon.size();
            let handler = RcBlock::new(move |rect: NSRect| {
                icon.drawInRect(rect);
                draw_bar(rect, progress);
                Bool::YES
            });
            let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler);
            // SAFETY: The image is fully initialized and only used on the main thread.
            unsafe { app.setApplicationIconImage(Some(&image)) };
        }
    }

    /// Draw a rounded bar along the bottom of the icon.
    fn draw_bar(rect: NSRect, progress: Progress) {
        let (width, height) = (rect.size.width, rect.size.height);
        let track = NSRect::new(
            NSPoint::new(rect.origin.x + width * 0.1, rect.origin.y + height * 0.08),
            NSSize::new(width * 0.8, height * 0.09),
        );
        let radius = track.size.height / 2.;

        NSColor::blackColor().colorWithAlphaComponent(0.55).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(track, radius, radius).fill();

        let color = match progress.state {
            ProgressState::Error => NSColor::systemRedColor(),
            ProgressState::Paused => NSColor::systemYellowColor(),
            _ => NSColor::systemBlueColor(),
        };
        // A static half-strength full bar stands in for indeterminate animation.
        let (fraction, alpha) = match progress.state {
            ProgressState::Indeterminate => (1., 0.6),
            _ => (f64::from(progress.percent) / 100., 1.),
        };
        if fraction <= 0. {
            return;
        }

        // Keep at least a full circle so low percentages remain visible.
        let mut fill = track;
        fill.size.width = (track.size.width * fraction).max(track.size.height);
        color.colorWithAlphaComponent(alpha).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(fill, radius, radius).fill();
    }
}

#[cfg(windows)]
mod taskbar {
    use std::collections::HashMap;

    use alacritty_terminal::protocols::progress::{Progress, ProgressState};
    use log::warn;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{
        ITaskbarList3, TBPF_ERROR, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
        TaskbarList,
    };
    use winit::raw_window_handle::RawWindowHandle;

    /// Progress shown on each window's taskbar button.
    #[derive(Default)]
    pub struct Taskbar {
        list: Option<ITaskbarList3>,
        failed: bool,
        shown: HashMap<isize, Progress>,
    }

    impl Taskbar {
        pub fn show(&mut self, windows: &[(RawWindowHandle, Progress)]) {
            let windows: Vec<_> = windows
                .iter()
                .filter_map(|(handle, progress)| match handle {
                    RawWindowHandle::Win32(handle) => Some((handle.hwnd.get(), *progress)),
                    _ => None,
                })
                .collect();
            self.shown.retain(|hwnd, _| windows.iter().any(|(window, _)| window == hwnd));

            for (hwnd, progress) in windows {
                if self.shown.get(&hwnd).copied().unwrap_or_default() == progress {
                    continue;
                }
                let Some(list) = self.list() else { return };
                let hwnd_ptr = HWND(hwnd as *mut _);
                let flag = match progress.state {
                    ProgressState::Hidden => TBPF_NOPROGRESS,
                    ProgressState::Normal => TBPF_NORMAL,
                    ProgressState::Error => TBPF_ERROR,
                    ProgressState::Indeterminate => TBPF_INDETERMINATE,
                    ProgressState::Paused => TBPF_PAUSED,
                };
                // SAFETY: The handle belongs to a live window owned by this thread.
                let result = unsafe {
                    list.SetProgressState(hwnd_ptr, flag).and_then(|_| {
                        if matches!(
                            progress.state,
                            ProgressState::Hidden | ProgressState::Indeterminate
                        ) {
                            return Ok(());
                        }
                        list.SetProgressValue(hwnd_ptr, progress.percent.into(), 100)
                    })
                };
                match result {
                    Ok(()) => {
                        self.shown.insert(hwnd, progress);
                    },
                    Err(err) => warn!("Unable to update taskbar progress: {err}"),
                }
            }
        }

        fn list(&mut self) -> Option<&ITaskbarList3> {
            if self.list.is_none() && !self.failed {
                // SAFETY: COM is initialized for this UI thread before creating the object;
                // repeated or mismatched initialization is harmless here.
                let list = unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
                        .and_then(|list| list.HrInit().map(|_| list))
                };
                match list {
                    Ok(list) => self.list = Some(list),
                    Err(err) => {
                        warn!("Taskbar progress is unavailable: {err}");
                        self.failed = true;
                    },
                }
            }
            self.list.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(state: ProgressState, percent: u8) -> Progress {
        Progress { state, percent }
    }

    #[test]
    fn combine_prefers_attention_states_and_least_complete_panes() {
        assert_eq!(combine([]), Progress::default());
        assert_eq!(
            combine([
                progress(ProgressState::Indeterminate, 0),
                progress(ProgressState::Normal, 80),
                progress(ProgressState::Normal, 30),
                progress(ProgressState::Hidden, 0),
            ]),
            progress(ProgressState::Normal, 30),
        );
        assert_eq!(
            combine([progress(ProgressState::Normal, 10), progress(ProgressState::Error, 60)]),
            progress(ProgressState::Error, 60),
        );
        assert_eq!(
            combine([progress(ProgressState::Paused, 20), progress(ProgressState::Normal, 5)]),
            progress(ProgressState::Paused, 20),
        );
    }
}
