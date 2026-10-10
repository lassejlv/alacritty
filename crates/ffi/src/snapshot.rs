//! Owned copies of visible terminal content for rendering outside the terminal lock.

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::graphics::media::{GraphicsImage, graphics_display_layout};
use alacritty_terminal::graphics::{KittyGraphicsRenderPlacement, unicode};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermDamage, TermMode, point_to_viewport};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use crate::colors::{Palette, pack};
use crate::engine::{Listener, Options};

/// The cell is part of the selection.
pub const CELL_SELECTED: u32 = 1 << 16;
/// The cell uses the terminal's default background and may be drawn transparent.
pub const CELL_DEFAULT_BACKGROUND: u32 = 1 << 17;
/// The cell has an explicit SGR 58 underline color.
pub const CELL_UNDERLINE_COLOR: u32 = 1 << 18;
/// The cell is a Kitty Unicode placeholder; draw its image instead of a glyph.
pub const CELL_IMAGE_PLACEHOLDER: u32 = 1 << 19;

/// A visible grid cell with resolved colors.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub codepoint: u32,
    /// Foreground as `0xRRGGBB`, after bold, dim, inverse and hidden attributes.
    pub fg: u32,
    pub bg: u32,
    pub underline_color: u32,
    /// Raw cell flags in the low 16 bits, `CELL_*` flags above.
    pub flags: u32,
    /// Index of the first combining character in the snapshot's codepoint table.
    pub zerowidth_index: u32,
    pub zerowidth_count: u16,
    pub reserved: u16,
    /// One-based hyperlink index, or `0` without a hyperlink.
    pub hyperlink: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub column: u32,
    pub row: u32,
    /// [`CursorShape`] as `0` block, `1` underline, `2` beam, `3` hollow block, `4` hidden.
    pub shape: u32,
    pub color: u32,
    /// Color for text under a block cursor.
    pub text_color: u32,
    pub blinking: bool,
    /// The cursor covers a double-width character.
    pub wide: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineDamage {
    pub row: u32,
    pub left: u32,
    pub right: u32,
}

/// A visible Kitty graphics placement with destination and source rectangles.
///
/// Destination coordinates are pixels relative to the top-left of the cell grid.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Image {
    pub image_id: u32,
    pub placement_id: u32,
    /// Changes whenever the image pixels change, for texture caching.
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA pixels, or null when only `png` is available.
    pub rgba: *const u8,
    pub rgba_len: usize,
    pub png: *const u8,
    pub png_len: usize,
    pub z_index: i32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_width: f32,
    pub dest_height: f32,
    pub src_x: f32,
    pub src_y: f32,
    pub src_width: f32,
    pub src_height: f32,
}

pub struct Hyperlink {
    pub id: CString,
    pub uri: CString,
}

/// How to refresh a snapshot.
pub(crate) struct Update {
    /// Copy every row instead of only damaged ones.
    pub rebuild: bool,
    /// Report every row as damaged.
    pub full_damage: bool,
    /// View of the last snapshot shown to the host, which damage is relative to. Without it,
    /// every row is reported as damaged.
    pub previous: Option<View>,
}

/// Viewport state of a snapshot, deciding what terminal damage can't describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct View {
    columns: usize,
    rows: usize,
    display_offset: usize,
    selection: Option<SelectionRange>,
    cursor_row: u32,
}

#[derive(Default)]
pub struct Snapshot {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub zerowidth: Vec<u32>,
    pub hyperlinks: Vec<Hyperlink>,
    pub cursor: Cursor,
    pub display_offset: usize,
    pub history_size: usize,
    pub mode: u32,
    pub foreground: u32,
    pub background: u32,
    pub full_damage: bool,
    pub damage: Vec<LineDamage>,
    pub images: Vec<Image>,
    pub animation_deadline: Option<Instant>,
    /// Identifier of the last update, used by the engine to allow incremental updates.
    pub(crate) id: u64,
    /// Hyperlink indices by id, so unchanged links keep their index across updates.
    hyperlink_ids: HashMap<String, u32>,
    selection: Option<SelectionRange>,
    /// Keeps image pixels referenced by `images` alive.
    pixels: Vec<Arc<GraphicsImage>>,
    png: Vec<Arc<Vec<u8>>>,
}

