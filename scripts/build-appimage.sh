#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"
binary="${1:-target/release/table-scale}"
appdir="target/appimage/TableScale.AppDir"
output="dist/TableScale-${version}-x86_64.AppImage"

if [[ ! -x "$binary" ]]; then
    echo "Missing Linux binary: $binary" >&2
    echo "Build the portable binary first with scripts/build-linux-portable.sh" >&2
    exit 1
fi
if ! command -v appimagetool >/dev/null 2>&1; then
    echo "appimagetool is required to build the AppImage." >&2
    exit 1
fi

rm -rf "$appdir"
install -Dm755 "$binary" "$appdir/usr/bin/table-scale"
install -Dm644 assets/icon.png \
    "$appdir/usr/share/icons/hicolor/512x512/apps/table-scale.png"
install -Dm644 packaging/table-scale.desktop \
    "$appdir/usr/share/applications/table-scale.desktop"
install -Dm644 packaging/table-scale.appdata.xml \
    "$appdir/usr/share/metainfo/table-scale.appdata.xml"
install -Dm644 packaging/table-scale.desktop "$appdir/table-scale.desktop"
install -Dm644 assets/icon.png "$appdir/table-scale.png"
ln -s table-scale.png "$appdir/.DirIcon"

cat > "$appdir/AppRun" <<'EOF'
#!/bin/sh
HERE="$(cd "$(dirname "$0")" && pwd)"
exec "$HERE/usr/bin/table-scale" "$@"
EOF
chmod 755 "$appdir/AppRun"

mkdir -p dist
ARCH=x86_64 appimagetool --no-appstream "$appdir" "$output"
chmod 755 "$output"
echo "AppImage: $output"
