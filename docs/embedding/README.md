# Embedding the Alacritty engine

`alacritty_ffi` exposes Alacritty's terminal emulator as a library with a C ABI.
It runs child processes on PTYs, parses their output, answers terminal protocol
queries and encodes keyboard and mouse input. You decide what to do with the
result: draw it in your own window, show it on a web page, or read it as text in
an agent.

| Guide | For |
| --- | --- |
| [agents.md](agents.md) | Running commands and interactive programs from code or LLM agents |
| [hosts.md](hosts.md) | Building a graphical terminal: rendering, input, clipboard, notifications |
| [c-api.md](c-api.md) | Every function, event and ownership rule in `alacritty.h` |

The crate lives in `crates/ffi/`. Its
[README](../../crates/ffi/README.md) covers the examples, and
[architecture.md](../architecture.md) explains how it relates to the other
crates.

## Building and linking

```sh
make ffi
export PKG_CONFIG_PATH=$PWD/target/release/alacritty_ffi/lib/pkgconfig
cc host.c $(pkg-config --cflags --libs alacritty_ffi) -o host
```

`make ffi` builds a release library and writes a relocatable directory under
`target/release/alacritty_ffi/` with `include/alacritty.h`, static and shared
libraries, and `lib/pkgconfig/alacritty_ffi.pc`. Copy it to any prefix. Use
`pkg-config --static` for static linking.

The shared library carries the ABI version in its name
(`libalacritty_ffi.so.1`, `@rpath/libalacritty_ffi.1.dylib`), so an incompatible
library is never loaded by accident. Programs linked against the raw
`target/release` output of `cargo build` can't find that name at runtime; link
against the packaged directory instead. Hosts that load the library dynamically
should still check `alac_abi_version() == ALAC_ABI_VERSION`.

Language bindings:

- **C and C++** include `alacritty.h`.
- **Rust** depends on the crate and uses `alacritty_ffi::Engine`, the same type
  the C ABI wraps.
- **Python** uses `crates/ffi/bindings/python/alacritty_ffi.py`, a ctypes binding
  without dependencies, plus `agent.py` for agent workflows.
- **Other languages** generate bindings from `alacritty.h`. It contains only C
  types, enums and opaque handles.

## Concepts

**Terminal modes.** `alac_terminal_spawn` starts a program on a new PTY; the
engine reads its output on a background thread. `alac_terminal_new_headless`
creates a terminal without a process: you feed it bytes from anywhere (SSH, a
container, a recording) and send its `ALAC_EVENT_OUTPUT` bytes back.

**Event loop.** The engine calls your wakeup callback, from any thread, when
something happened. On your own thread, call `alac_terminal_poll_event` until it
returns false. Polling is also when the engine answers protocol requests from
the program, so a host that stops polling stalls programs that wait for replies.
Call `alac_terminal_tick` when `alac_terminal_next_timeout_ms` elapses, for
notification expiry, progress timeouts, image animation and synchronized updates.

```c
static void wakeup(void *window) { post_to_main_thread(window); } /* Never call the library here. */

/* On the main thread: */
AlacEvent event;
while (alac_terminal_poll_event(term, &event)) handle(&event);
set_timer(alac_terminal_next_timeout_ms(term));   /* -1 means no timer. */
```

**Coordinates.** Input and selection functions take viewport cells: column 0 to
`columns - 1` and row 0 at the top of the visible area. Text ranges and search
results use grid lines, where 0 is the top of the screen, positive lines go down
and negative lines are scrollback. When the viewport is scrolled back by
`display_offset` lines, viewport row `r` is grid line `r - display_offset`.

**Threads.** Use a terminal handle from one thread at a time. The wakeup
callback is the only thing the engine calls from other threads. To write to the
child from another thread, create an `AlacWriter` with `alac_terminal_writer`.

**Memory.** Returned `char *` strings belong to you; free them with
`alac_string_free`. Snapshots, lists and writers have their own `_free`
functions. Pointers inside an event are valid until the next poll; pointers from
`alac_snapshot_info` are valid until the snapshot is updated or freed.

**Errors.** Functions return null, false or -1 on failure and set a message for
`alac_last_error`, which is per thread. Panics inside the library are caught and
reported the same way; they never unwind into your code.

## Rust

```rust
use alacritty_ffi::{Engine, HostEvent, Options, PtyOptions};

let pty = PtyOptions { program: Some("htop".into()), ..PtyOptions::default() };
let mut engine = Engine::spawn(Options::default(), pty, Some(Box::new(|| wake_ui())))?;

while let Some(event) = engine.poll() {
    match event {
        HostEvent::Wakeup => request_redraw(),
        HostEvent::Title(title) => set_title(title.as_deref().unwrap_or("Terminal")),
        HostEvent::ChildExit(code) => close(code),
        _ => (),
    }
}

let mut frame = alacritty_ffi::snapshot::Snapshot::default();
engine.update_snapshot(&mut frame);
```

`Engine::terminal()` gives access to the underlying `alacritty_terminal::Term` for
anything the engine doesn't wrap. Don't hold its lock across engine calls.

## Validation

```sh
cargo test --locked -p alacritty_ffi
cargo run --release -p alacritty_ffi --example snapshot_bench
python3 -m unittest discover -s scripts/release -p 'test_*.py'
```

The C ABI tests compile `alacritty.h` against the Rust layouts, run the C
examples, check the Python binding against the header and run the agent helpers
against real programs. The SDL example is compiled and run with
`SDL_VIDEODRIVER=dummy` when `pkg-config` finds SDL2 and SDL2_ttf.
