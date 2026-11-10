#!/bin/sh
# scripts/build-mihomo-android.sh <arm64-v8a|x86_64> [dir]
# Builds the pinned mihomo (scripts/mihomo.env: same tag as the desktop apps) for
# Android and writes dir/libmihomo.so (default dir: target/android-core/<abi>).
# It is named lib*.so so the APK installer unpacks it into the app's native
# library directory, the one place an app may execute a file from.
#
# Why not the official mihomo-android build: an ordinary app cannot use it as a
# VPN core. It opens a netlink socket (banned for apps) and reads
# /data/system/packages.xml (unreadable). The `cmfa` build tag, which Clash Meta
# for Android uses, removes the second; `auto-detect-interface: false` in the
# config (Kairo writes it) avoids the first. `cmfa` also defaults to "embed mode",
# which disables config reloads through the REST API; that default is removed
# (see below). mihomo is GPL-3.0, like Kairo.
#
# Needs Go (the version mihomo's go.mod asks for), git, and the Android NDK
# (ANDROID_NDK_HOME, or $ANDROID_HOME/ndk/<newest>, or ~/Android/Sdk/ndk/<newest>).
set -e
here=$(cd "$(dirname "$0")" && pwd)
. "$here/mihomo.env"
abi=${1:?usage: build-mihomo-android.sh <arm64-v8a|x86_64> [dir]}
dir=${2:-$here/../target/android-core/$abi}
api=28 # the APK's minSdk
case "$abi" in
  arm64-v8a) goarch=arm64; triple=aarch64-linux-android ;;
  x86_64) goarch=amd64; triple=x86_64-linux-android ;;
  *) echo "unknown ABI $abi (arm64-v8a or x86_64)" >&2; exit 1 ;;
esac
ndk=${ANDROID_NDK_HOME:-}
if [ -z "$ndk" ]; then
  sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
  ndk=$(ls -d "$sdk"/ndk/*/ 2>/dev/null | sort -V | tail -1)
  ndk=${ndk%/}
fi
[ -d "$ndk" ] || { echo "no Android NDK (set ANDROID_NDK_HOME)" >&2; exit 1; }
cc=$(ls "$ndk"/toolchains/llvm/prebuilt/*/bin/"$triple$api"-clang | head -1)
[ -x "$cc" ] || { echo "no $triple$api-clang in $ndk" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git init -q "$tmp/src"
git -C "$tmp/src" fetch -q --depth 1 https://github.com/MetaCubeX/mihomo.git "$MIHOMO_COMMIT"
git -C "$tmp/src" checkout -q FETCH_HEAD
got=$(git -C "$tmp/src" rev-parse HEAD)
[ "$got" = "$MIHOMO_COMMIT" ] || { echo "mihomo source is $got, expected $MIHOMO_COMMIT" >&2; exit 1; }

# The `cmfa` build starts mihomo in "embed mode" (hub/route/patch_android.go),
# which turns off PUT/PATCH /configs and the rules API: the host app is expected
# to restart the core instead. Kairo reloads configs through the API on every
# profile or settings change, so that one file, which only sets the default, is
# removed.
rm "$tmp/src/hub/route/patch_android.go"

mkdir -p "$dir"
out=$(cd "$dir" && pwd)
(
  cd "$tmp/src"
  GOOS=android GOARCH=$goarch CGO_ENABLED=1 CC="$cc" \
    go build -trimpath -tags "with_gvisor,cmfa" \
    -ldflags "-s -w -X github.com/metacubex/mihomo/constant.Version=$MIHOMO_VERSION -X github.com/metacubex/mihomo/constant.BuildTime=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    -o "$out/libmihomo.so" .
)
git -C "$tmp/src" archive --format=tar HEAD LICENSE | tar -xO -f - > "$out/LICENSE.mihomo"
echo "$out/libmihomo.so"
