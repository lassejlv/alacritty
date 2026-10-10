//! Detection of URLs and hyperlinks in visible terminal content.

use std::collections::HashSet;
use std::iter;

use crate::grid::{BidirectionalIterator, Dimensions};
use crate::index::{Boundary, Column, Direction, Line, Point};
use crate::term::Term;
use crate::term::cell::Hyperlink;
use crate::term::search::{Match, RegexIter, RegexSearch};

/// Regex matching URLs, used by the default URL hint.
#[rustfmt::skip]
pub const URL_REGEX: &str = "(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file:|git://|ssh:|ftp://)\
                             [^\u{0000}-\u{001F}\u{007F}-\u{009F}<>\"\\s{-}\\^⟨⟩`\\\\]+";

/// Maximum number of linewraps followed outside of the viewport during search highlighting.
pub const MAX_SEARCH_LINES: usize = 100;

/// Iterate over all visible regex matches.
pub fn visible_regex_match_iter<'a, T>(
    term: &'a Term<T>,
    regex: &'a mut RegexSearch,
) -> impl Iterator<Item = Match> + 'a {
    let viewport_start = Line(-(term.grid().display_offset() as i32));
    let viewport_end = viewport_start + term.bottommost_line();
    let mut start = term.line_search_left(Point::new(viewport_start, Column(0)));
    let mut end = term.line_search_right(Point::new(viewport_end, Column(0)));
    start.line = start.line.max(viewport_start - MAX_SEARCH_LINES);
    end.line = end.line.min(viewport_end + MAX_SEARCH_LINES);

    RegexIter::new(start, end, Direction::Right, term, regex)
        .skip_while(move |rm| rm.end().line < viewport_start)
        .take_while(move |rm| rm.start().line <= viewport_end)
}

/// Iterate over all visible hyperlinks, yanking only unique ones.
pub fn visible_unique_hyperlinks_iter<T>(term: &Term<T>) -> impl Iterator<Item = Match> + '_ {
    let mut display_iter = term.grid().display_iter().peekable();

    // Avoid creating hints for the same hyperlinks, but from a different places.
    let mut unique_hyperlinks = HashSet::<Hyperlink>::new();

    iter::from_fn(move || {
        // Find the start of the next unique hyperlink.
        let (cell, hyperlink) = display_iter.find_map(|cell| {
            let hyperlink = cell.hyperlink()?;
            (!unique_hyperlinks.contains(&hyperlink)).then(|| {
                unique_hyperlinks.insert(hyperlink.clone());
                (cell, hyperlink)
            })
        })?;

        let start = cell.point;
        let mut end = start;

        // Find the end bound of just found unique hyperlink.
        while let Some(next_cell) = display_iter.peek() {
            // Cell at display iter doesn't match, yield the hyperlink and start over with
            // `find_map`.
            if next_cell.hyperlink().as_ref() != Some(&hyperlink) {
                break;
            }

            // Advance to the next cell.
            end = next_cell.point;
            let _ = display_iter.next();
        }

        Some(start..=end)
    })
}

/// Retrieve the match, if the specified point is inside the content matching the regex.
pub fn regex_match_at<T>(
    term: &Term<T>,
    point: Point,
    regex: &mut RegexSearch,
    post_processing: bool,
) -> Option<Match> {
    let regex_match = visible_regex_match_iter(term, regex).find(|rm| rm.contains(&point))?;

    // Apply post-processing and search for sub-matches if necessary.
    if post_processing {
        HintPostProcessor::new(term, regex, regex_match).find(|rm| rm.contains(&point))
    } else {
        Some(regex_match)
    }
}

/// Retrieve the hyperlink with its range, if there is one at the specified point.
///
/// This will only return contiguous cells, even if another hyperlink with the same ID exists.
pub fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<(Hyperlink, Match)> {
    let hyperlink = term.grid()[point].hyperlink()?;

    let grid = term.grid();

    let mut match_end = point;
    for cell in grid.iter_from(point) {
        if cell.hyperlink().is_some_and(|link| link == hyperlink) {
            match_end = cell.point;
        } else {
            break;
        }
    }

    let mut match_start = point;
    let mut iter = grid.iter_from(point);
    while let Some(cell) = iter.prev() {
        if cell.hyperlink().is_some_and(|link| link == hyperlink) {
            match_start = cell.point;
        } else {
            break;
        }
    }

    Some((hyperlink, match_start..=match_end))
}

/// Iterator over all post-processed matches inside an existing hint match.
pub struct HintPostProcessor<'a, T> {
    /// Regex search DFAs.
    regex: &'a mut RegexSearch,

    /// Terminal reference.
    term: &'a Term<T>,

    /// Next hint match in the iterator.
    next_match: Option<Match>,

    /// Start point for the next search.
    start: Point,

    /// End point for the hint match iterator.
    end: Point,
}

impl<'a, T> HintPostProcessor<'a, T> {
    /// Create a new iterator for an unprocessed match.
    pub fn new(term: &'a Term<T>, regex: &'a mut RegexSearch, regex_match: Match) -> Self {
        let mut post_processor = Self {
            next_match: None,
            start: *regex_match.start(),
            end: *regex_match.end(),
            term,
            regex,
        };

        // Post-process the first hint match.
        post_processor.next_processed_match(regex_match);

        post_processor
    }

