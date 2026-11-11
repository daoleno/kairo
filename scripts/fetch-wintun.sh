#!/bin/sh
# scripts/fetch-wintun.sh [dir]
# Downloads the pinned wintun release (scripts/mihomo.env), checks the zip against
# its published SHA-256 and writes dir/wintun.dll (the amd64 build) and
# dir/LICENSE.wintun.txt. Windows TUN needs the library next to the core.
set -e
here=$(cd "$(dirname "$0")" && pwd)
. "$here/mihomo.env"
dir=${1:-$here/../target/mihomo}
name="wintun-$WINTUN_VERSION.zip"
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
mkdir -p "$dir"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 8 --retry-delay 15 --retry-all-errors --connect-timeout 30 -o "$tmp/$name" "https://www.wintun.net/builds/$name"
got=$(sha256 "$tmp/$name")
[ "$got" = "$WINTUN_SHA256" ] || { echo "$name: SHA-256 $got, expected $WINTUN_SHA256" >&2; exit 1; }
py=$(command -v python3 || command -v python)
"$py" -I - "$tmp/$name" "$dir" <<'PY'
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
open(sys.argv[2] + "/wintun.dll", "wb").write(z.read("wintun/bin/amd64/wintun.dll"))
open(sys.argv[2] + "/LICENSE.wintun.txt", "wb").write(z.read("wintun/LICENSE.txt"))
PY
echo "$dir/wintun.dll ($name, sha256 $WINTUN_SHA256)"
