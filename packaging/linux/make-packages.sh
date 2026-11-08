#!/bin/sh
# Makes a .deb and an AppImage of the desktop app for one Linux build.
# Usage: packaging/linux/make-packages.sh <verbacue binary> <cpu-gui|vulkan|cuda> <out dir>
# The host must have the architecture of the binary (amd64 or arm64).
set -eu
cd "$(dirname "$0")/../.."
binary=$1
variant=$2
out=$3
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case $(uname -m) in
    x86_64) deb_arch=amd64 app_arch=x86_64 ;;
    aarch64) deb_arch=arm64 app_arch=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
name="verbacue-$version-linux-$deb_arch-$variant"
work=$(mktemp -d)
mkdir -p "$out"

# The window code loads these at run time; Vulkan builds also need the loader.
depends="libc6 (>= 2.35), libxkbcommon0, libxkbcommon-x11-0"
case $variant in
    vulkan) depends="libc6 (>= 2.39), libxkbcommon0, libxkbcommon-x11-0, libvulkan1" ;;
esac
recommends="libwayland-client0, libx11-6, libxcursor1, libxi6, libxrandr2, libegl1, libvulkan1"

# .deb
deb="$work/deb"
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/applications" \
    "$deb/usr/share/icons/hicolor/256x256/apps" "$deb/usr/share/doc/verbacue"
install -m 755 "$binary" "$deb/usr/bin/verbacue"
install -m 644 packaging/linux/verbacue.desktop "$deb/usr/share/applications/"
install -m 644 packaging/icons/verbacue-256.png "$deb/usr/share/icons/hicolor/256x256/apps/verbacue.png"
install -m 644 LICENSE "$deb/usr/share/doc/verbacue/copyright"
install -m 644 THIRD-PARTY-LICENSES.txt "$deb/usr/share/doc/verbacue/"
cat > "$deb/DEBIAN/control" <<CONTROL
Package: verbacue
Version: $version
Architecture: $deb_arch
Maintainer: verbacue <https://github.com/ksrichard/verbacue>
Installed-Size: $(du -sk "$deb/usr" | cut -f1)
Depends: $depends
Recommends: $recommends
Section: video
Priority: optional
Homepage: https://github.com/ksrichard/verbacue
Description: Subtitles for any video, made on this computer ($variant build)
 Transcribes speech with Whisper, translates with a local language model or an
 online service, and writes SRT files or videos with burned-in subtitles.
CONTROL
dpkg-deb --root-owner-group --build "$deb" "$out/$name.deb" >/dev/null

# AppImage. The tool and its runtime are pinned and checked; the tool unpacks itself,
# so no FUSE is needed to build.
tools="$work/tools"
mkdir -p "$tools"
case $app_arch in
    x86_64) tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
            runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64) tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
             runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
esac
curl -fsSL -o "$tools/appimagetool" \
    "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$app_arch.AppImage"
curl -fsSL -o "$tools/runtime" \
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$app_arch"
echo "$tool_sha  $tools/appimagetool" | sha256sum -c - >/dev/null
echo "$runtime_sha  $tools/runtime" | sha256sum -c - >/dev/null
chmod +x "$tools/appimagetool"

app="$work/verbacue.AppDir"
mkdir -p "$app/usr/bin"
install -m 755 "$binary" "$app/usr/bin/verbacue"
ln -s usr/bin/verbacue "$app/AppRun"
# In an AppImage, "Exec" names the AppRun entry; no arguments opens the app.
sed 's/^Exec=.*/Exec=verbacue/' packaging/linux/verbacue.desktop > "$app/verbacue.desktop"
install -m 644 packaging/icons/verbacue-256.png "$app/verbacue.png"
cp LICENSE THIRD-PARTY-LICENSES.txt "$app/"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$app_arch "$tools/appimagetool" --no-appstream \
    --runtime-file "$tools/runtime" "$app" "$out/$name.AppImage" >/dev/null 2>&1

rm -rf "$work"
ls -l "$out/$name.deb" "$out/$name.AppImage"
