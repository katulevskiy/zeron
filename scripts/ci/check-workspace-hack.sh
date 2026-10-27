#!/usr/bin/env bash
# The workspace-hack crate (crates/workspace-hack, configured in .config/hakari.toml)
# must match the dependency graph: it pins one feature set per third-party dependency,
# so builds of different packages (CI's jobs, `cargo test -p` then `cargo build -p`)
# reuse each other's compiled dependencies instead of rebuilding them.
#
# After changing dependencies:  cargo hakari generate && cargo hakari manage-deps
# (cargo install cargo-hakari --locked)
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=0.9.39
SHA256=699a3193a9bd1a8d914b970d3f50f3385f24fbd8b1294d0c0ff31fbe7c2c9f48
if ! command -v cargo-hakari >/dev/null; then
  [ "$(uname -m)" = x86_64 ] || { echo "check-workspace-hack.sh: x86_64 Linux only" >&2; exit 1; }
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  curl --proto '=https' --tlsv1.2 -fsSL --retry 5 -o "$tmp/hakari.tar.gz" \
    "https://github.com/guppy-rs/guppy/releases/download/cargo-hakari-$VERSION/cargo-hakari-$VERSION-x86_64-unknown-linux-gnu.tar.gz"
  echo "$SHA256  $tmp/hakari.tar.gz" | sha256sum -c -
  mkdir -p "$HOME/.cargo/bin"
  tar -xzf "$tmp/hakari.tar.gz" -C "$HOME/.cargo/bin" cargo-hakari
fi

status=0
cargo hakari generate --diff || { echo "::error::crates/workspace-hack is out of date: run cargo hakari generate"; status=1; }
cargo hakari manage-deps --dry-run || { echo "::error::a crate lacks the workspace-hack dependency: run cargo hakari manage-deps"; status=1; }
exit "$status"
