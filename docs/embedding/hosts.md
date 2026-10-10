# Building a terminal host

A host owns the window, fonts and drawing; the engine owns everything a terminal
emulator knows. `crates/ffi/examples/sdl_host.c` is a complete host in under 500
lines of C, using SDL2 and SDL2_ttf. Read it next to this guide.

## Setup

Measure your font first. The cell size is the advance width of a monospace
glyph by the line height, in the same pixels you draw with (physical pixels on
high-DPI displays).

```c
AlacOptions options;
alac_options_default(&options);
options.cell_width = cell_width;
options.cell_height = cell_height;
options.columns = window_width / cell_width;
options.rows = window_height / cell_height;

AlacTerminal *term = alac_terminal_spawn(&options, NULL, wakeup, window);
AlacSnapshot *frame = alac_snapshot_new();
```

A null `AlacPtyOptions` starts the user's login shell. `TERM` defaults to
`xterm-256color` and `COLORTERM` to `truecolor`; override them in `env`. The cell
size reaches the program through pixel-size queries and Kitty graphics, so keep
it accurate and call `alac_terminal_resize` whenever the window or font changes.

## Main loop

1. The wakeup callback posts an event to your UI thread. It runs on the PTY
   thread, possibly while the terminal is locked, so it must not call the
   library.
2. On the UI thread, drain `alac_terminal_poll_event` and handle the events in
   the table below.
3. Redraw after `ALAC_EVENT_WAKEUP` or input, at most once per display frame.
4. Schedule `alac_terminal_tick` after `alac_terminal_next_timeout_ms`.