impl Snapshot {
    /// Refresh the snapshot from the terminal, returning its new view.
    pub(crate) fn update(
        &mut self,
        term: &mut Term<Listener>,
        palette: &Palette,
        options: &Options,
        focused: bool,
        update: Update,
    ) -> View {
        let Update { rebuild, full_damage: force_full_damage, previous } = update;
        let columns = term.columns();
        let rows = term.screen_lines();
        let display_offset = term.grid().display_offset();
        let selection = term.selection.as_ref().and_then(|selection| selection.to_range(term));

        self.damage.clear();
        let cursor = cursor_point(term);
        let view = View { columns, rows, display_offset, selection, cursor_row: cursor.0 };
        // Damage lines are screen lines, which only match viewport rows without scrollback.
        let mut full = force_full_damage
            || display_offset != 0
            || previous.is_none_or(|previous| {
                previous.display_offset != 0
                    || (previous.columns, previous.rows) != (columns, rows)
                    || previous.selection != selection
            });
        match term.damage() {
            TermDamage::Full => full = true,
            TermDamage::Partial(lines) => {
                if !full {
                    self.damage.extend(lines.filter(|line| line.line < rows).map(|line| {
                        LineDamage {
                            row: line.line as u32,
                            left: line.left as u32,
                            right: line.right as u32,
                        }
                    }));
                }
            },
        }
        term.reset_damage();

        self.columns = columns;
        self.rows = rows;
        self.display_offset = display_offset;
        self.selection = selection;
        self.history_size = term.history_size();
        self.mode = term.mode().bits();
        let overrides = term.colors();
        self.foreground = pack(palette.get(overrides, NamedColor::Foreground as usize));
        self.background = pack(palette.get(overrides, NamedColor::Background as usize));

        // Compact tables grown by partial updates.
        let rebuild = rebuild
            || full
            || self.cells.len() != columns * rows
            || self.zerowidth.len() > 2 * self.cells.len() + 1024
            || self.hyperlinks.len() > self.cells.len();

        // Terminal damage misses vi cursor and focus changes, so damage cursor rows explicitly.
        if let Some(previous) = previous.filter(|_| !full) {
            for row in [previous.cursor_row, cursor.0] {
                let row = row as usize;
                if row < rows && !self.damage.iter().any(|damage| damage.row as usize == row) {
                    let right = columns.saturating_sub(1) as u32;
                    self.damage.push(LineDamage { row: row as u32, left: 0, right });
                }
            }
            self.damage.sort_unstable_by_key(|damage| damage.row);
        }

        if rebuild {
            self.cells.clear();
            self.cells.resize(columns * rows, Cell::default());
            self.zerowidth.clear();
            self.hyperlinks.clear();
            self.hyperlink_ids.clear();
            for row in 0..rows {
                self.build_row(term, palette, options, row);
            }
        } else {
            for index in 0..self.damage.len() {
                let row = self.damage[index].row as usize;
                self.build_row(term, palette, options, row);
            }
        }
        self.full_damage = full;

        self.cursor = self.cursor(term, palette, focused, cursor);
        self.update_images(term, options);
        view
    }

