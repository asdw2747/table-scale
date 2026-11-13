#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

target="x86_64-unknown-linux-gnu"
glibc="2.35"

if ! command -v cargo-zigbuild >/dev/null 2>&1; then
    echo "cargo-zigbuild is required. Install it with: cargo install cargo-zigbuild --locked" >&2
    exit 1
fi
if ! command -v zig >/dev/null 2>&1; then
    echo "zig is required and must be on PATH (Zig 0.14.x is recommended)." >&2
    exit 1
fi

echo "Building Linux release against glibc ${glibc}..."
cargo zigbuild --release --target "${target}.${glibc}"
cp "target/${target}/release/table-scale" "target/release/table-scale"
echo "Portable Linux binary: target/release/table-scale (requires glibc >= ${glibc})"
