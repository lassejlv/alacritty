# macOS tabs, split panes, menus, and config migration

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

## Delete the current input line

Cmd+Delete and Ctrl+Delete clear the whole current shell input line, including
text after the cursor. Both the Mac Delete key, reported as Backspace, and
forward Delete are supported. Ctrl+Backspace/Delete also work on other platforms.

The shortcut sends Ctrl+A followed by Ctrl+K, using the shell's normal Emacs-style
line editing. It does not run while terminal vi mode, search, an alternate screen,
or enhanced keyboard reporting is active. Shells with custom or vi keymaps may
need a different binding in `keyboard.bindings`.

## Selection and context menu

Cmd+A or Edit → Select All selects the focused pane's entire buffer, including
scrollback. Copy with Cmd+C. On an alternate screen, selection covers only that
screen. The shortcut can be remapped with the `SelectAll` binding action.

Right-click or Control-click a pane to open its native macOS context menu. It
includes Copy, Paste, Select All, Clear Selection, Find, Clear Scrollback, Split
Right, Split Down, New Tab, and New Window. Copy and Clear Selection are disabled
without a selection. Commands apply to the pane you clicked.

Programs that enable mouse reporting still receive ordinary right-clicks. Hold
Shift while right-clicking to open the native menu instead.

## Split panes

Each native tab can contain multiple independent terminals. Splits can be nested:

| Action | Shortcut | Menu |
| --- | --- | --- |
| Split the focused pane to the right | Cmd+D | File → Split Right |
| Split the focused pane below | Cmd+Shift+D | File → Split Down |
| Focus next / previous pane | Cmd+Option+Right / Left | Window → Next / Previous Pane |
| Close the focused pane | Cmd+W | File → Close Pane |
| Close the entire tab and its panes | Cmd+Shift+W | File → Close Tab |

Click a pane to focus it, or drag a divider to resize its terminals. A subtle
outline identifies the focused pane. New panes inherit the foreground process's
working directory and start a new shell. Each pane keeps its own scrollback,
selection, search, terminal colors, and cursor state. Scrolling targets the pane
under the pointer without changing keyboard focus. Font zoom is independent for each pane. Cmd+Plus, Cmd+Minus, and Cmd+0
change or reset only the focused pane. New splits inherit the focused pane's
zoom, then retain their own size. Zoom is preserved when switching panes or
moving the window between displays.

Closing a pane (or exiting its shell) expands its sibling. Closing the last pane
closes its tab. Closing the native window/tab closes all its panes. Splits stay
inside their tab when creating or switching tabs. A split is ignored if there
isn't room for two panes with at least 12 columns and four rows each.

Custom bindings can use `SplitRight`, `SplitDown`, `FocusNextPane`,
`FocusPreviousPane`, and `ClosePane`. `Quit` closes the entire native tab. These
actions also work on other platforms when configured explicitly; the default
shortcuts and native menu integration above are macOS-specific.

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
and simple keyboard chords with equivalent Alacritty actions, including right/down
splits, next/previous pane focus, and closing a pane. Both window
width and height must be specified to import startup dimensions.

`config-file` includes follow Ghostty's order: included files load after their
containing file. Missing optional includes are allowed; cycles and oversized
files are rejected. Named theme colors are resolved from Ghostty's user themes,
a `themes` directory beside the selected config, or an installed Ghostty app's
bundled themes. Explicit config colors override theme colors.

The preview reports unsupported or approximate settings. Examples include
fallback fonts, font features, asymmetric padding, scrollback byte limits,
unsupported split directions/actions, shaders, shell integration, commands, key sequences, global shortcuts,
and unresolved themes. Light/dark theme pairs import the currently selected
appearance and report the loss of automatic theme switching. Commands in the
source config or a theme are never executed. Only color settings are imported
from theme files.

If Ghostty loads multiple independent root configs, select the desired root
file explicitly; its includes are imported with it. Ghostty files are left
unchanged.

## Clipboard formats

Terminal applications can use the [Kitty clipboard protocol](../protocols/kitty-clipboard.md)
to copy and paste text, images, HTML, and other MIME formats. Clipboard access
uses the existing `terminal.osc52` policy. The Paste shortcut and native menu
also support MIME paste notifications when requested by the application.

## Inline images

Kitty graphics clients can display images and animations inside any tab or split
pane. See [Kitty graphics support](../protocols/kitty-graphics.md) for supported operations and
a self-contained visual test.