    /// Copy one viewport row from the terminal.
    fn build_row(
        &mut self,
        term: &Term<Listener>,
        palette: &Palette,
        options: &Options,
        row: usize,
    ) {
        let columns = self.columns;
        let line = Line(row as i32 - self.display_offset as i32);
        let overrides = term.colors();
        let bold_bright = options.bold_is_bright;
        let grid_row = &term.grid()[line];
        for column in 0..columns {
            let cell = &grid_row[Column(column)];
            let flags = cell.flags;

            let mut fg = palette.foreground(overrides, cell.fg, flags, bold_bright);
            let mut bg = palette.background(overrides, cell.bg);
            let mut extra = 0;
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            } else if cell.bg == Color::Named(NamedColor::Background) {
                extra |= CELL_DEFAULT_BACKGROUND;
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let underline_color = match cell.underline_color() {
                Some(color) => {
                    extra |= CELL_UNDERLINE_COLOR;
                    pack(palette.foreground(overrides, color, flags, bold_bright))
                },
                None => pack(fg),
            };
            let point = Point::new(line, Column(column));
            if self.selection.is_some_and(|selection| selection.contains(point)) {
                extra |= CELL_SELECTED;
            }
            if cell.c == unicode::PLACEHOLDER {
                extra |= CELL_IMAGE_PLACEHOLDER;
            }

            let zerowidth_index = self.zerowidth.len() as u32;
            let combining = cell.zerowidth().unwrap_or_default();
            self.zerowidth.extend(combining.iter().map(|&c| u32::from(c)));

            let hyperlink = match cell.hyperlink() {
                Some(link) => match self.hyperlink_ids.get(link.id()) {
                    Some(&index) => index,
                    None => {
                        self.hyperlinks
                            .push(Hyperlink { id: c_string(link.id()), uri: c_string(link.uri()) });
                        let index = self.hyperlinks.len() as u32;
                        self.hyperlink_ids.insert(link.id().to_owned(), index);
                        index
                    },
                },
                None => 0,
            };

            self.cells[row * columns + column] = Cell {
                codepoint: u32::from(cell.c),
                fg: pack(fg),
                bg: pack(bg),
                underline_color,
                flags: u32::from(flags.bits()) | extra,
                zerowidth_index,
                zerowidth_count: combining.len() as u16,
                reserved: 0,
                hyperlink,
            };
        }
    }

    fn cursor(
        &self,
        term: &Term<Listener>,
        palette: &Palette,
        focused: bool,
        (row, column, shape): (u32, u32, CursorShape),
    ) -> Cursor {
        let cell = self.cell(column as usize, row as usize).copied();
        let mut shape = if cell.is_some() { shape } else { CursorShape::Hidden };
        if !focused && shape != CursorShape::Hidden {
            shape = CursorShape::HollowBlock;
        }
        let color = pack(palette.get(term.colors(), NamedColor::Cursor as usize));
        let cell = cell.unwrap_or_default();
        let text_color = if cell.bg == color { cell.fg } else { cell.bg };
        Cursor {
            column,
            row,
            shape: cursor_shape(shape),
            color,
            text_color,
            blinking: term.cursor_style().blinking,
            wide: cell.flags & u32::from(Flags::WIDE_CHAR.bits()) != 0,
        }
    }

    fn update_images(&mut self, term: &mut Term<Listener>, options: &Options) {
        self.images.clear();
        self.pixels.clear();
        self.png.clear();
        self.animation_deadline = None;
        let cell_size = (options.cell_width.max(1.), options.cell_height.max(1.));
        for placement in &term.graphics_placements() {
            if let Some(deadline) = placement.animation_deadline {
                self.animation_deadline =
                    Some(self.animation_deadline.map_or(deadline, |current| current.min(deadline)));
            }
            let Some((dest, src)) = geometry(placement, cell_size, self.columns, self.rows) else {
                continue;
            };
            let image = &placement.image;
            let (rgba, rgba_len, png_ptr, png_len) = match image.rgba() {
                Some(rgba) => (rgba.as_ptr(), rgba.len(), std::ptr::null(), 0),
                None => {
                    let data = image.png();
                    let pointers = (std::ptr::null(), 0, data.as_ptr(), data.len());
                    self.png.push(data);
                    pointers
                },
            };
            self.pixels.push(image.clone());
            self.images.push(Image {
                image_id: placement.image_id,
                placement_id: placement.placement_id,
                generation: placement.image_generation,
                width: placement.image_width,
                height: placement.image_height,
                rgba,
                rgba_len,
                png: png_ptr,
                png_len,
                z_index: placement.z_index,
                dest_x: dest.x,
                dest_y: dest.y,
                dest_width: dest.width,
                dest_height: dest.height,
                src_x: src.x,
                src_y: src.y,
                src_width: src.width,
                src_height: src.height,
            });
        }
    }

