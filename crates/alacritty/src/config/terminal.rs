use serde::{Deserialize, Deserializer, Serialize, de};
use toml::Value;

use alacritty_config_derive::{ConfigDeserialize, SerdeReplace};
use alacritty_terminal::term::Osc52;

use crate::config::ui_config::{Program, StringVisitor};

#[derive(ConfigDeserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Terminal {
    /// OSC 52 and Kitty OSC 5522 clipboard access policy.
    pub osc52: SerdeOsc52,
    /// Path to a shell program to run on startup.
    pub shell: Option<Program>,
    /// Automatically mark prompts and report directories for supported shells.
    pub shell_integration: bool,
    /// Allow OSC 99 native desktop notifications.
    pub desktop_notifications: bool,
}

impl Default for Terminal {
    fn default() -> Self {
        Self {
            osc52: Default::default(),
            shell: None,
            shell_integration: true,
            desktop_notifications: true,
        }
    }
}

#[derive(SerdeReplace, Serialize, Default, Copy, Clone, Debug, PartialEq)]
pub struct SerdeOsc52(pub Osc52);

impl<'de> Deserialize<'de> for SerdeOsc52 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = deserializer.deserialize_str(StringVisitor)?;
        Osc52::deserialize(Value::String(value)).map(SerdeOsc52).map_err(de::Error::custom)
    }
}
