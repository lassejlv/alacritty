//! Color themes.
//!
//! Every `themes/*.toml` file in this crate is embedded at build time and can be selected with
//! `general.theme`, using the file name without its extension. Files in the user's
//! `alacritty/themes` config directory are selected the same way and take precedence.

use std::fs;
use std::path::{Path, PathBuf};

use log::error;
use toml::Value;

use crate::config::{LOG_TARGET_CONFIG, serde_utils};

include!(concat!(env!("OUT_DIR"), "/themes.rs"));

/// Find a built-in theme's canonical name.
///
/// Matching ignores case and punctuation, so Ghostty names like `TokyoNight Storm` also resolve
/// to `tokyo-night-storm`.
#[cfg(target_os = "macos")]
pub fn find(name: &str) -> Option<&'static str> {
    builtin(name).map(|(name, _)| *name)
}

/// Merge the theme selected by `general.theme` below the user's configuration.
///
/// A loaded user theme file is added to `config_paths`, so editing it triggers live reload.
pub fn apply(config: Value, config_paths: &mut Vec<PathBuf>) -> Value {
    apply_from(config, user_themes_dir().as_deref(), config_paths)
}

fn apply_from(config: Value, user_dir: Option<&Path>, config_paths: &mut Vec<PathBuf>) -> Value {
    let Some(name) = config.get("general").and_then(|general| general.get("theme")) else {
        return config;
    };

    match name.as_str().map(|name| load(name, user_dir, config_paths)) {
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

/// Load a user or built-in theme by name.
fn load(
    name: &str,
    user_dir: Option<&Path>,
    config_paths: &mut Vec<PathBuf>,
) -> Result<Value, String> {
    let user_themes = user_dir.map(user_themes).unwrap_or_default();
    let key = normalize(name);

    let theme = match user_themes.iter().find(|path| normalize(&stem(path)) == key) {
        Some(path) => {
            config_paths.push(path.clone());
            fs::read_to_string(path)
                .map_err(|err| format!("Unable to read theme {path:?}: {err}"))?
        },
        None => match builtin(name) {
            Some((_, theme)) => theme.to_string(),
            None => {
                let mut available: Vec<_> = user_themes.iter().map(|path| stem(path)).collect();
                available.extend(BUILTIN_THEMES.iter().map(|(name, _)| name.to_string()));
                return Err(format!(
                    "Unknown theme {name:?}; available themes: {}",
                    available.join(", ")
                ));
            },
        },
    };

    toml::from_str(&theme).map_err(|err| format!("Unable to load theme {name:?}: {err}"))
}

fn builtin(name: &str) -> Option<&'static (&'static str, &'static str)> {
    let key = normalize(name);
    BUILTIN_THEMES.iter().find(|(theme, _)| normalize(theme) == key)
}

/// All `*.toml` files in the user theme directory, sorted by path.
fn user_themes(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut themes: Vec<_> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    themes.sort();
    themes
}

#[cfg(not(windows))]
fn user_themes_dir() -> Option<PathBuf> {
    xdg::BaseDirectories::with_prefix("alacritty").get_config_home().map(|dir| dir.join("themes"))
}

#[cfg(windows)]
fn user_themes_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("alacritty").join("themes"))
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
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
        UiConfig::deserialize(apply_from(toml::from_str(config).unwrap(), None, &mut Vec::new()))
            .unwrap()
    }

    #[test]
    fn builtin_themes_only_set_colors() {
        assert!(!BUILTIN_THEMES.is_empty());
        let mut names: Vec<_> = BUILTIN_THEMES.iter().map(|(name, _)| normalize(name)).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BUILTIN_THEMES.len(), "theme names must stay distinct");
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
        let find = |name| builtin(name).map(|(name, _)| *name);
        assert_eq!(find("TokyoNight Storm"), Some("tokyo-night-storm"));
        assert_eq!(find("tokyo_night"), Some("tokyo-night"));
        assert_eq!(find("missing"), None);
    }

    #[test]
    fn unknown_theme_keeps_default_colors() {
        let config = theme_config(r#"general.theme = "missing""#);
        assert_eq!(config.colors, UiConfig::default().colors);
    }

    #[test]
    fn user_theme_overrides_builtin_and_is_watched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Tokyo-Night.toml");
        fs::write(&path, "[colors.primary]\nbackground = \"#010203\"\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "").unwrap();

        let mut paths = Vec::new();
        let config: Value = toml::from_str(r#"general.theme = "tokyo-night""#).unwrap();
        let config = UiConfig::deserialize(apply_from(config, Some(dir.path()), &mut paths));

        assert_eq!(config.unwrap().colors.primary.background, Rgb::new(1, 2, 3));
        assert_eq!(paths, [path]);
    }

    #[test]
    fn missing_user_theme_falls_back_to_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        let config: Value = toml::from_str(r#"general.theme = "tokyo-night""#).unwrap();
        let config = UiConfig::deserialize(apply_from(config, Some(dir.path()), &mut paths));

        assert_eq!(config.unwrap().colors.primary.background, Rgb::new(0x1a, 0x1b, 0x26));
        assert!(paths.is_empty());
    }

    #[test]
    fn unknown_theme_lists_user_themes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mine.toml"), "").unwrap();
        let err = load("missing", Some(dir.path()), &mut Vec::new()).unwrap_err();
        assert!(err.contains("available themes: mine, "), "{err}");
        assert!(err.contains(", tokyo-night"), "{err}");
    }
}
