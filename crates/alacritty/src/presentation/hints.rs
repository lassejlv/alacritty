use std::borrow::Cow;
use std::cmp::Reverse;
use std::rc::Rc;

use winit::keyboard::ModifiersState;

use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Hyperlink;
pub use alacritty_terminal::term::hints::{
    HintPostProcessor, hyperlink_at, regex_match_at, visible_regex_match_iter,
    visible_unique_hyperlinks_iter,
};
use alacritty_terminal::term::search::Match;
use alacritty_terminal::term::{Term, TermMode};

use crate::config::UiConfig;
use crate::config::ui_config::{Hint, HintAction};

/// Percentage of characters in the hints alphabet used for the last character.
const HINT_SPLIT_PERCENTAGE: f32 = 0.5;

/// Keyboard regex hint state.
pub struct HintState {
    /// Hint currently in use.
    hint: Option<Rc<Hint>>,

    /// Alphabet for hint labels.
    alphabet: String,

    /// Visible matches.
    matches: Vec<Match>,

    /// Key label for each visible match.
    labels: Vec<Vec<char>>,

    /// Keys pressed for hint selection.
    keys: Vec<char>,
}

impl HintState {
    /// Initialize an inactive hint state.
    pub fn new<S: Into<String>>(alphabet: S) -> Self {
        Self {
            alphabet: alphabet.into(),
            hint: Default::default(),
            matches: Default::default(),
            labels: Default::default(),
            keys: Default::default(),
        }
    }

    /// Check if a hint selection is in progress.
    pub fn active(&self) -> bool {
        self.hint.is_some()
    }

    /// Start the hint selection process.
    pub fn start(&mut self, hint: Rc<Hint>) {
        self.hint = Some(hint);
    }

    /// Cancel the hint highlighting process.
    fn stop(&mut self) {
        self.matches.clear();
        self.labels.clear();
        self.keys.clear();
        self.hint = None;
    }

    /// Update the visible hint matches and key labels.
    pub fn update_matches<T>(&mut self, term: &Term<T>) {
        let hint = match self.hint.as_mut() {
            Some(hint) => hint,
            None => return,
        };

        // Clear current matches.
        self.matches.clear();

        // Add escape sequence hyperlinks.
        if hint.content.hyperlinks {
            self.matches.extend(visible_unique_hyperlinks_iter(term));
        }

        // Add visible regex matches.
        if let Some(regex) = hint.content.regex.as_ref() {
            regex.with_compiled(|regex| {
                let matches = visible_regex_match_iter(term, regex);

                // Apply post-processing and search for sub-matches if necessary.
                if hint.post_processing {
                    let mut matches = matches.collect::<Vec<_>>();
                    self.matches.extend(matches.drain(..).flat_map(|rm| {
                        HintPostProcessor::new(term, regex, rm).collect::<Vec<_>>()
                    }));
                } else {
                    self.matches.extend(matches);
                }
            });
        }

        // Cancel highlight with no visible matches.
        if self.matches.is_empty() {
            self.stop();
            return;
        }

        // Sort and dedup ranges. Currently overlapped but not exactly same ranges are kept.
        self.matches.sort_by_key(|bounds| (*bounds.start(), Reverse(*bounds.end())));
        self.matches.dedup_by_key(|bounds| *bounds.start());

        let mut generator = HintLabels::new(&self.alphabet, HINT_SPLIT_PERCENTAGE);
        let match_count = self.matches.len();
        let keys_len = self.keys.len();

        // Get the label for each match.
        self.labels.resize(match_count, Vec::new());
        for i in (0..match_count).rev() {
            let mut label = generator.next();
            if label.len() >= keys_len && label[..keys_len] == self.keys[..] {
                self.labels[i] = label.split_off(keys_len);
            } else {
                self.labels[i] = Vec::new();
            }
        }
    }

