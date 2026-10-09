//! Semantic shell markers carried with grid cells through scrollback and reflow.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Anchor {
    pub id: u64,
    pub after: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Markers {
    pub prompt: Option<Anchor>,
    pub input: Option<Anchor>,
    pub output: Option<Anchor>,
    pub end: Option<Anchor>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: u64,
    pub text: String,
    pub exit_status: Option<u8>,
}

#[derive(Default, Debug)]
pub struct ShellState {
    pub(crate) id: u64,
    pub(crate) phase: Phase,
    pub(crate) command: String,
    pub(crate) navigation: Option<u64>,
    pub(crate) last_command: Option<Command>,
}

impl ShellState {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn last_command(&self) -> Option<&Command> {
        self.last_command.as_ref()
    }
}
