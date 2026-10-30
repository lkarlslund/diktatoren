#!/usr/bin/env bash
# Installs Diktatoren from an extracted release archive into ~/.local (or PREFIX).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
dest="$prefix/lib/diktatoren"
id=io.github.lkarlslund.Diktatoren
mkdir -p "$dest" "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/scalable/apps"
cp -r "$here/diktatoren" "$here"/libaudiocpp.so* "$here/models" "$dest/"
ln -sf "$dest/diktatoren" "$prefix/bin/diktatoren"
sed "s|^Exec=diktatoren|Exec=$dest/diktatoren|" "$here/$id.desktop" > "$prefix/share/applications/$id.desktop"
cp "$here/$id.svg" "$prefix/share/icons/hicolor/scalable/apps/$id.svg"
echo "Installed to $dest. Start Diktatoren from the application menu or run: diktatoren"
echo "To start it at login: cp $prefix/share/applications/$id.desktop ~/.config/autostart/"
