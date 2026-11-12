#!/bin/sh
# dist/Kairo-<version>-linux-<arch>.tar.gz: Kairo's own launcher (window, tray,
# guardian) in release mode, the pinned mihomo core (scripts/mihomo.env,
# checksum verified) in lib/kairo, its .desktop entry, hicolor icons, the
# licences and an install script for ~/.local (or a prefix).
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
name="Kairo-$version-linux-$arch"
out="dist/$name"
rm -rf "$out" "dist/$name.tar.gz"
mkdir -p "$out/bin" "$out/lib/kairo" "$out/share/applications" "$out/share/metainfo" "$out/share/doc/kairo"

cargo build --release
cp target/release/kairo "$out/bin/kairo"

# The core, and its licence and source.
scripts/fetch-mihomo.sh "linux-$arch" "$out/lib/kairo"
mv "$out/lib/kairo/LICENSE.mihomo" "$out/share/doc/kairo/LICENSE.mihomo"
. scripts/mihomo.env
"$out/lib/kairo/mihomo" -v | grep -q "Mihomo Meta $MIHOMO_VERSION "
cat > "$out/share/doc/kairo/NOTICE" <<NOTICE
Kairo $version: https://github.com/daoleno/kairo
Free software under the GNU GPL, version 3 or later (LICENSE).

It ships the mihomo core $MIHOMO_VERSION (lib/kairo/mihomo), the official
MetaCubeX build, unmodified. mihomo is free software under the GNU GPL,
version 3 (LICENSE.mihomo). Its source code:
https://github.com/MetaCubeX/mihomo/tree/$MIHOMO_VERSION
NOTICE
cp LICENSE "$out/share/doc/kairo/LICENSE"

for size in 16 24 32 48 64 128 256 512; do
  dir="$out/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$dir"
  rsvg-convert -w $size -h $size assets/icon.svg -o "$dir/dev.kairo.app.png"
done
mkdir -p "$out/share/icons/hicolor/scalable/apps"
cp assets/icon.svg "$out/share/icons/hicolor/scalable/apps/dev.kairo.app.svg"

cat > "$out/share/applications/dev.kairo.app.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kairo
GenericName=Proxy Client
Comment=A native client for the mihomo proxy core
Exec=kairo
Icon=dev.kairo.app
StartupWMClass=dev.kairo.app
Terminal=false
Categories=Network;
Keywords=proxy;mihomo;clash;vpn;tun;
SingleMainWindow=true
EOF

cat > "$out/share/metainfo/dev.kairo.app.metainfo.xml" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>dev.kairo.app</id>
  <name>Kairo</name>
  <summary>A native client for the mihomo proxy core</summary>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>GPL-3.0-or-later</project_license>
  <launchable type="desktop-id">dev.kairo.app.desktop</launchable>
  <releases><release version="$version"/></releases>
</component>
EOF

cat > "$out/install.sh" <<'EOF'
#!/bin/sh
# ./install.sh [prefix]   (default: ~/.local; /usr/local needs sudo)
set -e
prefix=${1:-$HOME/.local}
here=$(cd "$(dirname "$0")" && pwd)
# Another mihomo in the same bin (the machine's own) is left alone and never used.
if [ -e "$prefix/bin/mihomo" ]; then
  echo "Note: $prefix/bin/mihomo is there too. Kairo does not use it: its core is $prefix/lib/kairo/mihomo."
fi
install -Dm755 "$here/bin/kairo" "$prefix/bin/kairo"
install -Dm755 "$here/lib/kairo/mihomo" "$prefix/lib/kairo/mihomo"
(cd "$here/share" && find . -type f) | while read -r f; do
  install -Dm644 "$here/share/$f" "$prefix/share/$f"
done
# The launcher entry runs `kairo`; point it at this prefix if that is not on PATH.
sed -i "s|^Exec=kairo|Exec=$prefix/bin/kairo|" "$prefix/share/applications/dev.kairo.app.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "Kairo installed in $prefix (run: kairo)"
EOF
cat > "$out/uninstall.sh" <<'EOF'
#!/bin/sh
# ./uninstall.sh [prefix]   Removes the app; your data (~/.local/share/kairo) stays.
prefix=${1:-$HOME/.local}
rm -rf "$prefix/lib/kairo" "$prefix/share/doc/kairo"
rm -f "$prefix/bin/kairo" "$prefix/share/applications/dev.kairo.app.desktop" \
  "$prefix/share/metainfo/dev.kairo.app.metainfo.xml" \
  "$prefix"/share/icons/hicolor/*/apps/dev.kairo.app.png "$prefix/share/icons/hicolor/scalable/apps/dev.kairo.app.svg"
echo "Kairo removed from $prefix"
EOF
chmod +x "$out/install.sh" "$out/uninstall.sh"
cp LICENSE README.md "$out/"

tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
