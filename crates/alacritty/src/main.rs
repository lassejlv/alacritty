//! Alacritty - The GPU Enhanced Terminal.

#![cfg_attr(clippy, deny(warnings))]
// With the default subsystem, 'console', windows creates an additional console
// window for the program.
// This is silently ignored on non-windows systems.
// See https://msdn.microsoft.com/en-us/library/4cc7ya5b.aspx for more details.
#![windows_subsystem = "windows"]

#[cfg(not(any(feature = "x11", feature = "wayland", target_os = "macos", windows)))]
compile_error!(r#"at least one of the "x11"/"wayland" features must be enabled"#);

mod app;
mod cli;
mod clipboard;
mod config;
mod input;
#[cfg(unix)]
mod ipc;
mod logging;
mod platform;
mod presentation;
mod string;
mod workspace;

use alacritty_renderer as renderer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