    /// Apply some hint post processing heuristics.
    ///
    /// This will check the end of the hint and make it shorter if certain characters are determined
    /// to be unlikely to be intentionally part of the hint.
    ///
    /// This is most useful for identifying URLs appropriately.
    fn hint_post_processing(&self, regex_match: &Match) -> Option<Match> {
        let mut iter = self.term.grid().iter_from(*regex_match.start());

        let mut c = iter.cell().c;

        // Truncate uneven number of brackets.
        let end = *regex_match.end();
        let mut open_parents = 0;
        let mut open_brackets = 0;
        loop {
            match c {
                '(' => open_parents += 1,
                '[' => open_brackets += 1,
                ')' => {
                    if open_parents == 0 {
                        iter.prev();
                        break;
                    } else {
                        open_parents -= 1;
                    }
                },
                ']' => {
                    if open_brackets == 0 {
                        iter.prev();
                        break;
                    } else {
                        open_brackets -= 1;
                    }
                },
                _ => (),
            }

            if iter.point() == end {
                break;
            }

            match iter.next() {
                Some(indexed) => c = indexed.cell.c,
                None => break,
            }
        }

        // Truncate trailing characters which are likely to be delimiters.
        let start = *regex_match.start();
        while iter.point() != start {
            if !matches!(c, '.' | ',' | ':' | ';' | '?' | '!' | '(' | '[' | '\'') {
                break;
            }

            match iter.prev() {
                Some(indexed) => c = indexed.cell.c,
                None => break,
            }
        }

        if start > iter.point() { None } else { Some(start..=iter.point()) }
    }

    /// Loop over submatches until a non-empty post-processed match is found.
    fn next_processed_match(&mut self, mut regex_match: Match) {
        self.next_match = loop {
            if let Some(next_match) = self.hint_post_processing(&regex_match) {
                self.start = next_match.end().add(self.term, Boundary::Grid, 1);
                break Some(next_match);
            }

            self.start = regex_match.start().add(self.term, Boundary::Grid, 1);
            if self.start > self.end {
                return;
            }

            match self.term.regex_search_right(self.regex, self.start, self.end) {
                Some(rm) => regex_match = rm,
                None => return,
            }
        };
    }
}

impl<T> Iterator for HintPostProcessor<'_, T> {
    type Item = Match;

    fn next(&mut self) -> Option<Self::Item> {
        let next_match = self.next_match.take()?;

        if self.start <= self.end
            && let Some(rm) = self.term.regex_search_right(self.regex, self.start, self.end)
        {
            self.next_processed_match(rm);
        }

        Some(next_match)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::test::mock_term;
    use crate::vte::ansi::Handler;

    #[test]
    fn closed_bracket_does_not_result_in_infinite_iterator() {
        let term = mock_term(" ) ");

        let mut search = RegexSearch::new("[^/ ]").unwrap();

        let count = HintPostProcessor::new(
            &term,
            &mut search,
            Point::new(Line(0), Column(1))..=Point::new(Line(0), Column(1)),
        )
        .take(1)
        .count();

        assert_eq!(count, 0);
    }

    #[test]
    fn collect_unique_hyperlinks() {
        let mut term = mock_term("000\r\n111");
        term.goto(0, 0);

        let hyperlink_foo = Hyperlink::new(Some("1"), String::from("foo"));
        let hyperlink_bar = Hyperlink::new(Some("2"), String::from("bar"));

        // Create 2 hyperlinks on the first line.
        term.set_hyperlink(Some(hyperlink_foo.clone().into()));
        term.input('b');
        term.input('a');
        term.set_hyperlink(Some(hyperlink_bar.clone().into()));
        term.input('r');
        term.set_hyperlink(Some(hyperlink_foo.clone().into()));
        term.goto(1, 0);

        // Ditto for the second line.
        term.set_hyperlink(Some(hyperlink_foo.into()));
        term.input('b');
        term.input('a');
        term.set_hyperlink(Some(hyperlink_bar.into()));
        term.input('r');
        term.set_hyperlink(None);

        let mut unique_hyperlinks = visible_unique_hyperlinks_iter(&term);
        assert_eq!(
            Some(Match::new(Point::new(Line(0), Column(0)), Point::new(Line(0), Column(1)))),
            unique_hyperlinks.next()
        );
        assert_eq!(
            Some(Match::new(Point::new(Line(0), Column(2)), Point::new(Line(0), Column(2)))),
            unique_hyperlinks.next()
        );
        assert_eq!(None, unique_hyperlinks.next());
    }

    #[test]
    fn visible_regex_match_covers_entire_viewport() {
        let content = "I'm a match!\r\n".repeat(4096);
        // The Term returned from this call will have a viewport starting at 0 and ending at 4096.
        // That's good enough for this test, since it only cares about visible content.
        let term = mock_term(&content);
        let mut regex = RegexSearch::new("match!").unwrap();

        // The iterator should match everything in the viewport.
        assert_eq!(visible_regex_match_iter(&term, &mut regex).count(), 4096);
    }
}
