#!/bin/sh
# dist/Kairo-<version>-linux-<arch>.tar.gz: Kairo's own launcher (window, tray,
# guardian) in release mode, its .desktop entry, hicolor icons and an install
# script for ~/.local (or a prefix). The core is not included: Kairo downloads
# it on first run.
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
name="Kairo-$version-linux-$arch"
out="dist/$name"
rm -rf "$out" "dist/$name.tar.gz"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/metainfo"

cargo build --release
cp target/release/kairo "$out/bin/kairo"

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
install -Dm755 "$here/bin/kairo" "$prefix/bin/kairo"
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
rm -f "$prefix/bin/kairo" "$prefix/share/applications/dev.kairo.app.desktop" \
  "$prefix/share/metainfo/dev.kairo.app.metainfo.xml" \
  "$prefix"/share/icons/hicolor/*/apps/dev.kairo.app.png "$prefix/share/icons/hicolor/scalable/apps/dev.kairo.app.svg"
echo "Kairo removed from $prefix"
EOF
chmod +x "$out/install.sh" "$out/uninstall.sh"
cp LICENSE README.md "$out/"

tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
