#!/bin/sh
# scripts/fetch-mihomo.sh [os-arch] [dir]
# Downloads the pinned mihomo core (scripts/mihomo.env) for os-arch
# (linux-x86_64, linux-aarch64, darwin-aarch64, darwin-x86_64, windows-x86_64;
# default: this machine), checks it against the published SHA-256 and writes
# dir/mihomo (dir/mihomo.exe for Windows), plus dir/LICENSE.mihomo. The default dir, target/mihomo, is where a
# development build of Kairo looks for its core.
set -e
here=$(cd "$(dirname "$0")" && pwd)
. "$here/mihomo.env"
target=${1:-$(uname -s | tr 'A-Z' 'a-z')-$(uname -m)}
case "$target" in
  *-arm64) target=${target%-arm64}-aarch64 ;;
esac
dir=${2:-$here/../target/mihomo}
key=$(echo "MIHOMO_$target" | tr 'a-z-' 'A-Z_')
asset=$(eval echo "\${$key:-}")
sum=$(eval echo "\${${key}_SHA256:-}")
[ -n "$asset" ] && [ -n "$sum" ] || { echo "no pinned mihomo build for $target" >&2; exit 1; }
case "$target" in
  windows-*) ext=zip; out=mihomo.exe ;;
  *) ext=gz; out=mihomo ;;
esac
name="$asset-$MIHOMO_VERSION.$ext"
url="https://github.com/MetaCubeX/mihomo/releases/download/$MIHOMO_VERSION/$name"
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 8 --retry-delay 15 --retry-all-errors --connect-timeout 30 -o "$tmp/$name" "$url"
got=$(sha256 "$tmp/$name")
[ "$got" = "$sum" ] || { echo "$name: SHA-256 $got, expected $sum" >&2; exit 1; }
if [ "$ext" = zip ]; then
  # A zip with the one executable in it (Git Bash has no unzip; Python is on every runner).
  py=$(command -v python3 || command -v python)
  "$py" -I - "$tmp/$name" "$tmp/$out" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
names = [n for n in z.namelist() if n.endswith(".exe")]
assert len(names) == 1, names
open(sys.argv[2], "wb").write(z.read(names[0]))
PY
else
  gunzip -c "$tmp/$name" > "$tmp/$out"
fi
chmod 755 "$tmp/$out"
mv "$tmp/$out" "$dir/$out"
curl -fsSL --retry 8 --retry-delay 15 --retry-all-errors --connect-timeout 30 -o "$tmp/LICENSE" "https://raw.githubusercontent.com/MetaCubeX/mihomo/$MIHOMO_VERSION/LICENSE"
got=$(sha256 "$tmp/LICENSE")
[ "$got" = "$MIHOMO_LICENSE_SHA256" ] || { echo "mihomo LICENSE: SHA-256 $got, expected $MIHOMO_LICENSE_SHA256" >&2; exit 1; }
mv "$tmp/LICENSE" "$dir/LICENSE.mihomo"
echo "$dir/$out ($name, sha256 $sum)"