    /// Handle keyboard input during hint selection.
    pub fn keyboard_input<T>(&mut self, term: &Term<T>, c: char) -> Option<HintMatch> {
        match c {
            // Use backspace to remove the last character pressed.
            '\x08' | '\x1f' => {
                self.keys.pop();
            },
            // Cancel hint highlighting on ESC/Ctrl+c.
            '\x1b' | '\x03' => self.stop(),
            _ => (),
        }

        // Update the visible matches.
        self.update_matches(term);

        let hint = self.hint.as_ref()?;

        // Find the last label starting with the input character.
        let mut labels = self.labels.iter().enumerate().rev();
        let (index, label) = labels.find(|(_, label)| !label.is_empty() && label[0] == c)?;

        // Check if the selected label is fully matched.
        if label.len() == 1 {
            let bounds = self.matches[index].clone();
            let hint = hint.clone();

            // Exit hint mode unless it requires explicit dismissal.
            if hint.persist {
                self.keys.clear();
            } else {
                self.stop();
            }

            // Hyperlinks take precedence over regex matches.
            let hyperlink = term.grid()[*bounds.start()].hyperlink();
            Some(HintMatch { bounds, hyperlink, hint })
        } else {
            // Store character to preserve the selection.
            self.keys.push(c);

            None
        }
    }

    /// Hint key labels.
    pub fn labels(&self) -> &Vec<Vec<char>> {
        &self.labels
    }

    /// Visible hint regex matches.
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    /// Update the alphabet used for hint labels.
    pub fn update_alphabet(&mut self, alphabet: &str) {
        if self.alphabet != alphabet {
            alphabet.clone_into(&mut self.alphabet);
            self.keys.clear();
        }
    }
}

/// Hint match which was selected by the user.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct HintMatch {
    /// Terminal range matching the hint.
    bounds: Match,

    /// OSC 8 hyperlink.
    hyperlink: Option<Hyperlink>,

    /// Hint which triggered this match.
    hint: Rc<Hint>,
}

impl HintMatch {
    #[inline]
    pub fn should_highlight(&self, point: Point, pointed_hyperlink: Option<&Hyperlink>) -> bool {
        self.hyperlink.as_ref() == pointed_hyperlink
            && (self.hyperlink.is_some() || self.bounds.contains(&point))
    }

    #[inline]
    pub fn action(&self) -> &HintAction {
        &self.hint.action
    }

    #[inline]
    pub fn bounds(&self) -> &Match {
        &self.bounds
    }

    pub fn hyperlink(&self) -> Option<&Hyperlink> {
        self.hyperlink.as_ref()
    }

    /// Get the text content of the hint match.
    ///
    /// This will always revalidate the hint text, to account for terminal content
    /// changes since the [`HintMatch`] was constructed. The text of the hint might
    /// be different from its original value, but it will **always** be a valid
    /// match for this hint.
    pub fn text<T>(&self, term: &Term<T>) -> Option<Cow<'_, str>> {
        // Revalidate hyperlink match.
        if let Some(hyperlink) = &self.hyperlink {
            let (validated, bounds) = hyperlink_at(term, *self.bounds.start())?;
            return (&validated == hyperlink && bounds == self.bounds)
                .then(|| hyperlink.uri().into());
        }

        // Revalidate regex match.
        let regex = self.hint.content.regex.as_ref()?;
        let bounds = regex.with_compiled(|regex| {
            regex_match_at(term, *self.bounds.start(), regex, self.hint.post_processing)
        })??;
        (bounds == self.bounds)
            .then(|| term.bounds_to_string(*bounds.start(), *bounds.end()).into())
    }
}

/// Generator for creating new hint labels.
struct HintLabels {
    /// Full character set available.
    alphabet: Vec<char>,

    /// Alphabet indices for the next label.
    indices: Vec<usize>,

    /// Point separating the alphabet's head and tail characters.
    ///
    /// To make identification of the tail character easy, part of the alphabet cannot be used for
    /// any other position.
    ///
    /// All characters in the alphabet before this index will be used for the last character, while
    /// the rest will be used for everything else.
    split_point: usize,
}

impl HintLabels {
    /// Create a new label generator.
    ///
    /// The `split_ratio` should be a number between 0.0 and 1.0 representing the percentage of
    /// elements in the alphabet which are reserved for the tail of the hint label.
    fn new(alphabet: impl Into<String>, split_ratio: f32) -> Self {
        let alphabet: Vec<char> = alphabet.into().chars().collect();
        let split_point = ((alphabet.len() - 1) as f32 * split_ratio.min(1.)) as usize;

        Self { indices: vec![0], split_point, alphabet }
    }

    /// Get the characters for the next label.
    fn next(&mut self) -> Vec<char> {
        let characters = self.indices.iter().rev().map(|index| self.alphabet[*index]).collect();
        self.increment();
        characters
    }

