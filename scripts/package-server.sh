#!/bin/sh
# scripts/package-server.sh [page-dir]
# dist/Kairo-server-<version>-linux-<arch>.tar.gz: kairo-server (the engine and
# the page behind a password, no GTK), the pinned mihomo core in lib/kairo, the
# page in share/kairo-server/web (never share/kairo: that is Kairo's data folder), a systemd user service, the licences and an install
# script for ~/.local (or a prefix). The page is page-dir when given (the
# output of `nsh package --target web`, a folder with index.html), else built.
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
name="Kairo-server-$version-linux-$arch"
out="dist/$name"
rm -rf "$out" "dist/$name.tar.gz"
mkdir -p "$out/bin" "$out/lib/kairo" "$out/share/kairo-server" "$out/share/doc/kairo"

page=$1
if [ -z "$page" ]; then
  nsh package --target web -o dist/web .
  page=dist/web/Kairo
fi
test -s "$page/index.html" || { echo "no page in $page" >&2; exit 1; }
cp -R "$page" "$out/share/kairo-server/web"

cargo build --release -p kairo-server
cp "${CARGO_TARGET_DIR:-target}/release/kairo-server" "$out/bin/kairo-server"

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
cp crates/kairo-server/SECURITY-NOTES.md "$out/share/doc/kairo/SECURITY-NOTES.md"
cp crates/kairo-server/kairo-server.service "$out/share/kairo-server/kairo-server.service"

cat > "$out/install.sh" <<'EOF'
#!/bin/sh
# ./install.sh [prefix]   (default: ~/.local)
# Installs kairo-server and its page, and a systemd user service that starts it.
set -e
prefix=${1:-$HOME/.local}
here=$(cd "$(dirname "$0")" && pwd)
install -Dm755 "$here/bin/kairo-server" "$prefix/bin/kairo-server"
install -Dm755 "$here/lib/kairo/mihomo" "$prefix/lib/kairo/mihomo"
rm -rf "$prefix/share/kairo-server/web"
mkdir -p "$prefix/share/kairo-server"
cp -R "$here/share/kairo-server/web" "$prefix/share/kairo-server/web"
for f in LICENSE LICENSE.mihomo NOTICE SECURITY-NOTES.md; do
  install -Dm644 "$here/share/doc/kairo/$f" "$prefix/share/doc/kairo/$f"
done
units=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
mkdir -p "$units"
sed "s|@BIN@|$prefix/bin/kairo-server|" "$here/share/kairo-server/kairo-server.service" > "$units/kairo-server.service"
systemctl --user daemon-reload 2>/dev/null || true
cat <<DONE
kairo-server installed in $prefix.

Start it now and with this account:   systemctl --user enable --now kairo-server
Start it at boot, before you log in:  loginctl enable-linger "$USER"
The code that sets the password:      $prefix/bin/kairo-server status
Then open http://kairo.localhost:7979 (on this machine).
For other devices: add --listen ADDR:PORT --tls-cert CERT.pem --tls-key KEY.pem to
ExecStart in the unit (plain HTTP there opens only with --insecure-http). Read
$prefix/share/doc/kairo/SECURITY-NOTES.md first.
DONE
EOF
cat > "$out/uninstall.sh" <<'EOF'
#!/bin/sh
# ./uninstall.sh [prefix]   Removes kairo-server; your data (~/.local/share/kairo) stays.
prefix=${1:-$HOME/.local}
systemctl --user disable --now kairo-server 2>/dev/null || true
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/kairo-server.service"
systemctl --user daemon-reload 2>/dev/null || true
rm -rf "$prefix/share/kairo-server"
# The docs and the core stay when the desktop app (bin/kairo) uses them too.
[ -e "$prefix/bin/kairo" ] || rm -rf "$prefix/share/doc/kairo"
rm -f "$prefix/bin/kairo-server"
# The core stays when the desktop app (bin/kairo) uses it too.
[ -e "$prefix/bin/kairo" ] || rm -rf "$prefix/lib/kairo"
echo "kairo-server removed from $prefix"
EOF
chmod +x "$out/install.sh" "$out/uninstall.sh"
cp LICENSE README.md "$out/"

tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
