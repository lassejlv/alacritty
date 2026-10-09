# Changelog

All notable changes to alacritty_terminal are documented in this file. The
sections should follow the order `Added`, `Changed`, `Deprecated`, `Fixed` and
`Removed`.

**Breaking changes are written in bold style.**

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## 0.26.1-dev

### Added

- OSC 7501 program status records, feature detection, and lifecycle handling

### Changed

- **Move `event_loop`, `tty`, `sync`, and `thread` to `alacritty_session`; `tty` is now `pty`**
- Emit `ProgramStatusChanged` events; window-title formatting is owned by the application

- Minimum Rust version has been bumped to 1.99.0

### Fixed

- Panic when the PTY could not be set to non-blocking
- Off-by-one in ViMotion::ParagraphUp
- Unbounded per-cell memory usage for zero-width cells
- Unsoundness in `Row::new` when `columns == 0`

## 0.26.0

### Added

- New `escape_args` field on `tty::Options` for Windows shell argument escaping control

### Changed

- Pass `-q` to `login` on macOS if `~/.hushlogin` is present
- **`ChildEvent::Exited` and `Event::ChildExit` now contain `ExitStatus` instead of `i32`**

## 0.25.0

### Changed

- Replaced `Options::hold` with `Options::drain_on_exit`

## 0.24.2

### Added

- Escape sequence to move cursor forward tabs ( CSI Ps I )

## 0.24.1

### Changed

- Shell RCs are no longer sourced on macOs

### Fixed

- Semantic search handling of fullwidth characters
- Inline search ignoring line wrapping flag
- Clearing of `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` in the main process
- FD leaks when closing PTYs on Unix
- Crash when ConPTY creation failed

## 0.24.0

### Added

- `tty::unix::from_fd()` to create a TTY from a pre-opened PTY's file-descriptors

### Changed

- **`Term` is not focused by default anymore**
