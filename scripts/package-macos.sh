#!/bin/sh
# dist/Kairo-<version>-macos-universal.{dmg,zip}: Kairo.app (Apple silicon and
# Intel in one binary), ad hoc signed. Run on a Mac with both Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
# The core is not included: Kairo downloads it on first run.
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

iconset=dist/Kairo.iconset
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon-1024.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d assets/icon-1024.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Kairo.icns"
rm -rf "$iconset"

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
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force --deep --sign - "$app"
codesign --verify --strict "$app"

ditto -c -k --keepParent "$app" "dist/$name.zip"
mkdir -p dist/dmg
cp -R "$app" dist/dmg/
ln -s /Applications dist/dmg/Applications
hdiutil create -quiet -volname Kairo -srcfolder dist/dmg -fs HFS+ -format UDZO -ov "dist/$name.dmg"
rm -rf dist/dmg
echo "dist/$name.dmg"
echo "dist/$name.zip"
