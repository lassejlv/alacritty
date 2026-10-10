//! Host palette and cell color resolution matching the desktop renderer.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::{COUNT, Colors};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// Factor for automatic computation of dim colors.
pub const DIM_FACTOR: f32 = 0.66;

/// All 269 terminal colors, indexed like [`Colors`].
#[derive(Clone, Copy, Debug)]
pub struct Palette(pub [Rgb; COUNT]);

const NORMAL: [Rgb; 8] = [
    rgb(0x18, 0x18, 0x18),
    rgb(0xac, 0x42, 0x42),
    rgb(0x90, 0xa9, 0x59),
    rgb(0xf4, 0xbf, 0x75),
    rgb(0x6a, 0x9f, 0xb5),
    rgb(0xaa, 0x75, 0x9f),
    rgb(0x75, 0xb5, 0xaa),
    rgb(0xd8, 0xd8, 0xd8),
];

const BRIGHT: [Rgb; 8] = [
    rgb(0x6b, 0x6b, 0x6b),
    rgb(0xc5, 0x55, 0x55),
    rgb(0xaa, 0xc4, 0x74),
    rgb(0xfe, 0xca, 0x88),
    rgb(0x82, 0xb8, 0xc8),
    rgb(0xc2, 0x8c, 0xb8),
    rgb(0x93, 0xd3, 0xc3),
    rgb(0xf8, 0xf8, 0xf8),
];

impl Default for Palette {
    fn default() -> Self {
        let mut colors = [Rgb::default(); COUNT];
        colors[..8].copy_from_slice(&NORMAL);
        colors[8..16].copy_from_slice(&BRIGHT);

        let mut index = 16;
        for r in 0..6u8 {
            for g in 0..6u8 {
                for b in 0..6u8 {
                    let level = |value: u8| if value == 0 { 0 } else { value * 40 + 55 };
                    colors[index] = rgb(level(r), level(g), level(b));
                    index += 1;
                }
            }
        }
        for i in 0..24u8 {
            let value = i * 10 + 8;
            colors[232 + i as usize] = rgb(value, value, value);
        }

        let mut palette = Self(colors);
        palette.set_primary(rgb(0xd8, 0xd8, 0xd8), rgb(0x18, 0x18, 0x18));
        palette
    }
}

impl Palette {
    /// Set foreground, background and the colors derived from them.
    pub fn set_primary(&mut self, foreground: Rgb, background: Rgb) {
        self.0[NamedColor::Foreground as usize] = foreground;
        self.0[NamedColor::Background as usize] = background;
        self.0[NamedColor::Cursor as usize] = foreground;
        self.0[NamedColor::BrightForeground as usize] = foreground;
        self.0[NamedColor::DimForeground as usize] = foreground * DIM_FACTOR;
        self.derive_dim();
    }

    /// Derive dim colors from the normal ANSI colors.
    pub fn derive_dim(&mut self) {
        for i in 0..8 {
            self.0[NamedColor::DimBlack as usize + i] = self.0[i] * DIM_FACTOR;
        }
    }

    /// Color at `index`, preferring terminal overrides from OSC 4/10/11/12.
    pub fn get(&self, overrides: &Colors, index: usize) -> Rgb {
        overrides[index].unwrap_or(self.0[index])
    }

    /// Resolve a cell's foreground color.
    pub fn foreground(
        &self,
        overrides: &Colors,
        color: Color,
        flags: Flags,
        bold_bright: bool,
    ) -> Rgb {
        match color {
            Color::Spec(rgb) if flags.contains(Flags::DIM) => rgb * DIM_FACTOR,
            Color::Spec(rgb) => rgb,
            Color::Named(ansi) => match (bold_bright, flags & Flags::DIM_BOLD) {
                (true, Flags::BOLD) => self.get(overrides, ansi.to_bright() as usize),
                (_, Flags::DIM) | (false, Flags::DIM_BOLD) => {
                    self.get(overrides, ansi.to_dim() as usize)
                },
                _ => self.get(overrides, ansi as usize),
            },
            Color::Indexed(index) => {
                let index = match (bold_bright, flags & Flags::DIM_BOLD, index) {
                    (true, Flags::BOLD, 0..=7) => index as usize + 8,
                    (false, Flags::DIM, 8..=15) => index as usize - 8,
                    (false, Flags::DIM, 0..=7) => NamedColor::DimBlack as usize + index as usize,
                    _ => index as usize,
                };
                self.get(overrides, index)
            },
        }
    }

    /// Resolve a cell's background color.
    pub fn background(&self, overrides: &Colors, color: Color) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Named(ansi) => self.get(overrides, ansi as usize),
            Color::Indexed(index) => self.get(overrides, index as usize),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// Pack a color as `0xRRGGBB`.
pub fn pack(rgb: Rgb) -> u32 {
    (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
}

/// Unpack a `0xRRGGBB` color.
pub fn unpack(value: u32) -> Rgb {
    rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
