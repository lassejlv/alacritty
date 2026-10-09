use crate::color::Rgb;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::{Flags, Hyperlink};

/// Cell ready for rendering.
#[derive(Clone, Debug)]
pub struct RenderableCell {
    pub character: char,
    pub point: Point<usize>,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bg_alpha: f32,
    pub underline: Rgb,
    pub flags: Flags,
    pub extra: Option<Box<RenderableCellExtra>>,
}

/// Extra storage with rarely present fields for [`RenderableCell`], to reduce the cell size we
/// pass around.
#[derive(Clone, Debug)]
pub struct RenderableCellExtra {
    pub zerowidth: Option<Vec<char>>,
    pub hyperlink: Option<Hyperlink>,
}

impl RenderableCell {
    /// Check if cell contains any renderable content.
    pub fn is_empty(&self) -> bool {
        self.bg_alpha == 0.
            && self.character == ' '
            && self.extra.is_none()
            && !self.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
    }
}
