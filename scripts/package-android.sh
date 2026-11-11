#!/bin/sh
# scripts/package-android.sh <arm64-v8a|x86_64> [out-dir]
# Builds Kairo's Android APK for one ABI: dist/Kairo-<version>-android-<abi>.apk.
#
# `nsh package` makes the app (the NSH runtime's dex and Kairo's libnsh.so); its
# manifest has nothing but INTERNET and no place for a VpnService, so the APK is
# put together here instead: our manifest (android/), the NSH runtime's dex, our
# Java (android/java: the VPN service and its bridge to the Rust core), libnsh.so,
# and the mihomo core built for Android (scripts/build-mihomo-android.sh) as
# libmihomo.so, which the installer unpacks where an app may execute it.
#
# Signing: with KAIRO_ANDROID_KEYSTORE (a .p12/.jks path) and
# KAIRO_ANDROID_KEY_PASSWORD_FILE (a file holding its password) the release key
# signs it; without them, the debug key (a test build, not an update of a release).
# In CI the secrets are written to $RUNNER_TEMP first. The password is never printed.
# Needs: nsh, the Android SDK (build-tools 35+, platform 35) and NDK, Go, a JDK.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
abi=${1:?usage: package-android.sh <arm64-v8a|x86_64> [out-dir]}
out=${2:-$here/dist}
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
bt=$(ls -d "$sdk"/build-tools/*/ | sort -V | tail -1); bt=${bt%/}
jar=$(ls -d "$sdk"/platforms/android-3[5-9]*/android.jar 2>/dev/null | sort -V | tail -1)
[ -f "$jar" ] || { echo "no Android platform 35+ in $sdk" >&2; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -1)
code=$(sh "$here/scripts/android-version-code.sh" "$version")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

# The app: NSH's runtime and libnsh.so (it signs with a debug key; we sign again).
(cd "$here" && nsh package --target android --abi "$abi" -o "$work/nsh" . >/dev/null)
mkdir "$work/nsh-apk"
python3 -I - "$work/nsh/Kairo.apk" "$work/nsh-apk" <<'PY'
import sys, zipfile
zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])
PY

# The core.
core=$here/target/android-core/$abi
[ -f "$core/libmihomo.so" ] || sh "$here/scripts/build-mihomo-android.sh" "$abi" "$core" >/dev/null

# Our Java, as a dex beside NSH's.
mkdir "$work/classes"
javac -nowarn --release 17 -cp "$jar" -d "$work/classes" $(find "$here/android/java" -name '*.java')
(cd "$work/classes" && find . -name '*.class' | sed 's|^\./||' > "$work/classlist")
jar cf "$work/kairo.jar" -C "$work/classes" .
"$bt/d8" --min-api 28 --lib "$jar" --output "$work" "$work/kairo.jar"

# Manifest and resources (the icon).
mkdir "$work/compiled"
"$bt/aapt2" compile --dir "$here/android/res" -o "$work/compiled"
manifest=$here/android/AndroidManifest.xml
if [ -z "${KAIRO_ANDROID_KEYSTORE:-}" ]; then
  # A test build (debug key) can be inspected with run-as; a release one cannot.
  sed 's|<application|<application android:debuggable="true"|' "$manifest" > "$work/AndroidManifest.xml"
  manifest=$work/AndroidManifest.xml
fi
"$bt/aapt2" link -I "$jar" --manifest "$manifest" \
  --version-code "$code" --version-name "$version" \
  -o "$work/base.apk" "$work"/compiled/*.flat

# Everything in one APK.
python3 -I - "$work" "$abi" "$core/libmihomo.so" <<'PY'
import sys, zipfile, os
work, abi, mihomo = sys.argv[1:4]
src = os.path.join(work, "nsh-apk")
with zipfile.ZipFile(os.path.join(work, "unsigned.apk"), "w", zipfile.ZIP_DEFLATED) as z:
    base = zipfile.ZipFile(os.path.join(work, "base.apk"))
    for info in base.infolist():
        z.writestr(info, base.read(info.filename), compress_type=info.compress_type)
    z.write(os.path.join(work, "classes.dex"), "classes.dex")
    z.write(os.path.join(src, "classes.dex"), "classes2.dex")
    z.write(os.path.join(src, "lib", abi, "libnsh.so"), f"lib/{abi}/libnsh.so")
    z.write(mihomo, f"lib/{abi}/libmihomo.so")
    for root, _, files in os.walk(os.path.join(src, "assets")):
        for f in files:
            p = os.path.join(root, f)
            z.write(p, os.path.relpath(p, src))
PY
"$bt/zipalign" -f -p 4 "$work/unsigned.apk" "$work/aligned.apk"

apk="$out/Kairo-$version-android-$abi.apk"
if [ -n "${KAIRO_ANDROID_KEYSTORE:-}" ]; then
  [ -f "${KAIRO_ANDROID_KEY_PASSWORD_FILE:-}" ] || { echo "KAIRO_ANDROID_KEY_PASSWORD_FILE is not a file" >&2; exit 1; }
  "$bt/apksigner" sign --ks "$KAIRO_ANDROID_KEYSTORE" --ks-key-alias kairo \
    --ks-pass "file:$KAIRO_ANDROID_KEY_PASSWORD_FILE" \
    --out "$apk" "$work/aligned.apk"
else
  dk=${ANDROID_DEBUG_KEYSTORE:-$HOME/.android/debug.keystore}
  if [ ! -f "$dk" ]; then
    mkdir -p "$(dirname "$dk")"
    keytool -genkeypair -keystore "$dk" -storepass android -keypass android -alias androiddebugkey \
      -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null 2>&1
  fi
  "$bt/apksigner" sign --ks "$dk" --ks-pass pass:android --key-pass pass:android --out "$apk" "$work/aligned.apk"
fi
"$bt/apksigner" verify "$apk"
echo "$apk"
