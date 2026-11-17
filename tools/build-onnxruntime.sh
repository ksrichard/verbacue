#!/bin/sh
# Builds a static ONNX Runtime for macOS on Intel. Dubbing needs ONNX Runtime, and
# neither Microsoft nor the ort crate ships a prebuilt one for Intel Macs.
#
# Usage: tools/build-onnxruntime.sh <work dir>
# Then build verbacue with ORT_LIB_LOCATION=<work dir>/lib. The version must match the
# ort crate (ort-sys 2.0.0-rc.15 uses 1.31.0).
#
# PYTHON names the Python (3.10 or newer) that runs the ONNX Runtime build script.
# The build takes about an hour on a CI runner.
set -eu
version=1.31.0
# The github.com/microsoft/onnxruntime repository is gone, so the source of v1.31.0
# (commit 402979ac1bef89dcd55add41eaa7277c407599eb) comes from the Software Heritage
# archive. This is the git tree of that commit.
swhid=swh:1:dir:6ebffa882d87ca25e05568f369892e4036124e14
# The git tree of the downloaded files, without line-ending conversion. It differs
# from the commit's tree only in the three git submodules, which are empty folders,
# as in a clone without --recursive.
tree=39f8c997f553d71f27b6c140625cc4ba8c495e59
dir=$1
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)

if [ ! -d "$dir/src" ]; then
    # The archive packs the source on request; this can take an hour.
    vault=https://archive.softwareheritage.org/api/1/vault/flat/$swhid/
    status=$(curl -fsS -X POST "$vault" | python3 -c 'import sys,json; print(json.load(sys.stdin)["status"])')
    tries=0
    while [ "$status" != done ]; do
        if [ "$status" = failed ] || [ $tries -ge 240 ]; then
            echo "error: Software Heritage could not pack $swhid ($status)" >&2
            exit 1
        fi
        sleep 30
        tries=$((tries + 1))
        status=$(curl -fsS "$vault" | python3 -c 'import sys,json; print(json.load(sys.stdin)["status"])' || echo pending)
    done
    curl -fsSL --retry 5 --retry-all-errors -o "$dir/src.tar.gz" "${vault}raw/"
    rm -rf "$dir/src.tmp"
    mkdir "$dir/src.tmp"
    tar xzf "$dir/src.tar.gz" -C "$dir/src.tmp" --strip-components=1
    rm "$dir/src.tar.gz"
    mv "$dir/src.tmp" "$dir/src"
fi

# Check every file against the expected tree. A throwaway index keeps the source
# folder free of a .git folder, which the ONNX Runtime build would read.
check=$(mktemp -d)
got=$(cd "$dir/src" &&
    echo "* -text -filter -ident" > "$check/attributes" &&
    GIT_DIR="$check/git" git init --quiet &&
    cp "$check/attributes" "$check/git/info/attributes" &&
    GIT_DIR="$check/git" GIT_WORK_TREE=. git -c core.autocrlf=false add -f -A . &&
    GIT_DIR="$check/git" git write-tree)
rm -rf "$check"
if [ "$got" != "$tree" ]; then
    echo "error: the ONNX Runtime $version source is tree $got, not $tree" >&2
    exit 1
fi

# The same oldest macOS as verbacue (MACOSX_DEPLOYMENT_TARGET in the build workflow).
# The dependencies come from source, not from Homebrew, so their versions match. The
# telemetry is off; it would also link curl and mbedtls.
"${PYTHON:-python3}" "$dir/src/tools/ci_build/build.py" \
    --build_dir "$dir/build" \
    --config Release \
    --update --build --parallel \
    --skip_tests --skip_submodule_sync \
    --compile_no_warning_as_error \
    --osx_arch x86_64 \
    --apple_deploy_target "${MACOSX_DEPLOYMENT_TARGET:-11.0}" \
    --cmake_extra_defines onnxruntime_BUILD_UNIT_TESTS=OFF \
        onnxruntime_USE_TELEMETRY=OFF FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER
# ort links re2, but ONNX Runtime builds it only for its own shared library.
cmake --build "$dir/build/Release" --config Release --target re2 --parallel

# Keep only the static libraries, in the folder layout that ort expects.
rm -rf "$dir/lib"
rsync -a --include='*/' --include='*.a' --exclude='*' --prune-empty-dirs \
    "$dir/build/Release/" "$dir/lib/"
du -sh "$dir/lib"
