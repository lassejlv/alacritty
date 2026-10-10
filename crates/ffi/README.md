# alacritty_ffi

Build your own terminal on the Alacritty engine. This crate wraps terminal emulation, PTY
processes, keyboard and mouse encoding, selection, links, vi mode and protocol handling behind a
C ABI. Your host provides the window, fonts, drawing and native clipboard.

Guides live in [docs/embedding](../../docs/embedding/README.md): driving programs from agents,
building graphical hosts, and the C API reference.

## Building and linking

```sh
make ffi
```

This builds a release library and lays it out under `target/release/alacritty_ffi/`:

```
include/alacritty.h
lib/libalacritty_ffi.a
lib/libalacritty_ffi.1.dylib      # libalacritty_ffi.so.1 on Linux, bin/alacritty_ffi.dll on Windows
lib/libalacritty_ffi.dylib        # development symlink
lib/pkgconfig/alacritty_ffi.pc
```

The directory is relocatable; copy it to a prefix or point `PKG_CONFIG_PATH` at it:

```sh
export PKG_CONFIG_PATH=target/release/alacritty_ffi/lib/pkgconfig
cc host.c $(pkg-config --cflags --libs alacritty_ffi) -o host
```

The shared library's soname (`libalacritty_ffi.so.1`) or install name
(`@rpath/libalacritty_ffi.1.dylib`) carries `ALAC_ABI_VERSION`, so programs only load a
compatible library. Linking directly against `target/release` from `cargo build` also works,
but the program then needs the versioned name next to the library at runtime; use the packaged
layout instead. For static linking add `pkg-config --static --libs alacritty_ffi` flags.

Rust hosts can depend on the crate and use `alacritty_ffi::Engine` directly.

## Host loop

```c
AlacOptions options;
alac_options_default(&options);
options.columns = 120;
options.rows = 40;
options.cell_width = 9;
options.cell_height = 18;

AlacTerminal *term = alac_terminal_spawn(&options, NULL, wake_main_thread, my_window);
AlacSnapshot *frame = alac_snapshot_new();

/* On the main thread, after wake_main_thread() scheduled it: */
AlacEvent event;
while (alac_terminal_poll_event(term, &event)) {
    switch (event.kind) {
        case ALAC_EVENT_WAKEUP: request_redraw(); break;
        case ALAC_EVENT_TITLE: set_title(event.data ? (const char *)event.data : "Terminal"); break;
        case ALAC_EVENT_CHILD_EXIT: close_window(); break;
    }
}
schedule_timer(alac_terminal_next_timeout_ms(term), /* then */ alac_terminal_tick);

/* When drawing: only damaged rows are copied into the reused snapshot. */
alac_terminal_snapshot_update(term, frame);
AlacSnapshotInfo info;
alac_snapshot_info(frame, &info);
for (uint32_t row = 0; row < info.rows; row++) {
    for (uint32_t column = 0; column < info.columns; column++) {
        const AlacCell *cell = &info.cells[row * info.columns + column];
        draw_cell(column, row, cell->codepoint, cell->fg, cell->bg, cell->flags);
    }
}
for (size_t i = 0; i < info.image_count; i++) draw_image(&info.images[i]);
draw_cursor(&info.cursor); /* Draw the glyph under a block cursor in cursor.text_color. */
```

Hosts that redraw only what changed can use `info.full_damage` and `info.damage`.

Forward input with `alac_terminal_key`, `alac_terminal_text` (IME commits),
`alac_terminal_paste`, `alac_terminal_mouse_button`, `alac_terminal_mouse_move` and
`alac_terminal_mouse_wheel`. Set `AlacKeyEvent.alt_composes` when Option/Alt produced the text,
like macOS Option without "Option as Alt". Call `alac_terminal_resize` and
`alac_terminal_set_focused` when the window changes.

When the mouse isn't reported to the application, use `alac_terminal_cell_at` to convert pixels
to cells, `alac_terminal_select_click` with the click count for cell, word and line selection,
`alac_terminal_select_update` while dragging and `alac_terminal_copy_selection` to copy.
`alac_terminal_link_at` returns the OSC 8 hyperlink or detected URL under the pointer, and
`alac_terminal_visible_links` lists them for keyboard hints.

