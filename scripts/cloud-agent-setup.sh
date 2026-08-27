#!/usr/bin/env bash
#
# Idempotent setup for building Alacritty in a Cursor Cloud Agent.
#
# Installs the system libraries required to build the GPU-accelerated
# terminal emulator, ensures a Rust toolchain new enough for the crate's
# `edition = "2024"` / `rust-version` requirement, and compiles the release
# binary. Safe to re-run: apt install and rustup are convergent, and cargo
# reuses its build cache.
set -euo pipefail

echo "==> Installing system build & runtime dependencies"
export DEBIAN_FRONTEND=noninteractive
sudo apt-get update -qq
# Build deps (per INSTALL.md) plus the Mesa/OpenGL runtime needed to actually
# render the window (software rendering via llvmpipe on a headless display).
sudo apt-get install -y --no-install-recommends \
  cmake \
  g++ \
  pkg-config \
  python3 \
  libfontconfig1-dev \
  libxcb-xfixes0-dev \
  libxkbcommon-dev \
  libgl1-mesa-dri \
  libegl1-mesa-dev \
  libgl1 \
  mesa-utils \
  libxkbcommon-x11-0 \
  fonts-dejavu-core

echo "==> Ensuring Rust toolchain satisfies the workspace rust-version"
# The workspace pins `rust-version` (currently 1.85.0) and uses edition 2024,
# so make sure a recent stable toolchain is installed and the default.
required="$(grep -m1 'rust-version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
echo "    workspace rust-version = ${required}"
if command -v rustup >/dev/null 2>&1; then
  rustup toolchain install stable --profile minimal --no-self-update
  rustup default stable
else
  echo "    rustup not found; relying on the system Rust toolchain" >&2
fi
rustc --version
cargo --version

echo "==> Building Alacritty (release)"
cargo build --release

echo "==> Done. Binary at: $(pwd)/target/release/alacritty"
