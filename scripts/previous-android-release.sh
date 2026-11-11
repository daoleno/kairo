#!/bin/sh
# scripts/previous-android-release.sh
# The latest published release's Android APK: prints "<versionName> <versionCode>",
# or "none" when that release has no APK. Used by the release workflow to hand
# check-android-apk.sh the code a new APK has to beat (KAIRO_PREVIOUS_ANDROID).
# Reads the public signed-manifest URL; no token. Fails if the network does.
set -e
base=${KAIRO_RELEASES_URL:-https://github.com/daoleno/kairo/releases}
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
bt=$(ls -d "$sdk"/build-tools/*/ | sort -V | tail -1); bt=${bt%/}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL "$base/latest/download/kairo-update.json" -o "$work/m.json"
set -- $(python3 -I - "$work/m.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
p = m["packages"].get("android-arm64-apk")
print(m["version"], p["name"] if p else "-")
PY
)
version=$1 name=$2
if [ "$name" = "-" ]; then echo none; exit 0; fi
curl -fsSL "$base/download/v$version/$name" -o "$work/prev.apk"
"$bt/aapt2" dump badging "$work/prev.apk" | sed -n "s/^package: .*versionCode='\([0-9]*\)' versionName='\([^']*\)'.*/\2 \1/p" | head -1
