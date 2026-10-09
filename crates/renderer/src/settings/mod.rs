//! Drawing settings supplied by the desktop application.
use alacritty_config_derive::ConfigDeserialize;
use serde::Serialize;

pub mod font;

/// A delta for a point in a 2 dimensional plane.
#[derive(ConfigDeserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delta<T: Default> {
    /// Horizontal change.
    pub x: T,
    /// Vertical change.
    pub y: T,
}

/// The renderer configuration options.
#[derive(ConfigDeserialize, Serialize, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RendererPreference {
    /// OpenGL 3.3 renderer.
    Glsl3,

    /// GLES 2 renderer, with optional extensions like dual source blending.
    Gles2,

    /// Pure GLES 2 renderer.
    Gles2Pure,
}
