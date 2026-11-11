#!/bin/sh
# scripts/check-android-apk.sh <apk> [release]
# What an APK from scripts/package-android.sh must hold. With "release": it is
# signed with Kairo's release key (the certificate's SHA-256 below).
set -e
apk=$1
[ -f "$apk" ] || { echo "usage: $0 <apk> [release]" >&2; exit 1; }
RELEASE_CERT_SHA256=95d8456a40eac46549f4dba6f6a26181612e724c229a397f2a46515de67c80f6
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}
bt=$(ls -d "$sdk"/build-tools/*/ | sort -V | tail -1); bt=${bt%/}
names=$(python3 -I -c "import sys,zipfile;print('\n'.join(zipfile.ZipFile(sys.argv[1]).namelist()))" "$apk")
for want in AndroidManifest.xml classes.dex classes2.dex 'lib/[a-z0-9_-]*/libnsh.so' 'lib/[a-z0-9_-]*/libmihomo.so'; do
  echo "$names" | grep -q "^$want\$" || { echo "the APK has no $want" >&2; exit 1; }
done
manifest=$("$bt/aapt2" dump xmltree --file AndroidManifest.xml "$apk")
echo "$manifest" | grep -q 'android.permission.BIND_VPN_SERVICE' || { echo "no VPN service in the manifest" >&2; exit 1; }
echo "$manifest" | grep -q 'extractNativeLibs.*=true' || { echo "extractNativeLibs must be true (the core runs from the native library directory)" >&2; exit 1; }
if echo "$manifest" | grep -q 'debuggable'; then
  [ "${2:-}" != release ] || { echo "a release APK must not be debuggable" >&2; exit 1; }
fi
# The version code comes from the version (scripts/android-version-code.sh), and a
# release's must beat the previous release's: KAIRO_PREVIOUS_ANDROID is "<version> <code>"
# of the latest published APK, or "none" (scripts/previous-android-release.sh).
badge=$("$bt/aapt2" dump badging "$apk" | sed -n "s/^package: .*versionCode='\([0-9]*\)' versionName='\([^']*\)'.*/\2 \1/p" | head -1)
name=${badge% *}; code=${badge#* }
[ -n "$badge" ] || { echo "no versionCode/versionName in the APK" >&2; exit 1; }
want=$(sh "$(dirname "$0")/android-version-code.sh" "$name")
[ "$code" = "$want" ] || { echo "versionCode $code for $name; it must be $want (scripts/android-version-code.sh)" >&2; exit 1; }
if [ "${2:-}" = release ]; then
  prev=${KAIRO_PREVIOUS_ANDROID:?set KAIRO_PREVIOUS_ANDROID to \"<version> <code>\" or none (scripts/previous-android-release.sh)}
  if [ "$prev" != none ]; then
    pname=${prev% *}; pcode=${prev#* }
    if [ "$pname" = "$name" ]; then
      echo "note: $name is already published (code $pcode); not comparing version codes"
    elif [ "$code" -le "$pcode" ]; then
      echo "versionCode $code ($name) must be greater than the previous release's $pcode ($pname)" >&2; exit 1
    fi
  fi
fi
certs=$("$bt/apksigner" verify --print-certs "$apk" 2>/dev/null)
echo "$certs"
if [ "${2:-}" = release ]; then
  echo "$certs" | grep -qi "certificate SHA-256 digest: $RELEASE_CERT_SHA256" || { echo "not signed with Kairo's release key" >&2; exit 1; }
fi
echo "ok: $apk"
