#!/usr/bin/env bash
set -euo pipefail

source_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"

install -Dm755 "$source_dir/table-scale" "$prefix/bin/table-scale"
install -Dm644 "$source_dir/assets/icon.png" \
    "$prefix/share/icons/hicolor/512x512/apps/table-scale.png"
install -Dm644 "$source_dir/table-scale.desktop" \
    "$prefix/share/applications/table-scale.desktop"
install -Dm644 "$source_dir/table-scale.appdata.xml" \
    "$prefix/share/metainfo/table-scale.appdata.xml"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" >/dev/null 2>&1 || true
fi

echo "Installed TableScale to $prefix."
echo "Make sure $prefix/bin is on your PATH."