`alac_terminal_vi_toggle` enters vi mode. While active, `alac_terminal_key` applies the default
vi bindings (hjkl, w/b/e, 0/$/^, g/G, Ctrl-u/d/b/f, v/V/Ctrl-v, y, n/N, i to leave), the
snapshot cursor is the vi cursor and keys are never sent to the child. Keys it doesn't use, like
`/`, return false so the host can show search UI and call `alac_terminal_search`.

## Examples

- `examples/headless.c`: protocol replies, clipboard, keys, snapshots, links, vi mode and writers.
- `examples/pty.c`: run a command and print its screen.
- `examples/sdl_host.c`: a complete graphical terminal using SDL2 and SDL2_ttf.

```sh
cc crates/ffi/examples/sdl_host.c \
    $(pkg-config --cflags --libs sdl2 SDL2_ttf alacritty_ffi) -o sdl_host
./sdl_host --font /path/to/mono.ttf
```

- `examples/snapshot_bench.rs`: compares fresh and incremental snapshots
  (`cargo run --release -p alacritty_ffi --example snapshot_bench`).

## Headless terminals and agents

`alac_terminal_new_headless` creates a terminal without a PTY. Feed it the child's output with
`alac_terminal_feed`; bytes for the child, including key input and protocol replies, arrive as
`ALAC_EVENT_OUTPUT` events. Use it for SSH clients, web terminals, tests and agents.

Agents can drive terminals without rendering: `alac_terminal_screen_text`,
`alac_terminal_text_range`, `alac_terminal_search`, `alac_terminal_title`,
`alac_terminal_shell_state`, `alac_terminal_last_command` and
`alac_terminal_last_command_output` read state, and `alac_encode_key` encodes keys without a
terminal. `alac_terminal_writer` returns a handle for writing to the child from any thread.

### Python

`bindings/python/alacritty_ffi.py` is a dependency-free ctypes binding:

```python
from alacritty_ffi import Terminal

with Terminal.spawn(["/bin/sh", "-c", "make test"]) as term:
    term.wait_for(lambda term: not term.running, timeout=60)
    print(term.screen_text(), term.exit_code)
```

It finds the library through `ALACRITTY_FFI_LIBRARY`, the system library path, or this
repository's `target` directory. See `bindings/python/example.py`, and `bindings/python/agent.py`
for command, shell-session and full-screen helpers described in
[docs/embedding/agents.md](../../docs/embedding/agents.md).

## Protocols

| Protocol | Engine handles | Host provides |
| --- | --- | --- |
| VT/xterm, truecolor, SGR attributes, hyperlinks | Parsing, grid, colors, OSC 4/10/11/12 replies | Glyph drawing |
| Kitty keyboard | Encoding for all enhancement flags | Key events |
| Kitty graphics | Storage, animation, placements, Unicode placeholders, clipping | Drawing RGBA rectangles |
| OSC 52 and Kitty clipboard (OSC 5522) | Parsing, chunking, passwords, permission grants, paste events | Clipboard callbacks |
| OSC 9 and OSC 99 notifications | Assembly, updates, replies, expiry | Showing notifications and reporting activation |
| OSC 9;4 progress | State and the 15 second timeout | Presentation |
| OSC 7 working directory | URI validation, change events | Choosing whether a host is local |
| OSC 133 shell integration | Prompt marks, commands, exit status, output ranges | Navigation and UI |
| OSC 7501 program status | Records and app inheritance | Presentation |
| Mouse reporting, focus events, bracketed paste, synchronized updates | Encoding and timing | Input events |

Clipboard requests from applications respect `AlacOptions.osc52` and require focus; user
actions like `alac_terminal_copy_selection` and `alac_terminal_paste_clipboard` don't.

## Rules

- Use a terminal from one thread at a time; use an `AlacWriter` to send input from others. The
  wakeup callback runs on any thread, possibly while the terminal is locked; it must only
  schedule polling and never call the library.
- Drain events until `alac_terminal_poll_event` returns false; replies to the application are
  sent while polling.
- Free returned strings with `alac_string_free`, snapshots with `alac_snapshot_free`, lists with
  their `_free` function and writers with `alac_writer_free`. Event pointers are valid until the
  next poll; snapshot pointers until the snapshot is updated or freed.
- Damage is relative to the previous snapshot of the terminal. A reused snapshot that missed
  updates reports full damage.
- Check `alac_abi_version() == ALAC_ABI_VERSION` when loading the library dynamically.
