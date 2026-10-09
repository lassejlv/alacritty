# Development

Run commands from the repository root. Rustup selects the version in
`rust-toolchain.toml`; install platform dependencies from [installation.md](installation.md).

```sh
cargo run --locked -p alacritty
cargo build --locked --workspace
cargo test --locked --workspace
cargo test --locked -p vte
cargo test --locked -p alacritty_terminal --no-default-features
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo +nightly fmt --all -- --check
python3 -m unittest discover -s scripts/release -p 'test_*.py'
python3 scripts/smoke/shell-integration.py
```

The application is the default workspace member. `--workspace` also runs the
renderer, PTY session and configuration-library tests. Keep unit tests with their
module and integration tests in the owning package's `tests/` directory.

Terminal recordings live in `crates/terminal/tests/fixtures/`; register new ones
in `crates/terminal/tests/reference.rs`. Protocol smoke instructions are under
[protocols/](protocols/). Native clipboard tests use private pasteboards.

`make app` and `make app-universal` read the templates in `packaging/macos/` and
write bundles under `target/release/osx/`. Output paths and release asset names
are unchanged. See [releases.md](releases.md) for signing and publication.

See [architecture.md](architecture.md) before moving code between crates.