| Event | Host action |
| --- | --- |
| `WAKEUP` | Request a redraw |
| `TITLE` | Set the window title; null `data` restores your default |
| `BELL` | Flash, play a sound or mark the window urgent |
| `CHILD_EXIT`, `EXIT` | Close the terminal or show the exit code |
| `NOTIFICATION_SHOW`, `_CLOSE`, `_QUERY_ALIVE` | See [Notifications](#notifications) |
| `PROGRESS_CHANGED` | Show `progress` in a dock, taskbar or tab |
| `WORKING_DIRECTORY_CHANGED` | Remember `alac_terminal_working_directory` for new tabs |
| `PROGRAM_STATUS_CHANGED` | Read `alac_terminal_program_statuses` for tab badges |
| `CURSOR_BLINKING_CHANGE` | Restart your blink timer |
| `MOUSE_CURSOR_DIRTY` | Reconsider the pointer shape (I-beam vs arrow when mouse reporting changes) |
| `OUTPUT` | Headless terminals only: send `data` to the remote program |

## Drawing

Keep one snapshot per terminal and refresh it before each frame:

```c
alac_terminal_snapshot_update(term, frame);
AlacSnapshotInfo info;
alac_snapshot_info(frame, &info);
```

Updating copies only damaged rows when this snapshot was the last one updated
for the terminal, which makes typing about 60 times cheaper than a full copy on
a 200x60 grid. Retained-mode renderers can redraw only `info.damage` rows unless
`info.full_damage` is set; immediate-mode renderers can ignore damage.

Draw in this order, which matches Alacritty:

1. Clear to `info.background`.
2. Images with `z_index < INT32_MIN / 2`.
3. Cell backgrounds. Skip cells with `ALAC_CELL_DEFAULT_BACKGROUND` to let window
   transparency show through.
4. Images with `INT32_MIN / 2 <= z_index < 0`.
5. Glyphs, underlines and strikeouts.
6. Images with `z_index >= 0`.
7. The cursor.

Cells are row-major, `info.columns` per row. Each `AlacCell` has resolved colors:
bold-as-bright, dim, inverse and hidden are already applied to `fg` and `bg`.
The low 16 bits of `flags` are the raw attributes; the rest are `ALAC_CELL_*`
flags computed for drawing:

- `WIDE_CHAR` cells span two columns; skip the following `WIDE_CHAR_SPACER`.
- Combining characters are `codepoints[zerowidth_index ..
  zerowidth_index + zerowidth_count]`, or use `alac_snapshot_cell_text` for the
  whole cluster as UTF-8.
- `SELECTED` marks the selection. Draw it however you like; swapping `fg` and
  `bg` matches Alacritty's default.
- `UNDERLINE`, `DOUBLE_UNDERLINE`, `UNDERCURL`, `DOTTED_UNDERLINE` and
  `DASHED_UNDERLINE` use `underline_color`.
- `hyperlink` is a one-based index for `alac_snapshot_hyperlink_uri`. Cells of
  one link share `alac_snapshot_hyperlink_id`, so you can underline the whole
  link on hover.
- `IMAGE_PLACEHOLDER` cells belong to a Kitty Unicode placeholder image; don't
  draw their glyph. The image is in `info.images`.

Images arrive positioned and clipped: draw the `src_*` rectangle of the image to
the `dest_*` rectangle, in pixels from the top-left of the grid. `rgba` holds
straight-alpha RGBA; when it is null, decode `png` yourself. Cache textures by
`image_id`, `placement_id` and `generation`; the generation changes when pixels
change, including animation frames.

The cursor is in `info.cursor`. Unfocused terminals report a hollow block; draw
the glyph under a block cursor in `cursor.text_color`. `wide` cursors cover two
columns. In vi mode the cursor is the vi cursor.

## Keyboard

Send every key press, repeat and release with `alac_terminal_key`, and text
committed by an input method with `alac_terminal_text`. The engine encodes keys
for the mode the program enabled, from legacy sequences to all Kitty keyboard
protocol flags, so release events and modifier keys matter.

`AlacKeyEvent` fields:

- `key` is `ALAC_KEY_CHARACTER` with `codepoint` set to the character produced
  with Shift and the keyboard layout applied, or a named key.
- `unshifted_codepoint` is the key without modifiers, for Kitty's alternate keys.
- `text` is the text the key produced with all modifiers, or null to derive it.
- `alt_composes` tells the engine that Alt/Option produced `text`, like macOS
  Option+e for accents. Without it, Alt sends an escape prefix.
- `location` distinguishes the numeric keypad and left/right modifiers.

`alac_terminal_key` returns false when it didn't use the key. Handle your own
shortcuts before calling it; copy and paste shortcuts usually belong to the
host.

Paste with `alac_terminal_paste_clipboard` (through your clipboard callbacks) or
`alac_terminal_paste` (text you provide). Both use bracketed paste when the
program enabled it and strip sequences that could end it early.

## Mouse

Convert pointer positions with `alac_terminal_cell_at`, which also returns which
half of the cell was hit. Then offer each event to the program first:

```c
if (!alac_terminal_mouse_button(term, ALAC_MOUSE_LEFT, pressed, column, row, mods)) {
    /* Not reported to the program: select text. */
    if (pressed) alac_terminal_select_click(term, column, row, side, click_count, block);
}
```

Mouse functions return true when the program received the event. Holding Shift
always bypasses reporting, so users can select text in programs that capture
the mouse. While dragging, call `alac_terminal_mouse_move` and, if it returns
false, `alac_terminal_select_update`. `alac_terminal_select_click` selects cells,
words or lines for 1, 2 or 3 clicks; pass `block` for rectangular selection,
conventionally with Ctrl or Option.

`alac_terminal_mouse_wheel` takes whole lines and returns what happened: reported
to the program, converted to arrow keys for alternate-screen programs like
`less`, or scrolled through history. Accumulate fractional trackpad deltas
yourself.

Use `alac_terminal_link_at` on hover or Ctrl+click to open URLs and OSC 8
hyperlinks, and `alac_terminal_visible_links` for keyboard hint overlays.
`alac_terminal_set_url_pattern` changes which plain-text URLs are detected.

## Clipboard

Install `AlacClipboardCallbacks` to give programs clipboard access through OSC 52
and the Kitty clipboard protocol (OSC 5522). The engine checks focus and
`AlacOptions.osc52` before calling you, handles chunking, MIME lists and paste
passwords, and calls `write` with every representation of a copy.

`alac_terminal_copy_selection` and `alac_terminal_paste_clipboard` are user
actions and skip the OSC 52 policy. Pass `ALAC_CLIPBOARD_PRIMARY` for the X11
primary selection; return false from callbacks for locations you don't support.

## Notifications

`ALAC_EVENT_NOTIFICATION_SHOW` carries an assembled OSC 9 or OSC 99 notification.
The engine has already applied `desktop_notifications`, focus and visibility
policy, so show it. Then report what the user did:

- `alac_terminal_notification_activated(term, serial, button)` when it was
  clicked (`button` is -1) or a button was pressed. Focus the terminal when it
  returns true.
- `alac_terminal_notification_closed` when it was dismissed or failed to show.
- On `NOTIFICATION_QUERY_ALIVE`, pass the serials you still show to
  `alac_terminal_notifications_alive`.

Close notifications on `NOTIFICATION_CLOSE`. Expiry is handled by
`alac_terminal_tick`. Call `alac_terminal_set_visible` when the terminal is
hidden, for example in a background tab, so notifications marked "only when
invisible" behave.

## Colors and themes

`alac_terminal_set_primary_colors` sets foreground and background and derives
cursor, bright and dim variants. `alac_terminal_set_colors` replaces the 16 ANSI
colors, and `alac_terminal_set_color` sets any index up to `ALAC_COLOR_COUNT`.
Programs can override colors with OSC 4, 10, 11 and 12; `alac_terminal_color`
returns the effective color. Color changes damage the whole snapshot.

## Search, vi mode and history

- `alac_terminal_search` finds a regex across the screen and scrollback, selects
  the match and scrolls to it. Call it again to continue; pass a negative
  direction to search upward.
- `alac_terminal_vi_toggle` enters vi mode. Keys sent to `alac_terminal_key` then
  drive the vi cursor with Alacritty's default bindings and never reach the
  program. `/` and `?` return false so you can open your search UI.
- `alac_terminal_scroll` and `alac_terminal_jump_to_prompt` move through history;
  prompt jumps need OSC 133 shell integration.

## Checklist

- Poll events after every wakeup and after sending input.
- Resize the terminal when the window, font or DPI changes.
- Report focus and visibility changes.
- Free every snapshot, string, list and writer you receive.
- Test with `vim`, `less`, `htop`, a Kitty graphics program such as
  `kitten icat`, and `printf '\e]9;hello\a'`.
