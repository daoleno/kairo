#!/bin/sh
# dist/kairo_<version>_<arch>.deb from the tree scripts/package-linux.sh made
# (run that first). Built with ar and tar, so it needs no dpkg.
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) arch=$(uname -m) ;;
esac
tree="dist/Kairo-$version-linux-$(uname -m)"
[ -x "$tree/bin/kairo" ] || { echo "run scripts/package-linux.sh first" >&2; exit 1; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

root="$work/root"
mkdir -p "$root/usr/bin" "$root/usr/lib/kairo" "$root/usr/share/doc/kairo"
cp "$tree/bin/kairo" "$root/usr/bin/kairo"
cp "$tree/lib/kairo/mihomo" "$root/usr/lib/kairo/mihomo"
cp -R "$tree/share/." "$root/usr/share/"
rm -f "$root/usr/share/doc/kairo/LICENSE"
. scripts/mihomo.env
{
  echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
  echo "Upstream-Name: kairo"
  echo "Source: https://github.com/daoleno/kairo"
  echo
  echo "Files: *"
  echo "Copyright: daoleno"
  echo "License: GPL-3.0-or-later"
  echo
  echo "Files: usr/lib/kairo/mihomo"
  echo "Copyright: MetaCubeX"
  echo "License: GPL-3.0"
  echo "Comment: mihomo $MIHOMO_VERSION, the official build, unmodified."
  echo " Source: https://github.com/MetaCubeX/mihomo/tree/$MIHOMO_VERSION"
  echo " Licence text: the GNU GPL version 3, as below; also in"
  echo " /usr/share/doc/kairo/LICENSE.mihomo (mihomo's own copy)."
  echo
  echo "License: GPL-3.0-or-later"
  sed 's/^$/./; s/^/ /' LICENSE
} > "$root/usr/share/doc/kairo/copyright"
size=$(du -sk "$root" | cut -f1)

mkdir -p "$work/control"
cat > "$work/control/control" <<CONTROL
Package: kairo
Version: $version
Architecture: $arch
Maintainer: daoleno <7393152+daoleno@users.noreply.github.com>
Installed-Size: $size
Depends: libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libwebkitgtk-6.0-4, libc6 (>= 2.39)
Recommends: pkexec | policykit-1
Section: net
Priority: optional
Homepage: https://github.com/daoleno/kairo
Description: Native client for the mihomo proxy core
 Kairo runs the mihomo (Clash Meta) core it ships with: subscriptions, node
 selection and delay tests, live traffic, connections, rules and logs, the
 system proxy and TUN mode. It updates itself, core included.
CONTROL
(cd "$root" && find . -type f ! -path './DEBIAN/*' -exec md5sum {} + | sed 's| \./| |') > "$work/control/md5sums"

printf '2.0\n' > "$work/debian-binary"
tar --owner=0 --group=0 --numeric-owner -C "$work/control" -czf "$work/control.tar.gz" .
tar --owner=0 --group=0 --numeric-owner -C "$root" -czf "$work/data.tar.gz" .
out="dist/kairo_${version}_${arch}.deb"
rm -f "$out"
(cd "$work" && ar rc deb.ar debian-binary control.tar.gz data.tar.gz)
mv "$work/deb.ar" "$out"
echo "$out"
