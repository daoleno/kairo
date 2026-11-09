#!/bin/sh
# scripts/fetch-mihomo.sh [os-arch] [dir]
# Downloads the pinned mihomo core (scripts/mihomo.env) for os-arch
# (linux-x86_64, linux-aarch64, darwin-aarch64, darwin-x86_64; default: this
# machine), checks it against the published SHA-256 and writes dir/mihomo,
# plus dir/LICENSE.mihomo. The default dir, target/mihomo, is where a
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
name="$asset-$MIHOMO_VERSION.gz"
url="https://github.com/MetaCubeX/mihomo/releases/download/$MIHOMO_VERSION/$name"
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/$name" "$url"
got=$(sha256 "$tmp/$name")
[ "$got" = "$sum" ] || { echo "$name: SHA-256 $got, expected $sum" >&2; exit 1; }
gunzip -c "$tmp/$name" > "$tmp/mihomo"
chmod 755 "$tmp/mihomo"
mv "$tmp/mihomo" "$dir/mihomo"
curl -fsSL --retry 3 -o "$tmp/LICENSE" "https://raw.githubusercontent.com/MetaCubeX/mihomo/$MIHOMO_VERSION/LICENSE"
got=$(sha256 "$tmp/LICENSE")
[ "$got" = "$MIHOMO_LICENSE_SHA256" ] || { echo "mihomo LICENSE: SHA-256 $got, expected $MIHOMO_LICENSE_SHA256" >&2; exit 1; }
mv "$tmp/LICENSE" "$dir/LICENSE.mihomo"
echo "$dir/mihomo ($name, sha256 $sum)"
