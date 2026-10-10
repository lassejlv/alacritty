//! Embeddable Alacritty terminal engine with a C ABI.
//!
//! The engine combines terminal emulation, PTY management, keyboard and mouse encoding and the
//! protocol handling of the desktop application: Kitty keyboard, graphics and clipboard
//! protocols, OSC 52 clipboard, OSC 7 working directories, OSC 9/99 notifications, OSC 9;4
//! progress, OSC 133 shell integration and OSC 7501 program status. Hosts provide windows,
//! fonts, rendering and native clipboard access.
//!
//! Rust hosts use [`Engine`]. Other languages use the functions in [`ffi`], declared in
//! `include/alacritty.h`.

#![cfg_attr(clippy, deny(warnings))]

pub mod colors;
pub mod engine;
pub mod ffi;
pub mod snapshot;

pub use alacritty_terminal;
pub use alacritty_terminal::input::{keyboard, mouse};
pub use engine::{
    ClipboardProvider, Engine, HostEvent, Link, Options, PtyOptions, WheelAction, Writer,
};
