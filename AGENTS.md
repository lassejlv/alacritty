# Repository Guidelines

## Project structure and module organization

This Rust 2024 workspace requires Rust 1.99 or newer and pins 1.99.0 in `rust-toolchain.toml`:

- `alacritty/`: application, OpenGL renderer, input, configuration, and native macOS integration.
- `alacritty_terminal/`: terminal state, PTY handling, Kitty protocols, and integration tests.
- `alacritty_config/` and `alacritty_config_derive/`: configuration utilities and procedural macros.
- `vendor/vte/`: patched parser; consult `ALACRITTY-PATCH.md` before changing it.
- `extra/`: icons, completions, terminfo, man pages, and app templates. `docs/` documents fork features; `scripts/release/` contains packaging and update tooling.

## Build, test, and development commands

Run from the repository root. Follow `INSTALL.md` for platform dependencies.

- `cargo build --locked --workspace`: build all workspace crates.
- `cargo run --locked -p alacritty`: launch the development terminal.
- `cargo test --locked --workspace`: run workspace unit and integration tests.
- `cargo test --locked -p vte`: test the vendored parser separately.
- `cargo test --locked -p alacritty_terminal --no-default-features`: check the minimal terminal configuration used in CI.
- `cargo clippy --all-targets`: run the CI lint check.
- `make app`: build the macOS bundle under `target/release/osx/`; requires `scdoc` and platform tooling.

## Coding style and naming conventions

Use four-space indentation, `snake_case` functions/modules, `UpperCamelCase` types, and `SCREAMING_SNAKE_CASE` constants. Follow `rustfmt.toml`; use `cargo +nightly fmt --all` to apply its nightly-only options. End comments with periods. Preserve the minimum supported Rust version and existing platform feature gates.

## Testing guidelines

Use Rust's `#[test]` framework with descriptive `snake_case` names. Add regression tests that fail before the fix; no numeric coverage threshold is configured.

Reference fixtures live in `alacritty_terminal/tests/ref/`. Record with a release binary using `--ref-test`, then register the fixture in `alacritty_terminal/tests/ref.rs`. Follow `docs/kitty-graphics.md` and `docs/kitty-clipboard.md` for protocol and manual checks. Test release-script changes with `python3 -m unittest discover -s scripts/release -p 'test_*.py'`. Benchmark changes affecting throughput or latency.

## Commit and pull request guidelines

Recent commits use imperative subjects such as `Add Kitty clipboard protocol support`; follow that convention. Keep commits focused.

Describe the behavior change, link relevant issues, and report validation commands/results. Include screenshots for visual changes. Update `CHANGELOG.md` for user-visible changes, the terminal changelog for notable library changes, and `extra/man/` for configuration changes. For upstream submissions, observe the LLM contribution restriction in `CONTRIBUTING.md` and `.github/pull_request_template.md`.
