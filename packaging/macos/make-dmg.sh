#!/bin/sh
# Makes verbacue.app and a DMG with it and a link to /Applications.
# Usage: packaging/macos/make-dmg.sh <verbacue binary> <output.dmg>
# The app is not signed, so macOS asks for confirmation at the first start
# (right-click, Open).
set -eu
cd "$(dirname "$0")/../.."
binary=$1
out=$2
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
work=$(mktemp -d)
app="$work/verbacue/verbacue.app"

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/verbacue"
cp packaging/icons/verbacue.icns "$app/Contents/Resources/verbacue.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>verbacue</string>
    <key>CFBundleDisplayName</key><string>verbacue</string>
    <key>CFBundleIdentifier</key><string>io.github.ksrichard.verbacue</string>
    <key>CFBundleExecutable</key><string>verbacue</string>
    <key>CFBundleIconFile</key><string>verbacue</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
</dict>
</plist>
PLIST
cp LICENSE THIRD-PARTY-LICENSES.txt "$work/verbacue/"
ln -s /Applications "$work/verbacue/Applications"

rm -f "$out"
hdiutil create -quiet -volname verbacue -srcfolder "$work/verbacue" -format UDZO -ov "$out"
rm -rf "$work"
echo "wrote $out"
