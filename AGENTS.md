# Repository Guidelines

## Project structure and module organization

This Rust 2024 workspace requires Rust 1.99 or newer and pins 1.99.0 in `rust-toolchain.toml`:

- `crates/alacritty/`: desktop application, commands, input, pane layout and native OS integration.
- `crates/terminal/`: terminal state, graphics and protocols; `crates/session/`: PTY processes and worker I/O.
- `crates/renderer/`: OpenGL drawing, glyphs and shaders. `crates/config/` and `crates/config-derive/`: configuration helpers and macros.
- `assets/`: icons and screenshots. `packaging/`: app templates, installers, completions and terminfo. `docs/` includes architecture, development, features, protocols and manpages.
- `vendor/vte/`: patched parser; consult `ALACRITTY-PATCH.md` before changing it.
- `scripts/release/`: packaging and updates; `scripts/smoke/` and `scripts/diagnostics/`: local checks.

## Build, test, and development commands

Run from the repository root; use `--workspace` to include every crate. Follow `docs/installation.md` for platform dependencies.

- `cargo build --locked --workspace`: build all workspace crates.
- `cargo run --locked -p alacritty`: launch the development terminal.
- `cargo test --locked --workspace`: run workspace unit and integration tests.
- `cargo test --locked -p vte`: test the vendored parser separately.
- `cargo test --locked -p alacritty_terminal --no-default-features`: check the minimal terminal configuration used in CI.
- `cargo clippy --workspace --all-targets`: run the workspace lint check.
- `make app`: build the macOS bundle under `target/release/osx/`; requires `scdoc` and platform tooling.

## Coding style and naming conventions

Use four-space indentation, `snake_case` functions/modules, `UpperCamelCase` types, and `SCREAMING_SNAKE_CASE` constants. Follow `rustfmt.toml`; use `cargo +nightly fmt --all` to apply its nightly-only options. End comments with periods. Preserve the minimum supported Rust version and existing platform feature gates.

## Testing guidelines

Use Rust's `#[test]` framework with descriptive `snake_case` names. Add regression tests that fail before the fix; no numeric coverage threshold is configured.

Reference fixtures live in `crates/terminal/tests/fixtures/`. Record with a release binary using `--ref-test`, then register the fixture in `crates/terminal/tests/reference.rs`. Follow `docs/protocols/kitty-graphics.md` and `docs/protocols/kitty-clipboard.md` for protocol and manual checks. Test release-script changes with `python3 -m unittest discover -s scripts/release -p 'test_*.py'`. Benchmark changes affecting throughput or latency.

## Commit and pull request guidelines

Recent commits use imperative subjects such as `Add Kitty clipboard protocol support`; follow that convention. Keep commits focused.

Describe the behavior change, link relevant issues, and report validation commands/results. Include screenshots for visual changes. Update `CHANGELOG.md` for user-visible changes, the terminal changelog for notable library changes, and `docs/man/` for configuration changes. For upstream submissions, observe the LLM contribution restriction in `CONTRIBUTING.md` and `.github/pull_request_template.md`.
