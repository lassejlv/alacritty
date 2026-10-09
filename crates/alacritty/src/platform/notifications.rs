//! Native notification delivery with callbacks routed to the originating pane.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeNotifications;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::NativeNotifications;
#[cfg(any(all(unix, not(target_os = "macos")), all(test, target_os = "macos")))]
#[cfg_attr(
    all(test, target_os = "macos"),
    expect(dead_code, reason = "Compile the XDG backend on macOS test hosts.")
)]
mod xdg;
#[cfg(all(unix, not(target_os = "macos")))]
pub use xdg::NativeNotifications;

#[derive(Clone, Debug)]
pub enum Feedback {
    Activated { serial: u64, button: Option<usize> },
    Closed(u64),
    Failed(u64),
    Alive { query: String, serials: Vec<u64> },
}
