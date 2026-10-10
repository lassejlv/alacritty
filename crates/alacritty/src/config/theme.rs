//! Built-in color themes.
//!
//! Every `themes/*.toml` file in this crate is embedded at build time and can be selected with
//! `general.theme`, using the file name without its extension.

use log::error;
use toml::Value;

use crate::config::{LOG_TARGET_CONFIG, serde_utils};

include!(concat!(env!("OUT_DIR"), "/themes.rs"));

/// Find a built-in theme's canonical name.
///
/// Matching ignores case and punctuation, so Ghostty names like `TokyoNight Storm` also resolve
/// to `tokyo-night-storm`.
pub fn find(name: &str) -> Option<&'static str> {
    builtin(name).map(|(name, _)| *name)
}

/// Merge the theme selected by `general.theme` below the user's configuration.
pub fn apply(config: Value) -> Value {
    let Some(name) = config.get("general").and_then(|general| general.get("theme")) else {
        return config;
    };

    match name.as_str().map(load) {
        Some(Ok(theme)) => serde_utils::merge(theme, config),
        Some(Err(err)) => {
            error!(target: LOG_TARGET_CONFIG, "{err}");
            config
        },
        None => {
            error!(target: LOG_TARGET_CONFIG, "Invalid general.theme type: expected a string");
            config
        },
    }
}

/// Load a built-in theme by name.
fn load(name: &str) -> Result<Value, String> {
    let Some((_, theme)) = builtin(name) else {
        let available: Vec<_> = BUILTIN_THEMES.iter().map(|(name, _)| *name).collect();
        return Err(format!("Unknown theme {name:?}; available themes: {}", available.join(", ")));
    };

    toml::from_str(theme).map_err(|err| format!("Unable to load theme {name:?}: {err}"))
}

fn builtin(name: &str) -> Option<&'static (&'static str, &'static str)> {
    let key = normalize(name);
    BUILTIN_THEMES.iter().find(|(theme, _)| normalize(theme) == key)
}

fn normalize(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::config::UiConfig;
    use crate::presentation::color::Rgb;

    fn theme_config(config: &str) -> UiConfig {
        UiConfig::deserialize(apply(toml::from_str(config).unwrap())).unwrap()
    }

    #[test]
    fn builtin_themes_only_set_colors() {
        assert!(!BUILTIN_THEMES.is_empty());
        for (name, theme) in BUILTIN_THEMES {
            let theme: Value = toml::from_str(theme).unwrap();
            let keys: Vec<_> = theme.as_table().unwrap().keys().collect();
            assert_eq!(keys, ["colors"], "{name} must only set colors");

            let config = UiConfig::deserialize(theme).unwrap();
            assert_ne!(config.colors, UiConfig::default().colors, "{name} sets no known colors");
        }
    }

    #[test]
    fn theme_applies_below_user_colors() {
        let config = theme_config(
            r##"
            general.theme = "tokyo-night"
            colors.primary.foreground = "#ffffff"
            "##,
        );
        assert_eq!(config.colors.primary.background, Rgb::new(0x1a, 0x1b, 0x26));
        assert_eq!(config.colors.primary.foreground, Rgb::new(0xff, 0xff, 0xff));
        assert_eq!(config.general.theme.as_deref(), Some("tokyo-night"));
    }

    #[test]
    fn theme_names_ignore_case_and_punctuation() {
        assert_eq!(find("TokyoNight Storm"), Some("tokyo-night-storm"));
        assert_eq!(find("tokyo_night"), Some("tokyo-night"));
        assert_eq!(find("missing"), None);
    }

    #[test]
    fn unknown_theme_keeps_default_colors() {
        let config = theme_config(r#"general.theme = "missing""#);
        assert_eq!(config.colors, UiConfig::default().colors);
    }
}
