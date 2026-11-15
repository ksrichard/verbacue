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
commit=402979ac1bef89dcd55add41eaa7277c407599eb
dir=$1
mkdir -p "$dir"
dir=$(cd "$dir" && pwd)

if [ ! -d "$dir/src" ]; then
    git clone --quiet --depth 1 --branch "v$version" \
        https://github.com/microsoft/onnxruntime "$dir/src"
fi
if [ "$(git -C "$dir/src" rev-parse HEAD)" != "$commit" ]; then
    echo "error: v$version of ONNX Runtime is not commit $commit" >&2
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
