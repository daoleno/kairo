#!/bin/sh
# dist/Kairo-<version>-macos-universal.{dmg,zip}: Kairo.app (Apple silicon and
# Intel in one binary) with the pinned mihomo core (scripts/mihomo.env, both
# builds checksum verified and joined into Contents/MacOS/mihomo), ad hoc
# signed. Run on a Mac with both Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
build=$(sed -n 's/.*"build": *\([0-9]*\).*/\1/p' nsh.json)
name="Kairo-$version-macos-universal"
rm -rf dist/Kairo.app "dist/$name.dmg" "dist/$name.zip" dist/dmg dist/Kairo.iconset
mkdir -p dist

targets=${KAIRO_MAC_TARGETS:-"aarch64-apple-darwin x86_64-apple-darwin"}
bins=""
for t in $targets; do
  cargo build --release --target "$t"
  bins="$bins target/$t/release/kairo"
done

app=dist/Kairo.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# shellcheck disable=SC2086
lipo -create $bins -output "$app/Contents/MacOS/Kairo"
# Liquid Glass (the sidebar, toolbar and controls of macOS 26) needs the 26 SDK.
sdk=$(otool -l "$app/Contents/MacOS/Kairo" | awk '/LC_BUILD_VERSION/{f=1} f&&$1=="sdk"{print $2; exit}')
echo "linked against the macOS $sdk SDK"
[ "${sdk%%.*}" -ge 26 ] || { echo "the macOS 26 SDK or later is needed (got $sdk)" >&2; exit 1; }

# The core: both official builds, joined, signed on its own first.
cores=$(mktemp -d)
scripts/fetch-mihomo.sh darwin-aarch64 "$cores/arm64"
scripts/fetch-mihomo.sh darwin-x86_64 "$cores/x86_64"
lipo -create "$cores/arm64/mihomo" "$cores/x86_64/mihomo" -output "$app/Contents/MacOS/mihomo"
chmod 755 "$app/Contents/MacOS/mihomo"
cp "$cores/arm64/LICENSE.mihomo" "$app/Contents/Resources/LICENSE.mihomo"
rm -rf "$cores"
cp LICENSE "$app/Contents/Resources/LICENSE"
. scripts/mihomo.env
cat > "$app/Contents/Resources/NOTICE" <<NOTICE
Kairo $version: https://github.com/daoleno/kairo
Free software under the GNU GPL, version 3 or later (LICENSE).

It ships the mihomo core $MIHOMO_VERSION (Contents/MacOS/mihomo), the official
MetaCubeX builds for Apple silicon and Intel joined with lipo, otherwise
unmodified. mihomo is free software under the GNU GPL, version 3
(LICENSE.mihomo). Its source code:
https://github.com/MetaCubeX/mihomo/tree/$MIHOMO_VERSION
NOTICE
codesign --force --sign - --identifier dev.kairo.mihomo "$app/Contents/MacOS/mihomo"

iconset=dist/Kairo.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon-1024.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d assets/icon-1024.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Kairo.icns"
rm -rf "$iconset"

# Kairo's accent for AppKit's own controls (switches, the sidebar's selection,
# focus rings) where the Mac's accent is Multicolor: an asset catalog's
# AccentColor, Ember light and dark (src/accent.rs).
assets=$(mktemp -d)/Assets.xcassets
mkdir -p "$assets/AccentColor.colorset"
echo '{"info":{"version":1,"author":"xcode"}}' > "$assets/Contents.json"
cat > "$assets/AccentColor.colorset/Contents.json" <<'JSON'
{"info":{"version":1,"author":"xcode"},"colors":[
 {"idiom":"universal","color":{"color-space":"srgb","components":{"red":"0.878","green":"0.333","blue":"0.184","alpha":"1.000"}}},
 {"idiom":"universal","appearances":[{"appearance":"luminosity","value":"dark"}],"color":{"color-space":"srgb","components":{"red":"1.000","green":"0.416","blue":"0.239","alpha":"1.000"}}}
]}
JSON
accent_key=""
if xcrun actool --compile "$app/Contents/Resources" --platform macosx --minimum-deployment-target 11.0 \
    --output-partial-info-plist "$(dirname "$assets")/partial.plist" "$assets" >/dev/null; then
  accent_key="<key>NSAccentColorName</key><string>AccentColor</string>"
else
  echo "actool failed: AppKit controls keep the system accent" >&2
fi
rm -rf "$(dirname "$assets")"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kairo</string>
  <key>CFBundleDisplayName</key><string>Kairo</string>
  <key>CFBundleIdentifier</key><string>dev.kairo.app</string>
  <key>CFBundleExecutable</key><string>Kairo</string>
  <key>CFBundleIconFile</key><string>Kairo</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>Kairo is free software under the GPL-3.0-or-later.</string>
  $accent_key
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"

ditto -c -k --keepParent "$app" "dist/$name.zip"
mkdir -p dist/dmg
cp -R "$app" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -quiet -volname Kairo -srcfolder dist/dmg -fs HFS+ -format UDZO -ov "dist/$name.dmg"
rm -rf dist/dmg
echo "dist/$name.dmg"
echo "dist/$name.zip"
