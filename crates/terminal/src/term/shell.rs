use super::{Term, TermMode};
use crate::event::EventListener;
use crate::grid::{Dimensions, Scroll};
use crate::index::{Column, Direction, Line, Point};
use crate::protocols::shell::{Anchor, Command, Markers, Phase, ShellState};
use crate::vte::ansi::Handler;

impl<T> Term<T> {
    fn shell_command_text(&self, start: Point, end: Point) -> String {
        let mut text = String::new();
        for line in start.line.0..=end.line.0 {
            let line = Line(line);
            let start_col = if line == start.line { start.column } else { Column(0) };
            let end_col = if line == end.line { end.column } else { self.last_column() };
            for character in self.line_to_string(line, start_col..end_col, line == end.line).chars()
            {
                if text.len() + character.len_utf8() > 8192 {
                    return text;
                }
                text.push(character);
            }
        }
        text
    }

    pub fn shell_state(&self) -> &ShellState {
        &self.shell
    }

    fn shell_anchor(&self, id: u64, field: impl Fn(Markers) -> Option<Anchor>) -> Option<Point> {
        for line in (self.topmost_line().0..=self.bottommost_line().0).rev() {
            for column in (0..self.columns()).rev() {
                let point = Point::new(Line(line), Column(column));
                let Some(markers) = self.grid[point].shell_markers() else { continue };
                if let Some(anchor) = field(markers).filter(|anchor| anchor.id == id) {
                    return Some(Point::new(point.line, point.column + usize::from(anchor.after)));
                }
            }
        }
        None
    }

    fn shell_range(&self, mut start: Point, end: Point) -> Option<(Point, Point)> {
        if start >= end {
            return None;
        }
        if start.column.0 == self.columns() {
            start = Point::new(start.line + 1, Column(0));
        }
        let end = if end.column.0 == 0 {
            Point::new(end.line - 1, self.last_column())
        } else {
            Point::new(end.line, end.column - 1)
        };
        (start <= end).then_some((start, end))
    }

    /// Bounds of the last completed command's output, excluding its prompt.
    pub fn last_command_output_range(&self) -> Option<(Point, Point)> {
        if self.mode.contains(TermMode::ALT_SCREEN) {
            return None;
        }
        let id = self.shell.last_command.as_ref()?.id;
        let start = self.shell_anchor(id, |marks| marks.output)?;
        let end = self.shell_anchor(id, |marks| marks.end)?;
        self.shell_range(start, end)
    }

    pub fn last_command_output(&self) -> Option<String> {
        let (start, end) = self.last_command_output_range()?;
        Some(self.bounds_to_string(start, end))
    }
}

impl<T: EventListener> Term<T> {
    /// Scroll to an adjacent prompt without requiring terminal vi mode.
    pub fn jump_to_prompt(&mut self, direction: Direction) {
        if self.mode.contains(TermMode::ALT_SCREEN) {
            return;
        }
        let top = -(self.grid.display_offset() as i32);
        let mut prompts = Vec::new();
        for line in self.topmost_line().0..=self.bottommost_line().0 {
            for column in 0..self.columns() {
                let point = Point::new(Line(line), Column(column));
                if let Some(anchor) = self.grid[point].shell_markers().and_then(|m| m.prompt) {
                    prompts.push((anchor.id, point));
                }
            }
        }
        let origin = self.shell.navigation.or_else(|| {
            if self.grid.display_offset() == 0 {
                Some(self.shell.id)
            } else {
                prompts.iter().find(|(_, point)| point.line.0 >= top).map(|(id, _)| *id)
            }
        });
        let target = match direction {
            Direction::Left => prompts.iter().rev().find(|(id, _)| origin.is_none_or(|n| *id < n)),
            Direction::Right => prompts.iter().find(|(id, _)| origin.is_none_or(|n| *id > n)),
        };
        if let Some(&(id, point)) = target {
            self.scroll_display(Scroll::Delta(-point.line.0 - self.grid.display_offset() as i32));
            self.shell.navigation = Some(id);
        } else if direction == Direction::Right {
            self.scroll_display(Scroll::Bottom);
        }
    }

    fn mark_shell(&mut self, field: impl FnOnce(&mut Markers, Anchor)) {
        let anchor = Anchor { id: self.shell.id, after: self.grid.cursor.input_needs_wrap };
        let cell = self.grid.cursor_cell();
        let mut markers = cell.shell_markers().unwrap_or_default();
        field(&mut markers, anchor);
        cell.set_shell_markers(markers);
    }

    fn finish_shell_command(&mut self, exit_status: Option<u8>) {
        self.mark_shell(|markers, anchor| markers.end = Some(anchor));
        self.shell.last_command = Some(Command {
            id: self.shell.id,
            text: std::mem::take(&mut self.shell.command),
            exit_status,
        });
        self.shell.phase = Phase::Unknown;
    }

    pub(super) fn apply_shell_marker(&mut self, body: &[u8]) {
        if self.mode.contains(TermMode::ALT_SCREEN) || body.len() > 8192 {
            return;
        }
        let Ok(body) = std::str::from_utf8(body) else { return };
        if body.chars().any(char::is_control) {
            return;
        }
        let mut fields = body.split(';');
        match fields.next() {
            Some("A") => {
                // Secondary prompts belong to the current command.
                if fields.any(|field| field == "k=s") {
                    return;
                }
                if self.shell.phase == Phase::Output {
                    self.finish_shell_command(None);
                }
                self.shell_prompt();
                self.shell.id = self.shell.id.wrapping_add(1);
                self.shell.phase = Phase::Prompt;
                self.shell.navigation = None;
                self.shell.command.clear();
                self.mark_shell(|markers, anchor| markers.prompt = Some(anchor));
            },
            Some("B") if self.shell.phase == Phase::Prompt => {
                self.mark_shell(|markers, anchor| markers.input = Some(anchor));
                self.shell.phase = Phase::Input;
            },
            Some("C") if matches!(self.shell.phase, Phase::Prompt | Phase::Input) => {
                self.mark_shell(|markers, anchor| markers.output = Some(anchor));
                let text = self
                    .shell_anchor(self.shell.id, |m| m.input)
                    .zip(self.shell_anchor(self.shell.id, |m| m.output))
                    .and_then(|(start, end)| self.shell_range(start, end))
                    .map(|(start, end)| self.shell_command_text(start, end))
                    .unwrap_or_default();
                self.shell.command = text.trim().chars().take(8192).collect();
                self.shell.phase = Phase::Output;
            },
            Some("D") => {
                let status = match fields.next() {
                    None | Some("") => None,
                    Some(value) => {
                        let Ok(status) = value.parse::<u8>() else { return };
                        Some(status)
                    },
                };
                if self.shell.phase == Phase::Output {
                    self.finish_shell_command(status);
                } else if self.shell.phase == Phase::Input {
                    let start = self
                        .shell_anchor(self.shell.id, |m| m.prompt)
                        .or_else(|| self.shell_anchor(self.shell.id, |m| m.input));
                    if let Some(start) = start {
                        for line in start.line.0..=self.bottommost_line().0 {
                            for column in 0..self.columns() {
                                self.grid[Point::new(Line(line), Column(column))]
                                    .clear_shell_markers(self.shell.id);
                            }
                        }
                    }
                    self.shell.phase = Phase::Unknown;
                    self.shell.command.clear();
                }
            },
            _ => (),
        }
    }
}
