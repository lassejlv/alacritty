//! PTY process ownership and terminal I/O, independent of desktop windows.

#![cfg_attr(clippy, deny(warnings))]

pub mod event_loop;
pub mod events;
pub mod pty;
mod session;
pub mod sync;
pub mod thread;

pub use session::Session;