    pub fn cell(&self, column: usize, row: usize) -> Option<&Cell> {
        (column < self.columns).then(|| self.cells.get(row * self.columns + column))?
    }

    /// Text of one cell, including combining characters.
    pub fn cell_text(&self, cell: &Cell) -> String {
        let start = cell.zerowidth_index as usize;
        let combining = &self.zerowidth[start..start + cell.zerowidth_count as usize];
        std::iter::once(cell.codepoint)
            .chain(combining.iter().copied())
            .filter_map(char::from_u32)
            .collect()
    }
}

/// Viewport row, column and shape of the visible cursor, which is the vi cursor in vi mode.
fn cursor_point(term: &Term<Listener>) -> (u32, u32, CursorShape) {
    let display_offset = term.grid().display_offset();
    let vi_mode = term.mode().contains(TermMode::VI);
    let mut point = if vi_mode { term.vi_mode_cursor.point } else { term.grid().cursor.point };
    if term.grid()[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
        point.column -= 1;
    }
    let shape = if !vi_mode && !term.mode().contains(TermMode::SHOW_CURSOR) {
        CursorShape::Hidden
    } else {
        term.cursor_style().shape
    };
    match point_to_viewport(display_offset, point) {
        Some(point) if point.line < term.screen_lines() => {
            (point.line as u32, point.column.0 as u32, shape)
        },
        _ => (u32::MAX, 0, CursorShape::Hidden),
    }
}

fn cursor_shape(shape: CursorShape) -> u32 {
    match shape {
        CursorShape::Block => 0,
        CursorShape::Underline => 1,
        CursorShape::Beam => 2,
        CursorShape::HollowBlock => 3,
        CursorShape::Hidden => 4,
    }
}

pub(crate) fn c_string(value: &str) -> CString {
    CString::new(value.replace('\0', "")).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Rect {
    fn intersection(self, other: Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > x && bottom > y).then_some(Rect { x, y, width: right - x, height: bottom - y })
    }
}

/// Visible destination and its matching source crop, as computed by the desktop renderer.
fn geometry(
    p: &KittyGraphicsRenderPlacement,
    (cw, ch): (f32, f32),
    columns: usize,
    rows: usize,
) -> Option<(Rect, Rect)> {
    let virtual_placement = p.virtual_cell.is_some();
    let layout = graphics_display_layout(
        p.source_width,
        p.source_height,
        p.display_cols,
        p.display_rows,
        (cw, ch),
        (p.x_offset, p.y_offset),
        virtual_placement,
    );
    let cell_x = (p.col as f32 + p.col_offset as f32) * cw;
    let cell_y = p.viewport_row as f32 * ch;
    let (offset_x, offset_y) =
        p.virtual_cell.map_or((0., 0.), |(col, row)| (col as f32 * cw, row as f32 * ch));
    let destination = Rect {
        x: cell_x + p.x_offset as f32 + layout.image_offset.0 - offset_x,
        y: cell_y + p.y_offset as f32 + layout.image_offset.1 - offset_y,
        width: layout.image_size.0,
        height: layout.image_size.1,
    };
    let viewport = Rect { x: 0., y: 0., width: columns as f32 * cw, height: rows as f32 * ch };
    let clip = if virtual_placement {
        Rect { x: cell_x, y: cell_y, width: cw, height: ch }
    } else {
        Rect {
            x: cell_x,
            y: cell_y + p.clip_top_rows as f32 * ch,
            width: p.occupied_cols as f32 * cw,
            height: p.occupied_rows.saturating_sub(p.clip_top_rows + p.clip_bottom_rows) as f32
                * ch,
        }
    };
    let visible = destination.intersection(viewport)?.intersection(clip)?;
    let source = Rect {
        x: p.source_x as f32
            + (visible.x - destination.x) / destination.width * p.source_width as f32,
        y: p.source_y as f32
            + (visible.y - destination.y) / destination.height * p.source_height as f32,
        width: visible.width / destination.width * p.source_width as f32,
        height: visible.height / destination.height * p.source_height as f32,
    };
    Some((visible, source))
}
