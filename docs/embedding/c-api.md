# C API reference

`crates/ffi/include/alacritty.h` is the authoritative declaration of every type
and constant; this page groups the functions by task and states the rules that
apply to all of them. The ABI test (`cargo test -p alacritty_ffi --test c_abi`)
checks every struct layout and constant in the header against the Rust
definitions.

## Conventions

- Handles (`AlacTerminal`, `AlacSnapshot`, `AlacWriter`, lists) are opaque. Every
  function accepts null handles and returns a neutral value.
- Strings passed in are UTF-8 and NUL-terminated unless a length is given.
- Colors are packed `0xRRGGBB`.
- Functions that fail return null, false or -1 and set `alac_last_error()`,
  which is per thread and valid until the next failing call on that thread.
- Viewport functions take cells with row 0 at the top of the visible area.
  `AlacMatch` and `alac_terminal_text_range` use grid lines: 0 is the top of the
  screen and negative lines are scrollback.
- Functions copying text into caller buffers write at most `cap - 1` bytes and a
  NUL, and return the full length so callers can retry with a larger buffer.

## Ownership

| You receive | Valid until | Release with |
| --- | --- | --- |
| `char *` results | You free it | `alac_string_free` |
| `AlacTerminal *` | You free it | `alac_terminal_free` |
| `AlacSnapshot *` | You free it | `alac_snapshot_free` |
| Pointers in `AlacSnapshotInfo` | The snapshot is updated or freed | — |
| `const char *` from `alac_snapshot_hyperlink_*` | The snapshot is updated or freed | — |
| Pointers in `AlacEvent` | The next `alac_terminal_poll_event` | — |
| `AlacLinkList *`, `AlacProgramStatusList *` | You free it | `alac_link_list_free`, `alac_program_status_list_free` |
| `AlacWriter *` | You free it, even after the terminal | `alac_writer_free` |
| `alac_version()`, `alac_last_error()` | Static, or the next error | — |

## Threading

A terminal and everything obtained from it must be used from one thread at a
time. Exceptions:

- The wakeup callback runs on the PTY thread, possibly while the terminal is
  locked. It must only schedule work on your thread. `alac_terminal_free` waits
  for a running callback, so `userdata` can be freed afterwards.
- `AlacWriter` may be used from any thread.
- Clipboard callbacks run on the thread calling `alac_terminal_poll_event` or a
  paste and copy function.

## Functions

### Library

| Function | Purpose |
| --- | --- |
| `alac_abi_version` | ABI version of the loaded library; compare with `ALAC_ABI_VERSION` |
| `alac_version` | Crate version string |
| `alac_last_error` | Last error message on this thread, or null |
| `alac_string_free` | Free a returned string |
| `alac_options_default` | Fill `AlacOptions` with defaults (80x24, 8x16 cells, 10,000 lines of scrollback) |

### Lifecycle and size

| Function | Purpose |
| --- | --- |
| `alac_terminal_spawn` | Start a program on a new PTY; null `AlacPtyOptions` runs the login shell |
| `alac_terminal_new_headless` | Terminal fed with `alac_terminal_feed`, emitting `ALAC_EVENT_OUTPUT` |
| `alac_terminal_free` | Destroy the terminal, asking a PTY child to shut down |
| `alac_terminal_is_headless`, `alac_terminal_child_pid` | Inspect the backend |
| `alac_terminal_set_options` | Change runtime options; size fields are ignored |
| `alac_terminal_resize` | Set columns, rows and cell size; informs the program |
| `alac_terminal_set_focused` | Focus reports, hollow cursor, clipboard permission |
| `alac_terminal_set_visible` | Notification policy for hidden terminals |
| `alac_terminal_mode` | `ALAC_MODE_*` flags the program enabled |

### Input

| Function | Purpose |
| --- | --- |
| `alac_terminal_key` | Encode and send a key event; drives vi mode while active |
| `alac_encode_key` | Encode a key for a mode without a terminal |
| `alac_terminal_text` | Send committed or typed text |
| `alac_terminal_paste` | Paste text, bracketed when enabled |
| `alac_terminal_write` | Send raw bytes |
| `alac_terminal_feed` | Headless only: parse bytes from the program |
| `alac_terminal_writer`, `alac_writer_write`, `alac_writer_free` | Thread-safe writing |
| `alac_terminal_mouse_button`, `_mouse_move`, `_mouse_wheel` | Mouse input; true when reported to the program |
| `alac_terminal_cell_at` | Pixel position to cell and side |

### Events and timers

| Function | Purpose |
| --- | --- |
| `alac_terminal_poll_event` | Next event; services protocol replies and clipboard requests |
| `alac_terminal_next_timeout_ms` | Milliseconds until `alac_terminal_tick` is due, or -1 |
| `alac_terminal_tick` | Run expired timers |
| `alac_terminal_notification_activated`, `_closed`, `alac_terminal_notifications_alive` | Report notification state |

