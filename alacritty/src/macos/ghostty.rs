//! Conservative Ghostty-to-Alacritty conversion. Source commands are never executed.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use toml::{Table, Value};
use toml_edit::{DocumentMut, Item, TableLike};

const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;
const MAX_FILES: usize = 64;
const ANSI: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Entry {
    key: String,
    value: String,
    location: String,
    quoted: bool,
}

pub struct Migration {
    pub settings: Table,
    pub notes: Vec<String>,
    pub mapped: usize,
}

pub struct Prepared {
    pub path: PathBuf,
    pub text: String,
    original: Option<Vec<u8>>,
}

impl Prepared {
    /// Compare against the preview snapshot, back up, then atomically replace the destination.
    pub fn apply(&self) -> Result<Option<PathBuf>> {
        let current = read_optional(&self.path)?;
        if current != self.original {
            return Err(
                "The Alacritty config changed while the preview was open. Please try again.".into(),
            );
        }
        let destination = if self.path.is_symlink() {
            fs::canonicalize(&self.path).map_err(|e| e.to_string())?
        } else {
            self.path.clone()
        };
        let parent = destination.parent().ok_or("The config has no parent directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let backup = if let Some(original) = &self.original {
            let mut file = tempfile::Builder::new()
                .prefix("alacritty-before-ghostty-")
                .suffix(".toml")
                .tempfile_in(parent)
                .map_err(|e| e.to_string())?;
            file.write_all(original).map_err(|e| e.to_string())?;
            file.as_file().sync_all().map_err(|e| e.to_string())?;
            Some(file.keep().map_err(|e| e.to_string())?.1)
        } else {
            None
        };
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        if let Ok(metadata) = fs::metadata(&destination) {
            file.as_file().set_permissions(metadata.permissions()).map_err(|e| e.to_string())?;
        }
        file.write_all(self.text.as_bytes()).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        if read_optional(&self.path)? != self.original {
            return Err("The config changed during import; it was not overwritten.".into());
        }
        file.persist(&destination).map_err(|e| e.to_string())?;
        Ok(backup)
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

impl Migration {
    pub fn prepare(&self, path: PathBuf) -> Result<Prepared> {
        if path.extension().is_some_and(|extension| extension == "yml" || extension == "yaml") {
            return Err(
                "Convert your Alacritty YAML config to TOML with ‘alacritty migrate’ first.".into(),
            );
        }
        let original = read_optional(&path)?;
        let contents = std::str::from_utf8(original.as_deref().unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let mut document = contents
            .parse::<DocumentMut>()
            .map_err(|e| format!("Your existing config is not valid TOML: {e}"))?;
        let mut settings = self.settings.clone();
        // Imported bindings override matching chords; preserve every unrelated custom binding.
        if let Some(incoming) = settings
            .get_mut("keyboard")
            .and_then(|v| v.get_mut("bindings"))
            .and_then(Value::as_array_mut)
        {
            let previous: Value = toml::from_str(contents).map_err(|e| e.to_string())?;
            if let Some(existing) =
                previous.get("keyboard").and_then(|v| v.get("bindings")).and_then(Value::as_array)
            {
                let mut bindings: Vec<_> = existing
                    .iter()
                    .filter(|old| !incoming.iter().any(|new| same_chord(old, new)))
                    .cloned()
                    .collect();
                bindings.append(incoming);
                *incoming = bindings;
            }
        }
        let patch = toml::to_string(&settings)
            .map_err(|e| e.to_string())?
            .parse::<DocumentMut>()
            .map_err(|e| e.to_string())?;
        merge(document.as_table_mut(), patch.as_table());
        let text = document.to_string();
        // Validate against Alacritty's real config types before presenting or writing it.
        let _: crate::config::UiConfig = toml::from_str(&text).map_err(|e| e.to_string())?;
        Ok(Prepared { path, text, original })
    }

    pub fn report(&self) -> String {
        let mut report = format!("{} settings translated.\n", self.mapped);
        if self.notes.is_empty() {
            report.push_str("No unsupported settings found.\n");
        } else {
            report.push_str("\nSettings needing attention:\n");
            for note in &self.notes {
                report.push_str(&format!("• {note}\n"));
            }
        }
        report.push_str("\nImported settings:\n");
        report.push_str(&toml::to_string_pretty(&self.settings).unwrap_or_default());
        report
    }
}

fn merge(destination: &mut dyn TableLike, source: &dyn TableLike) {
    for (key, value) in source.iter() {
        if let (Some(current), Some(addition)) =
            (destination.get_mut(key).and_then(Item::as_table_like_mut), value.as_table_like())
        {
            merge(current, addition);
        } else {
            destination.insert(key, value.clone());
        }
    }
}

fn same_chord(a: &Value, b: &Value) -> bool {
    fn mods(value: &Value) -> Vec<String> {
        let mut mods: Vec<_> = value
            .get("mods")
            .and_then(Value::as_str)
            .unwrap_or("")
            .split('|')
            .map(|s| match s.trim().to_lowercase().as_str() {
                "command" | "super" => "super".into(),
                "option" | "alt" => "alt".into(),
                "ctrl" | "control" => "control".into(),
                other => other.into(),
            })
            .collect();
        mods.sort();
        mods.dedup();
        mods
    }
    a.get("key").and_then(Value::as_str).map(str::to_lowercase)
        == b.get("key").and_then(Value::as_str).map(str::to_lowercase)
        && mods(a) == mods(b)
}

pub fn convert(source: &Path, theme_dirs: &[PathBuf], dark: bool) -> Result<Migration> {
    let mut result = Migration { settings: Table::new(), notes: Vec::new(), mapped: 0 };
    let mut entries = Vec::new();
    read_entries(source, &mut entries, &mut HashSet::new(), &mut 0, &mut result.notes, true)?;
    if let Some(theme) = entries.iter().rev().find(|entry| entry.key == "theme") {
        let mut name = theme.value.as_str();
        if name.contains("light:") || name.contains("dark:") {
            let wanted = if dark { "dark:" } else { "light:" };
            name = name
                .split(',')
                .find_map(|part| part.trim().strip_prefix(wanted))
                .unwrap_or("")
                .trim();
            result.notes.push("Imported the current light/dark theme only; automatic theme switching is not supported.".into());
        }
        if !name.is_empty() {
            let path = expand_home(name);
            let theme_path = if path.is_absolute() {
                path.is_file().then_some(path)
            } else if path.components().count() == 1 {
                theme_dirs.iter().map(|dir| dir.join(&path)).find(|path| path.is_file())
            } else {
                None
            };
            if let Some(path) = theme_path {
                let mut theme_entries = Vec::new();
                read_entries(
                    &path,
                    &mut theme_entries,
                    &mut HashSet::new(),
                    &mut 0,
                    &mut result.notes,
                    false,
                )?;
                // Theme colors are lower-priority defaults; non-color behavior is never
                // imported from a theme (themes can contain startup commands, too).
                theme_entries.retain(|entry| {
                    let supported = matches!(
                        entry.key.as_str(),
                        "background"
                            | "foreground"
                            | "palette"
                            | "cursor-color"
                            | "cursor-text"
                            | "selection-background"
                            | "selection-foreground"
                    );
                    if !supported {
                        result.notes.push(format!(
                            "{}: non-color theme setting ‘{}’ was not imported.",
                            entry.location, entry.key
                        ));
                    }
                    supported
                });
                theme_entries.extend(entries);
                entries = theme_entries;
            } else {
                result.notes.push(format!(
                    "{}: theme ‘{name}’ could not be found; its colors were not imported.",
                    theme.location
                ));
            }
        }
    }
    let mut scalar = BTreeMap::new();
    let mut palette = BTreeMap::new();
    let mut bindings = Vec::new();
    for entry in entries {
        match entry.key.as_str() {
            "theme" => (),
            "palette" => {
                if entry.value.is_empty() {
                    palette.clear();
                    result
                        .notes
                        .push(format!("{}: palette reset is not translated.", entry.location));
                } else if let Some((index, value)) = entry.value.split_once('=') {
                    match index.trim().parse::<u8>().ok().zip(color(value.trim())) {
                        Some((index, value)) => {
                            palette.insert(index, value);
                        },
                        None => {
                            result.notes.push(format!("{}: invalid palette entry.", entry.location))
                        },
                    }
                } else {
                    result.notes.push(format!("{}: invalid palette entry.", entry.location));
                }
            },
            "keybind" => {
                if entry.value.is_empty() {
                    bindings.clear();
                } else if let Some(binding) = keybinding(&entry.value) {
                    bindings.retain(|old| !same_chord(old, &binding));
                    bindings.push(binding);
                } else {
                    result.notes.push(format!("{}: keybind is unsupported (only simple chords and equivalent actions are imported).", entry.location));
                }
            },
            key if key.starts_with("font-family") => {
                if entry.value.is_empty() {
                    scalar.remove(&entry.key);
                    result.notes.push(format!(
                        "{}: font-family reset needs a font choice in Alacritty; left unchanged.",
                        entry.location
                    ));
                } else if scalar.contains_key(&entry.key) {
                    result.notes.push(format!(
                        "{}: fallback font skipped; Alacritty accepts one family per style.",
                        entry.location
                    ));
                } else {
                    scalar.insert(entry.key.clone(), entry);
                }
            },
            _ => {
                scalar.insert(entry.key.clone(), entry);
            },
        }
    }
    for (index, value) in palette {
        if index < 16 {
            set(
                &mut result.settings,
                &["colors", if index < 8 { "normal" } else { "bright" }, ANSI[index as usize % 8]],
                value.into(),
            );
        } else {
            let item = Value::Table(Table::from_iter([
                ("index".into(), Value::Integer(index.into())),
                ("color".into(), value.into()),
            ]));
            push(&mut result.settings, &["colors", "indexed_colors"], item);
        }
        result.mapped += 1;
    }
    let dimensions_valid = ["window-width", "window-height"].iter().all(|key| {
        scalar
            .get(*key)
            .and_then(|entry| entry.value.parse::<u32>().ok())
            .is_some_and(|size| (1..=1000).contains(&size))
    });
    for entry in scalar.values() {
        if matches!(entry.key.as_str(), "window-width" | "window-height") && !dimensions_valid {
            result.notes.push(format!("{}: Alacritty requires both a valid window-width and window-height; dimensions were not imported.", entry.location));
        } else {
            translate(entry, &mut result);
        }
    }
    if scalar.get("fullscreen").is_some_and(|entry| entry.value == "true") {
        set(&mut result.settings, &["window", "startup_mode"], "Fullscreen".into());
    }
    result.mapped += bindings.len();
    if !bindings.is_empty() {
        set(&mut result.settings, &["keyboard", "bindings"], Value::Array(bindings));
    }
    Ok(result)
}

fn read_entries(
    path: &Path,
    entries: &mut Vec<Entry>,
    stack: &mut HashSet<PathBuf>,
    count: &mut usize,
    notes: &mut Vec<String>,
    includes: bool,
) -> Result<()> {
    *count += 1;
    if *count > MAX_FILES {
        return Err("Too many included config files (limit: 64).".into());
    }
    let canonical =
        fs::canonicalize(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    if !stack.insert(canonical.clone()) {
        return Err(format!("Config include cycle at {}", path.display()));
    }
    let metadata = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
        return Err(format!("{} must be a regular file smaller than 2 MiB", path.display()));
    }
    let text = fs::read_to_string(&canonical).map_err(|e| e.to_string())?;
    let mut nested = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let location = format!("{}:{}", path.display(), line + 1);
        let Some((key, value)) = raw.split_once('=') else {
            notes.push(format!("{location}: expected key = value; skipped."));
            continue;
        };
        let value = value.trim();
        let quoted = value.starts_with('"');
        let value = if quoted {
            let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
                return Err(format!("{location}: unterminated quoted value"));
            };
            if value.contains('\\') {
                notes.push(format!("{location}: quoted escape sequences are not imported."));
                continue;
            }
            value
        } else {
            value
        };
        let entry = Entry { key: key.trim().into(), value: value.into(), location, quoted };
        if entry.key == "config-file" {
            if includes {
                nested.push(entry);
            } else {
                notes.push(format!("{}: config-file inside a theme was ignored.", entry.location));
            }
        } else if entry.key != "theme" || includes {
            entries.push(entry);
        }
    }
    // Ghostty loads includes after every setting in the containing file.
    for entry in nested {
        let optional = !entry.quoted && entry.value.starts_with('?');
        let value = if optional { &entry.value[1..] } else { &entry.value };
        let include = expand_home(value);
        let include = if include.is_absolute() {
            include
        } else {
            path.parent().unwrap_or(Path::new(".")).join(include)
        };
        if optional && !include.exists() {
            continue;
        }
        read_entries(&include, entries, stack, count, notes, true)?;
    }
    stack.remove(&canonical);
    Ok(())
}

fn expand_home(value: &str) -> PathBuf {
    if value == "~" {
        return home::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    match value.strip_prefix("~/").and_then(|rest| home::home_dir().map(|home| home.join(rest))) {
        Some(path) => path,
        None => PathBuf::from(value),
    }
}

fn color(value: &str) -> Option<String> {
    let value = value.strip_prefix('#').unwrap_or(value);
    (value.len() == 6 && value.bytes().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{value}"))
}

fn set(table: &mut Table, path: &[&str], value: Value) {
    if path.len() == 1 {
        table.insert(path[0].into(), value);
        return;
    }
    let child = table.entry(path[0]).or_insert_with(|| Value::Table(Table::new()));
    set(child.as_table_mut().unwrap(), &path[1..], value);
}
fn push(table: &mut Table, path: &[&str], value: Value) {
    if path.len() == 1 {
        table
            .entry(path[0])
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .unwrap()
            .push(value);
        return;
    }
    let child = table.entry(path[0]).or_insert_with(|| Value::Table(Table::new()));
    push(child.as_table_mut().unwrap(), &path[1..], value);
}

fn translate(entry: &Entry, result: &mut Migration) {
    let key = entry.key.as_str();
    let value = entry.value.as_str();
    let string = |path: &[&str], value: &str, result: &mut Migration| {
        set(&mut result.settings, path, value.into());
        true
    };
    let number =
        |path: &[&str], low: f64, high: f64, integer: bool, result: &mut Migration| match value
            .parse::<f64>()
        {
            Ok(n) if n.is_finite() && n >= low && n <= high && (!integer || n.fract() == 0.) => {
                set(
                    &mut result.settings,
                    path,
                    if integer { Value::Integer(n as i64) } else { Value::Float(n) },
                );
                true
            },
            _ => false,
        };
    let boolean = |path: &[&str], result: &mut Migration| match value.parse::<bool>() {
        Ok(value) => {
            set(&mut result.settings, path, value.into());
            true
        },
        _ => false,
    };
    let rgb = |path: &[&str], result: &mut Migration| match color(value) {
        Some(value) => string(path, &value, result),
        None => false,
    };
    let mapped = if value.is_empty() {
        false
    } else {
        match key {
            "font-family" => string(&["font", "normal", "family"], value, result),
            "font-family-bold" => string(&["font", "bold", "family"], value, result),
            "font-family-italic" => string(&["font", "italic", "family"], value, result),
            "font-family-bold-italic" => string(&["font", "bold_italic", "family"], value, result),
            "font-style" | "font-style-bold" | "font-style-italic" | "font-style-bold-italic"
                if value != "false" && value != "true" =>
            {
                let style = match key {
                    "font-style-bold" => "bold",
                    "font-style-italic" => "italic",
                    "font-style-bold-italic" => "bold_italic",
                    _ => "normal",
                };
                string(&["font", style, "style"], value, result)
            },
            "font-size" => number(&["font", "size"], 1., 512., false, result),
            "background" => rgb(&["colors", "primary", "background"], result),
            "foreground" => rgb(&["colors", "primary", "foreground"], result),
            "cursor-color" => rgb(&["colors", "cursor", "cursor"], result),
            "cursor-text" => rgb(&["colors", "cursor", "text"], result),
            "selection-background" => rgb(&["colors", "selection", "background"], result),
            "selection-foreground" => rgb(&["colors", "selection", "text"], result),
            "background-opacity" => number(&["window", "opacity"], 0., 1., false, result),
            "background-opacity-cells" => {
                boolean(&["colors", "transparent_background_colors"], result)
            },
            "background-blur" => match value
                .parse::<bool>()
                .ok()
                .or_else(|| value.parse::<u32>().ok().map(|n| n > 0))
            {
                Some(blur) => {
                    set(&mut result.settings, &["window", "blur"], blur.into());
                    result.notes.push(format!(
                        "{}: blur is on/off in Alacritty; radius is not preserved.",
                        entry.location
                    ));
                    true
                },
                _ => false,
            },
            "window-padding-x" => number(&["window", "padding", "x"], 0., 255., true, result),
            "window-padding-y" => number(&["window", "padding", "y"], 0., 255., true, result),
            "window-padding-balance" => boolean(&["window", "dynamic_padding"], result),
            "window-width" => number(&["window", "dimensions", "columns"], 1., 1000., true, result),
            "window-height" => number(&["window", "dimensions", "lines"], 1., 1000., true, result),
            "cursor-style" => match value {
                "block" => string(&["cursor", "style", "shape"], "Block", result),
                "bar" => string(&["cursor", "style", "shape"], "Beam", result),
                "underline" => string(&["cursor", "style", "shape"], "Underline", result),
                _ => false,
            },
            "cursor-style-blink" => match value {
                "true" => string(&["cursor", "style", "blinking"], "On", result),
                "false" => string(&["cursor", "style", "blinking"], "Off", result),
                _ => false,
            },
            "mouse-hide-while-typing" => boolean(&["mouse", "hide_when_typing"], result),
            "title" => {
                set(&mut result.settings, &["window", "dynamic_title"], false.into());
                string(&["window", "title"], value, result)
            },
            "working-directory" if expand_home(value).is_absolute() => string(
                &["general", "working_directory"],
                &expand_home(value).to_string_lossy(),
                result,
            ),
            "macos-option-as-alt" => match value {
                "true" => string(&["window", "option_as_alt"], "Both", result),
                "false" => string(&["window", "option_as_alt"], "None", result),
                "left" => string(&["window", "option_as_alt"], "OnlyLeft", result),
                "right" => string(&["window", "option_as_alt"], "OnlyRight", result),
                _ => false,
            },
            "window-decoration" => match value {
                "auto" | "true" => string(&["window", "decorations"], "Full", result),
                "none" | "false" => {
                    result.notes.push(
                        "Window decorations disabled: macOS native tabs will be unavailable."
                            .into(),
                    );
                    string(&["window", "decorations"], "None", result)
                },
                _ => false,
            },
            "maximize" | "fullscreen" => match value {
                "true" => string(
                    &["window", "startup_mode"],
                    if key == "fullscreen" { "Fullscreen" } else { "Maximized" },
                    result,
                ),
                "false" => string(&["window", "startup_mode"], "Windowed", result),
                _ => false,
            },
            _ => false,
        }
    };
    if mapped {
        result.mapped += 1;
    } else {
        result.notes.push(format!(
            "{}: ‘{key}’ is unsupported or its value cannot be represented; left unchanged.",
            entry.location
        ));
    }
}

fn keybinding(value: &str) -> Option<Value> {
    let (trigger, action) = value.split_once('=')?;
    if trigger.contains([':', '>']) {
        return None;
    }
    let mut parts: Vec<_> = trigger.trim().split('+').collect();
    let key = parts.pop()?;
    let key = match key {
        "enter" | "return" => "Enter",
        "backspace" => "Backspace",
        "tab" => "Tab",
        "escape" => "Escape",
        "space" => "Space",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        key if key.chars().count() == 1 => key,
        _ => return None,
    };
    let mods: Option<Vec<_>> = parts
        .into_iter()
        .map(|part| match part {
            "super" | "cmd" | "command" => Some("Super"),
            "ctrl" | "control" => Some("Control"),
            "alt" | "opt" | "option" => Some("Alt"),
            "shift" => Some("Shift"),
            _ => None,
        })
        .collect();
    let action = match action.trim() {
        "new_tab" => "CreateNewTab",
        "new_window" => "CreateNewWindow",
        "close_surface" => "ClosePane",
        "close_tab" => "Quit",
        "new_split:right" => "SplitRight",
        "new_split:down" => "SplitDown",
        "goto_split:next" => "FocusNextPane",
        "goto_split:previous" => "FocusPreviousPane",
        "copy_to_clipboard" => "Copy",
        "paste_from_clipboard" => "Paste",
        "previous_tab" => "SelectPreviousTab",
        "next_tab" => "SelectNextTab",
        "increase_font_size:1" | "increase_font_size" => "IncreaseFontSize",
        "decrease_font_size:1" | "decrease_font_size" => "DecreaseFontSize",
        "reset_font_size" => "ResetFontSize",
        "open_config" => "OpenConfig",
        "toggle_fullscreen" => "ToggleFullscreen",
        "ignore" => "None",
        "goto_tab:1" => "SelectTab1",
        "goto_tab:2" => "SelectTab2",
        "goto_tab:3" => "SelectTab3",
        "goto_tab:4" => "SelectTab4",
        "goto_tab:5" => "SelectTab5",
        "goto_tab:6" => "SelectTab6",
        "goto_tab:7" => "SelectTab7",
        "goto_tab:8" => "SelectTab8",
        "goto_tab:9" => "SelectLastTab",
        _ => return None,
    };
    Some(Value::Table(Table::from_iter([
        ("key".into(), key.into()),
        ("mods".into(), mods?.join("|").into()),
        ("action".into(), action.into()),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_split_bindings_and_distinguishes_pane_from_tab_close() {
        let value = keybinding("super+d=new_split:right").unwrap();
        assert_eq!(value["action"].as_str(), Some("SplitRight"));
        let value = keybinding("super+w=close_surface").unwrap();
        assert_eq!(value["action"].as_str(), Some("ClosePane"));
        let value = keybinding("super+shift+w=close_tab").unwrap();
        assert_eq!(value["action"].as_str(), Some("Quit"));
        assert!(keybinding("super+d=new_split:auto").is_none());
    }

    #[test]
    fn themes_includes_and_explicit_colors_have_ghostty_precedence() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("theme"), "background = 111111\npalette = 0=#222222\n").unwrap();
        fs::write(dir.path().join("child"), "background = 444444\nfont-size = 14\n").unwrap();
        let source = dir.path().join("config");
        fs::write(
            &source,
            "theme = theme\nconfig-file = child\nbackground = 333333\nconfig-file = ?missing\n",
        )
        .unwrap();
        let converted = convert(&source, &[dir.path().into()], true).unwrap();
        assert_eq!(converted.settings["colors"]["primary"]["background"].as_str(), Some("#444444"));
        assert_eq!(converted.settings["colors"]["normal"]["black"].as_str(), Some("#222222"));
        assert_eq!(converted.settings["font"]["size"].as_float(), Some(14.));
    }
    #[test]
    fn backup_merge_and_preview_race() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("ghostty");
        let dest = dir.path().join("alacritty.toml");
        fs::write(&source, "font-size = 15\nbackground = 123456\nkeybind = super+t=new_tab\n")
            .unwrap();
        let old = "# keep this comment\n[window]\nopacity = 0.8\n[keyboard]\nbindings = [{ key = 't', mods = 'Command', action = 'Quit' }, { key = 'x', mods = 'Alt', action = 'Paste' }]\n";
        fs::write(&dest, old).unwrap();
        let converted = convert(&source, &[], true).unwrap();
        let prepared = converted.prepare(dest.clone()).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), old);
        let backup = prepared.apply().unwrap().unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), old);
        let new = fs::read_to_string(&dest).unwrap();
        assert!(new.contains("# keep this comment"));
        let config: Value = toml::from_str(&new).unwrap();
        assert_eq!(config["window"]["opacity"].as_float(), Some(0.8));
        assert_eq!(config["keyboard"]["bindings"].as_array().unwrap().len(), 2);
        assert!(prepared.apply().is_err());
    }
    #[test]
    fn unsupported_commands_and_cycles_are_not_silently_imported() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config");
        fs::write(&source, "command = touch /tmp/never-executed\nscrollback-limit = 100000\nfont-size = NaN\nkeybind = global:super+t=new_tab\n").unwrap();
        let converted = convert(&source, &[], true).unwrap();
        assert!(converted.settings.is_empty());
        assert_eq!(converted.notes.len(), 4);
        fs::write(&source, "config-file = config\n").unwrap();
        assert!(convert(&source, &[], true).is_err());
    }
    #[test]
    fn missing_theme_and_asymmetric_padding_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("config");
        fs::write(&source, "theme = missing\nwindow-padding-x = 2,4\ncursor-style = bar\n")
            .unwrap();
        let converted = convert(&source, &[], true).unwrap();
        assert_eq!(converted.notes.len(), 2);
        assert_eq!(converted.settings["cursor"]["style"]["shape"].as_str(), Some("Beam"));
    }
    #[test]
    fn invalid_destination_and_cancellation_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("ghostty");
        let dest = dir.path().join("nested/config.toml");
        fs::write(&source, "font-size = 13\n").unwrap();
        let converted = convert(&source, &[], true).unwrap();
        let _preview = converted.prepare(dest.clone()).unwrap();
        assert!(!dest.parent().unwrap().exists());
        fs::create_dir(dest.parent().unwrap()).unwrap();
        fs::write(&dest, "not toml").unwrap();
        assert!(converted.prepare(dest.clone()).is_err());
        assert_eq!(fs::read_to_string(dest).unwrap(), "not toml");
    }
    #[test]
    fn preserves_config_symlink_and_reports_partial_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("ghostty");
        let target = dir.path().join("real.toml");
        let link = dir.path().join("alacritty.toml");
        fs::write(&source, "window-width = 120\nfont-size = 14\n").unwrap();
        fs::write(&target, "# original\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let converted = convert(&source, &[], true).unwrap();
        assert_eq!(converted.notes.len(), 1);
        assert!(!converted.settings.contains_key("window"));
        let backup = converted.prepare(link.clone()).unwrap().apply().unwrap().unwrap();
        assert!(link.is_symlink());
        assert_eq!(fs::read_to_string(backup).unwrap(), "# original\n");
        assert!(fs::read_to_string(target).unwrap().contains("size = 14"));
    }

    #[test]
    fn selects_current_theme_and_keeps_theme_behavior_out_of_config() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("dark"), "background = 111111\nfont-family = ThemeFont\n")
            .unwrap();
        fs::write(dir.path().join("light"), "background = eeeeee\n").unwrap();
        let source = dir.path().join("config");
        fs::write(&source, "theme = light:light,dark:dark\nfont-family = UserFont\n").unwrap();
        let converted = convert(&source, &[dir.path().into()], true).unwrap();
        assert_eq!(converted.settings["colors"]["primary"]["background"].as_str(), Some("#111111"));
        assert_eq!(converted.settings["font"]["normal"]["family"].as_str(), Some("UserFont"));
        assert_eq!(converted.notes.len(), 2);
    }
}
