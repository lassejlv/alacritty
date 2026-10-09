# Workspace architecture

The root `Cargo.toml` defines six packages, shared dependency versions, and the
Rust edition and minimum version. `cargo run` builds the `alacritty` application
by default. Use `--workspace` to check every package and its tests.

```text
crates/alacritty/        Desktop executable, input, windows, panes and OS integration
crates/terminal/      Terminal emulation, grid, selection, graphics and protocols
crates/session/       PTY processes, worker loop, synchronization and notifications
crates/renderer/      OpenGL rendering, glyph cache, drawing settings and shaders
crates/config/        Shared serialization and configuration replacement helpers
crates/config-derive/ Configuration procedural macros
assets/               Icons and screenshots
packaging/            macOS bundle, Windows installer, Linux desktop files,
                      shell completions and terminfo
scripts/release/      Signing, packaging and publication
scripts/diagnostics/  Color probes and performance tools
scripts/smoke/        Interactive protocol checks
vendor/vte/           Patched parser, with its upstream licenses and patch notes
```

## Crate boundaries

`alacritty_terminal` processes terminal input and stores screen/protocol state.
It does not own PTY workers, windows or native clipboard access. Protocol modules
live under `protocols/`; `clipboard` and `program_status` remain re-exported at the
crate root. Reference recordings live in `tests/fixtures/`.

`alacritty_session` depends on the terminal engine and owns process startup,
I/O, resizing and shutdown. A `Session` owns the shared terminal and the worker's
notification handle. Dropping it requests shutdown without blocking the UI.
The PTY worker retains the existing detached-thread cleanup behavior.

`alacritty_renderer` consumes prepared cells, image placements and rectangles.
It owns GL resources, context helpers, fonts and shaders. It depends on terminal
value types and the configuration helper crates for serialized drawing settings;
it never imports the desktop application or the session crate.

## Application modules

- `app/`: application lifecycle, events, command dispatch, action context and timers.
- `workspace/`: native window coordination, pane state and split-tree geometry.
- `input/`: keyboard encoding, mouse/touch handling and event routing.
- `presentation/`: prepared terminal content, colors, hints, search, messages and
  program-status title policy.
- `platform/`: native windows, process helpers and macOS/Windows/Unix integration.
- `clipboard/`: application clipboard access and Kitty protocol permission handling.
- `config/`: settings, bindings, reload monitoring and configuration migration.
- `ipc/`: Unix socket commands and I/O polling.

Keyboard shortcuts and native menus use the same command implementation.
Each pane owns one session, independent selection/search state, font zoom and
clipboard permissions. Each pane retains its font selection and metrics; the
window shares the rasterizer and GPU glyph storage and composes its panes.

## Library migration

The existing package names are unchanged. PTY consumers must replace
`alacritty_terminal::tty` with `alacritty_session::pty`. Worker message types
and `Notifier` now live in `alacritty_session::events`; the worker itself is
`alacritty_session::event_loop::EventLoop`. Synchronization and thread helpers
also moved to the session crate.

Hosts receiving `Event::ProgramStatusChanged` read `Term::program_status()` and
choose how to present it. Raw OSC window titles remain independent of status
records. The desktop app composes them in `presentation/status.rs`.
