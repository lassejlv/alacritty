//! Kitty graphics integration with the terminal grid and its ordered mutations.

use std::time::Instant;

use super::*;
use crate::graphics::{
    GraphicsSize, KittyGraphicsCommand, KittyGraphicsPlaceholder, KittyGraphicsRenderPlacement,
    KittyGraphicsScreen, unicode,
};

impl<T> Term<T> {
    pub fn set_graphics_cell_size(&mut self, width: f32, height: f32) {
        self.graphics_cell_size = (width.max(1.), height.max(1.));
        self.graphics.resize(self.graphics_size());
    }

    pub(super) fn graphics_size(&self) -> GraphicsSize {
        GraphicsSize {
            cols: self.columns().min(u16::MAX as usize) as u16,
            rows: self.screen_lines().min(u16::MAX as usize) as u16,
            cell_width: self.graphics_cell_size.0,
            cell_height: self.graphics_cell_size.1,
        }
    }

    pub(super) fn graphics_screen(&self) -> KittyGraphicsScreen {
        KittyGraphicsScreen::from_alternate_screen(self.mode.contains(TermMode::ALT_SCREEN))
    }

    fn graphics_placeholders(&self) -> Vec<KittyGraphicsPlaceholder> {
        let mut placeholders = Vec::new();
        if !self.graphics.has_virtual_placements_on_screen(self.graphics_screen()) {
            return placeholders;
        }
        let offset = self.grid.display_offset() as i64;
        let mut previous = None;
        for indexed in self.grid.display_iter() {
            let cell = indexed.cell;
            if cell.c != unicode::PLACEHOLDER {
                previous = None;
                continue;
            }
            let mut diacritics = [None; 3];
            for (slot, character) in diacritics.iter_mut().zip(cell.zerowidth().unwrap_or_default())
            {
                *slot = unicode::diacritic_index(*character);
            }
            let placeholder = KittyGraphicsPlaceholder::from_cell(
                i64::from(indexed.point.line.0) + offset,
                indexed.point.column.0,
                color_id(cell.fg),
                cell.underline_color().map_or(0, color_id),
                diacritics,
                previous,
            );
            placeholders.push(placeholder);
            previous = Some(placeholder);
        }
        placeholders
    }

    /// Immutable shared image pixels can be uploaded after releasing the terminal lock.
    pub fn graphics_placements(&mut self) -> Vec<KittyGraphicsRenderPlacement> {
        if self.graphics.advance_animations(Instant::now()) {
            self.mark_fully_damaged();
        }
        let placeholders = self.graphics_placeholders();
        self.graphics.render_placements_on_screen_with_placeholders(
            self.history_size(),
            self.grid.display_offset(),
            self.screen_lines(),
            self.columns(),
            self.graphics_screen(),
            &placeholders,
        )
    }

    pub(super) fn graphics_scrolled_up(
        &mut self,
        origin: Line,
        lines: usize,
        history_before: usize,
    ) {
        if !self.graphics.needs_grid_effects() {
            return;
        }
        let screen = self.graphics_screen();
        if origin == 0 && screen == KittyGraphicsScreen::Primary {
            if self.scroll_region.end.0 as usize == self.screen_lines() {
                let evicted =
                    lines.saturating_sub(self.history_size().saturating_sub(history_before));
                self.graphics.scroll_up_without_history_on_screen(evicted, screen);
            } else {
                self.graphics.scroll_partial_history_region(
                    self.scroll_region.end.0 as usize,
                    lines,
                    history_before,
                    self.history_size(),
                );
            }
        } else {
            self.graphics.scroll_region_on_screen(
                screen,
                origin.0 as usize,
                self.scroll_region.end.0 as usize,
                lines as i64,
                self.history_size(),
            );
        }
    }
}

impl<T: EventListener> Term<T> {
    pub(super) fn apply_graphics(&mut self, mut data: Vec<u8>, truncated: bool) {
        if data.first() != Some(&b'G') {
            return;
        }
        data.remove(0);
        let command = KittyGraphicsCommand::parse(data, truncated);
        let placeholders = if command.needs_placeholder_positions() {
            self.graphics_placeholders()
        } else {
            Vec::new()
        };
        let screen = self.graphics_screen();
        let result = self.graphics.apply_on_screen_with_placeholders(
            command,
            (self.grid.cursor.point.column.0, self.grid.cursor.point.line.0 as usize),
            self.history_size(),
            self.graphics_size(),
            screen,
            &placeholders,
        );
        if let Some(reply) = result.response {
            // Protocol responses are printable ASCII wrapped in APC delimiters.
            self.event_proxy
                .send_event(Event::PtyWrite(String::from_utf8_lossy(&reply).into_owned()));
        }
        if result.changed {
            self.mark_fully_damaged();
        }
        if let Some((cols, rows)) = result.cursor_advance {
            if result.cursor_advance_screen.is_none_or(|target| target == screen) {
                self.move_forward(cols.min(self.columns() as u32) as usize);
                if self.scroll_region.start == 0
                    && self.scroll_region.end.0 as usize == self.screen_lines()
                {
                    for _ in 0..rows.min(self.screen_lines() as u32) {
                        self.linefeed();
                    }
                } else {
                    self.move_down(rows.min(self.screen_lines() as u32) as usize);
                }
            }
        }
    }
}

fn color_id(color: Color) -> u32 {
    match color {
        Color::Spec(rgb) => (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b),
        Color::Indexed(index) => u32::from(index),
        Color::Named(_) => 0,
    }
}