| Event | Payload |
| --- | --- |
| `ALAC_EVENT_WAKEUP` | — |
| `ALAC_EVENT_TITLE` | `data`: title, or null to reset |
| `ALAC_EVENT_BELL` | — |
| `ALAC_EVENT_CURSOR_BLINKING_CHANGE` | — |
| `ALAC_EVENT_MOUSE_CURSOR_DIRTY` | — |
| `ALAC_EVENT_PROGRAM_STATUS_CHANGED` | Read `alac_terminal_program_statuses` |
| `ALAC_EVENT_PROGRESS_CHANGED` | `progress` |
| `ALAC_EVENT_NOTIFICATION_SHOW` | `serial`, `notification` |
| `ALAC_EVENT_NOTIFICATION_CLOSE` | `serial` |
| `ALAC_EVENT_NOTIFICATION_QUERY_ALIVE` | `data`: query to pass back |
| `ALAC_EVENT_OUTPUT` | `data`, `len`: bytes for the remote program |
| `ALAC_EVENT_CHILD_EXIT` | `exit_code`, or -1 when unknown |
| `ALAC_EVENT_EXIT` | — |
| `ALAC_EVENT_WORKING_DIRECTORY_CHANGED` | Read `alac_terminal_working_directory` |

### Rendering

| Function | Purpose |
| --- | --- |
| `alac_snapshot_new`, `alac_terminal_snapshot_update` | Reusable snapshot refreshed per frame, copying damaged rows |
| `alac_terminal_snapshot` | New snapshot of the current state |
| `alac_snapshot_info` | Cells, cursor, damage, images and colors |
| `alac_snapshot_cell_text` | UTF-8 text of one cell with combining characters |
| `alac_snapshot_hyperlink_uri`, `_id` | Hyperlink of `AlacCell.hyperlink` |
| `alac_snapshot_free` | Free a snapshot |

### Text, history and search

| Function | Purpose |
| --- | --- |
| `alac_terminal_screen_text` | Visible text, wrapped lines joined |
| `alac_terminal_text_range` | Text between two grid lines |
| `alac_terminal_history_size` | Lines in scrollback |
| `alac_terminal_scroll` | Scroll by `ALAC_SCROLL_*`; positive deltas move into history |
| `alac_terminal_jump_to_prompt` | Previous (negative) or next OSC 133 prompt |
| `alac_terminal_search`, `_search_clear` | Regex search that selects and reveals matches |

### Selection, clipboard and links

| Function | Purpose |
| --- | --- |
| `alac_terminal_select_start`, `_select_update` | Start and extend an `ALAC_SELECTION_*` selection |
| `alac_terminal_select_click` | Cell, word or line selection by click count |
| `alac_terminal_select_all`, `_select_clear` | Select everything, clear |
| `alac_terminal_selection_text` | Selected text |
| `alac_terminal_set_clipboard` | Install `AlacClipboardCallbacks` |
| `alac_clipboard_sink_add_format`, `_set_data` | Answer clipboard callbacks |
| `alac_terminal_copy_selection`, `_paste_clipboard` | User copy and paste |
| `alac_terminal_link_at` | URL or OSC 8 hyperlink at a cell |
| `alac_terminal_visible_links`, `alac_link_list_*` | All visible links |
| `alac_terminal_set_url_pattern` | URL regex; null restores the default, `""` disables |

### Vi mode

| Function | Purpose |
| --- | --- |
| `alac_terminal_vi_toggle` | Enter or leave vi mode |
| `alac_terminal_vi_motion` | Move by an `ALAC_VI_*` motion |
| `alac_terminal_vi_select` | Toggle a selection at the vi cursor |

### Colors

| Function | Purpose |
| --- | --- |
| `alac_terminal_set_primary_colors` | Foreground and background with derived variants |
| `alac_terminal_set_colors`, `_set_color` | Palette entries, `ALAC_COLOR_COUNT` in total |
| `alac_terminal_color` | Effective color including program overrides |

### Protocol state

| Function | Protocol |
| --- | --- |
| `alac_terminal_title` | Window title (OSC 0/2) |
| `alac_terminal_working_directory` | OSC 7 path and host |
| `alac_terminal_shell_state`, `_last_command`, `_last_command_output` | OSC 133 shell integration |
| `alac_terminal_progress` | OSC 9;4 progress |
| `alac_terminal_program_statuses`, `alac_program_status_list_*` | OSC 7501 program status |

## Protocol details

The engine implements the same protocols as Alacritty. Their behavior is
documented in [protocols/](../protocols/):
[shell integration](../protocols/shell-integration.md),
[Kitty graphics](../protocols/kitty-graphics.md),
[Kitty clipboard](../protocols/kitty-clipboard.md),
[notifications](../protocols/notifications.md),
[progress](../protocols/progress.md) and
[program status](../protocols/program-status.md).

## Compatibility

`ALAC_ABI_VERSION` and the shared library's soname change on incompatible ABI
changes: removed or reordered struct fields, changed signatures or changed
constant values. Adding functions, enum values and events keeps the version, so
ignore event kinds you don't know.
