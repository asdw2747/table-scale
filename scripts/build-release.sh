#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"
target_windows="x86_64-pc-windows-msvc"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
./scripts/build-linux-portable.sh
./scripts/build-appimage.sh
cargo xwin build --release --target "$target_windows"

mkdir -p dist
linux_dir="$stage/tablescale-${version}-linux-x86_64"
windows_dir="$stage/tablescale-${version}-windows-x86_64"
mkdir -p "$linux_dir" "$windows_dir"

cp target/release/table-scale "$linux_dir/"
cp LICENSE README.md THIRD_PARTY_NOTICES.md "$linux_dir/"
mkdir -p "$linux_dir/assets"
cp assets/icon.png "$linux_dir/assets/"
cp packaging/table-scale.desktop "$linux_dir/"
cp packaging/table-scale.appdata.xml "$linux_dir/"
cp packaging/install-linux.sh "$linux_dir/"

cp "target/${target_windows}/release/table-scale.exe" "$windows_dir/"
cp LICENSE README.md THIRD_PARTY_NOTICES.md "$windows_dir/"
cp assets/icon.png "$windows_dir/"

tar -C "$linux_dir" -czf "dist/TableScale-${version}-linux-x86_64.tar.gz" .
zip -q -j "dist/TableScale-${version}-windows-x86_64.zip" \
    "$windows_dir/table-scale.exe" "$windows_dir/LICENSE" \
    "$windows_dir/README.md" "$windows_dir/THIRD_PARTY_NOTICES.md" "$windows_dir/icon.png"

echo "Release archives written to dist/"
ls -lh dist/
