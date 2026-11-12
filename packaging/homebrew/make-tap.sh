#!/bin/sh
# Writes the Homebrew tap for one release: Formula/verbacue.rb (the command and the
# app as `verbacue gui`, for macOS and Linux, so also Windows through WSL),
# Casks/verbacue-app.rb (verbacue.app from the DMG, macOS only) and README.md.
# Usage: packaging/homebrew/make-tap.sh <version> <SHA256SUMS of the release> <out dir>
# The release job pushes the result to github.com/ksrichard/homebrew-verbacue.
set -eu
version=$1
sums=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
mkdir -p "$3"
out=$(cd "$3" && pwd)
cd "$(dirname "$0")"

sha() {
    hash=$(awk -v f="$1" '$2 == f || $2 == "*" f { print $1 }' "$sums")
    if [ -z "$hash" ]; then
        echo "error: $1 is not in $sums" >&2
        exit 1
    fi
    echo "$hash"
}
macos=$(sha verbacue-macos-universal-metal.tar.gz)
linux_amd64=$(sha verbacue-linux-amd64-cpu-gui.tar.gz)
linux_arm64=$(sha verbacue-linux-arm64-cpu-gui.tar.gz)
dmg=$(sha "verbacue-$version-macos-universal.dmg")

mkdir -p "$out/Formula" "$out/Casks"
sed -e "s/@VERSION@/$version/" -e "s/@SHA256_MACOS@/$macos/" \
    -e "s/@SHA256_LINUX_AMD64@/$linux_amd64/" -e "s/@SHA256_LINUX_ARM64@/$linux_arm64/" \
    verbacue.rb.in > "$out/Formula/verbacue.rb"
sed -e "s/@VERSION@/$version/" -e "s/@SHA256_DMG@/$dmg/" \
    verbacue-app.rb.in > "$out/Casks/verbacue-app.rb"
cp tap-README.md "$out/README.md"
if grep -n '@[A-Z0-9_]*@' "$out/Formula/verbacue.rb" "$out/Casks/verbacue-app.rb"; then
    echo "error: a placeholder was not replaced" >&2
    exit 1
fi
echo "wrote the tap for verbacue $version to $out"