    /// Increment the character sequence.
    fn increment(&mut self) {
        // Increment the last character; if it's not at the split point we're done.
        let tail = &mut self.indices[0];
        if *tail < self.split_point {
            *tail += 1;
            return;
        }
        *tail = 0;

        // Increment all other characters in reverse order.
        let alphabet_len = self.alphabet.len();
        for index in self.indices.iter_mut().skip(1) {
            if *index + 1 == alphabet_len {
                // Reset character and move to the next if it's already at the limit.
                *index = self.split_point + 1;
            } else {
                // If the character can be incremented, we're done.
                *index += 1;
                return;
            }
        }

        // Extend the sequence with another character when nothing could be incremented.
        self.indices.push(self.split_point + 1);
    }
}

/// Check if there is a hint highlighted at the specified point.
pub fn highlighted_at<T>(
    term: &Term<T>,
    config: &UiConfig,
    point: Point,
    mouse_mods: ModifiersState,
) -> Option<HintMatch> {
    let mouse_mode = term.mode().intersects(TermMode::MOUSE_MODE);

    config.hints.enabled.iter().find_map(|hint| {
        // Check if all required modifiers are pressed.
        let highlight = hint.mouse.is_some_and(|mouse| {
            mouse.enabled
                && mouse_mods.contains(mouse.mods.0)
                && (!mouse_mode || mouse_mods.contains(ModifiersState::SHIFT))
        });
        if !highlight {
            return None;
        }

        if let Some((hyperlink, bounds)) =
            hint.content.hyperlinks.then(|| hyperlink_at(term, point)).flatten()
        {
            return Some(HintMatch { bounds, hyperlink: Some(hyperlink), hint: hint.clone() });
        }

        let bounds = hint.content.regex.as_ref().and_then(|regex| {
            regex.with_compiled(|regex| regex_match_at(term, point, regex, hint.post_processing))
        });
        if let Some(bounds) = bounds.flatten() {
            return Some(HintMatch { bounds, hint: hint.clone(), hyperlink: None });
        }

        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_label_generation() {
        let mut generator = HintLabels::new("0123", 0.5);

        assert_eq!(generator.next(), vec!['0']);
        assert_eq!(generator.next(), vec!['1']);

        assert_eq!(generator.next(), vec!['2', '0']);
        assert_eq!(generator.next(), vec!['2', '1']);
        assert_eq!(generator.next(), vec!['3', '0']);
        assert_eq!(generator.next(), vec!['3', '1']);

        assert_eq!(generator.next(), vec!['2', '2', '0']);
        assert_eq!(generator.next(), vec!['2', '2', '1']);
        assert_eq!(generator.next(), vec!['2', '3', '0']);
        assert_eq!(generator.next(), vec!['2', '3', '1']);
        assert_eq!(generator.next(), vec!['3', '2', '0']);
        assert_eq!(generator.next(), vec!['3', '2', '1']);
        assert_eq!(generator.next(), vec!['3', '3', '0']);
        assert_eq!(generator.next(), vec!['3', '3', '1']);

        assert_eq!(generator.next(), vec!['2', '2', '2', '0']);
        assert_eq!(generator.next(), vec!['2', '2', '2', '1']);
        assert_eq!(generator.next(), vec!['2', '2', '3', '0']);
        assert_eq!(generator.next(), vec!['2', '2', '3', '1']);
        assert_eq!(generator.next(), vec!['2', '3', '2', '0']);
        assert_eq!(generator.next(), vec!['2', '3', '2', '1']);
        assert_eq!(generator.next(), vec!['2', '3', '3', '0']);
        assert_eq!(generator.next(), vec!['2', '3', '3', '1']);
        assert_eq!(generator.next(), vec!['3', '2', '2', '0']);
        assert_eq!(generator.next(), vec!['3', '2', '2', '1']);
        assert_eq!(generator.next(), vec!['3', '2', '3', '0']);
        assert_eq!(generator.next(), vec!['3', '2', '3', '1']);
        assert_eq!(generator.next(), vec!['3', '3', '2', '0']);
        assert_eq!(generator.next(), vec!['3', '3', '2', '1']);
        assert_eq!(generator.next(), vec!['3', '3', '3', '0']);
        assert_eq!(generator.next(), vec!['3', '3', '3', '1']);
    }
}
