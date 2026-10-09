//! Alacritty - The GPU Enhanced Terminal.

#![cfg_attr(clippy, deny(warnings))]

pub mod event;
pub mod graphics;
pub mod grid;
pub mod index;
pub mod protocols;
pub use protocols::{clipboard, program_status};
pub mod selection;
pub mod term;
pub mod vi_mode;

pub use crate::grid::Grid;
pub use crate::term::Term;
pub use vte;
