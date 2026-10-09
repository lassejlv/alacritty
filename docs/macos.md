# macOS tabs, menus, and config migration

**Cmd+T** and **File → New Tab** add a tab to the current window, including the
first tab. **Cmd+N** creates a separate window. Tab groups remain independent.
Native tabs require window decorations; New Tab is disabled when decorations
are `None`.

The application menu provides **Open Config…**, **Reload Config**,
**Migrate Ghostty Config…**, and the existing **Check for Updates…** command.
File, Edit, View, and Window menus provide terminal creation/closing, copy,
paste, search, font sizing, clear scrollback, fullscreen, minimization, and tab
navigation. Menu actions target the focused terminal. Native accelerators are
removed when the corresponding terminal shortcut has been remapped in config.

## Migrate Ghostty Config

1. Choose **Alacritty → Migrate Ghostty Config…** and select a Ghostty config.
   The picker starts in a detected Ghostty config directory when available.
2. Review the imported TOML settings and compatibility notes.
3. Choose **Import** to merge supported settings into the active Alacritty
   config. Matching settings are replaced; unrelated settings are preserved.
   Cancel leaves the config unchanged.

The importer creates an `alacritty-before-ghostty-*.toml` backup next to the
config before replacing it atomically, preserves config symlinks, and rejects
an import if the destination changed after the preview. Imported appearance
settings reload immediately; startup settings affect new terminals.

Supported settings include font families/styles/sizes, hex RGB colors and ANSI
palettes, cursor shape/blinking, selection colors, opacity, padding, blur,
window dimensions, startup mode, Option-as-Alt, absolute working directories,
and simple keyboard chords with equivalent Alacritty actions. Both window
width and height must be specified to import startup dimensions.

`config-file` includes follow Ghostty's order: included files load after their
containing file. Missing optional includes are allowed; cycles and oversized
files are rejected. Named theme colors are resolved from Ghostty's user themes,
a `themes` directory beside the selected config, or an installed Ghostty app's
bundled themes. Explicit config colors override theme colors.

The preview reports unsupported or approximate settings. Examples include
fallback fonts, font features, asymmetric padding, scrollback byte limits,
splits, shaders, shell integration, commands, key sequences, global shortcuts,
and unresolved themes. Light/dark theme pairs import the currently selected
appearance and report the loss of automatic theme switching. Commands in the
source config or a theme are never executed. Only color settings are imported
from theme files.

If Ghostty loads multiple independent root configs, select the desired root
file explicitly; its includes are imported with it. Ghostty files are left
unchanged.
