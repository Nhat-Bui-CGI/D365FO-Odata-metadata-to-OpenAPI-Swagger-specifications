#!/usr/bin/env bash
# Wraps a universal binary in an ad-hoc signed .app and puts it in a dmg.
# Usage: macos-dmg.sh <crate-binary> <app name> <bundle id> <version> <out.dmg>
set -euo pipefail
bin=$1 name=$2 id=$3 version=$4 out=$5

work=$(mktemp -d)
app="$work/pkg/$name.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$work/icon.iconset"
lipo -create -output "$app/Contents/MacOS/$bin" \
  "target/aarch64-apple-darwin/release/$bin" \
  "target/x86_64-apple-darwin/release/$bin"
for s in 16 32 128 256 512; do
  sips -z $s $s crates/d365-openapi-tauri/icons/icon.png \
    --out "$work/icon.iconset/icon_${s}x${s}.png" >/dev/null
done
iconutil -c icns "$work/icon.iconset" -o "$app/Contents/Resources/icon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleExecutable</key><string>$bin</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - "$app"
ln -s /Applications "$work/pkg/Applications"
hdiutil create -volname "$name" -srcfolder "$work/pkg" -ov -format UDZO "$out"
rm -rf "$work"
