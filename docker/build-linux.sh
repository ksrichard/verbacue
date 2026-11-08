#!/bin/sh
# Runs inside docker/linux.Dockerfile. Builds verbacue, checks what it links, and copies
# it to the folder in $1. A CPU build is fully static (glibc included), so it has no
# dynamic dependencies. A CUDA build must load the NVIDIA driver (libcuda.so) at run
# time, which a static program cannot do, so it links glibc and the driver dynamically.
set -eu
out=$1

# The build image is used only once, so it is safe to change it. Remove the shared
# C++ and CUDA libraries, so that the linker must use the static ones.
find /usr/lib/gcc -name 'libstdc++.so' -delete
case ${FEATURES:-} in *cuda*)
    lib=/usr/local/cuda/lib64
    for name in cudart cublas cublasLt; do
        rm -f "$lib/lib$name.so"
        ln -sf "lib${name}_static.a" "$lib/lib$name.a"
    done
    # libcuda.so is part of the driver; the build links against the stub.
    export LIBRARY_PATH="$lib/stubs${LIBRARY_PATH:+:$LIBRARY_PATH}"
    ;;
esac

# Use clang for everything, so the C++ headers and the C++ library come from the same
# GCC version. GCC 11 and 12 also cannot compile the fp16 code of ggml on arm64.
export CC=clang CXX=clang++
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=clang
# Do not tune the code for the CPU of the build machine.
export GGML_NATIVE=OFF
case $(uname -m) in
    x86_64)
        target=x86_64-unknown-linux-gnu
        export RUSTFLAGS="-C target-cpu=x86-64-v3" FFMPEG_MARCH=x86-64-v3
        export GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON GGML_BMI2=ON
        ;;
    aarch64)
        target=aarch64-unknown-linux-gnu
        export GGML_CPU_ARM_ARCH=armv8.2-a+fp16+dotprod FFMPEG_MARCH=armv8.2-a
        ;;
esac

case ${FEATURES:-} in
    *cuda*) ;;
    # Static glibc. With an explicit --target, RUSTFLAGS apply only to verbacue and its
    # libraries, not to build scripts and proc macros, which cannot be static.
    # The static libstdc++ comes after libc and libgcc on the link line and calls into
    # both, so link them once more at the end. rustc ends the link line in dynamic
    # mode, so name the archives; -lc would pick libc.so.
    *)
        export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static \
            -C link-arg=-Wl,--start-group -C link-arg=-l:libc.a -C link-arg=-l:libgcc.a \
            -C link-arg=-Wl,--end-group"
        # On arm64, libm.a uses the small GOT model, and a static PIE of this size
        # has too many GOT entries for it. A plain static binary has far fewer.
        if [ "$target" = aarch64-unknown-linux-gnu ]; then
            export RUSTFLAGS="$RUSTFLAGS -C relocation-model=static"
        fi
        ;;
esac

features=${FEATURES:+--features $FEATURES}
# The test binaries of a CUDA build need the driver, which the build has not got.
case ${FEATURES:-} in
    *cuda*) ;;
    *) cargo test --workspace --release --locked --target $target ;;
esac
cargo build --release --locked --target $target $features
mkdir -p "$out"
cp target/$target/release/verbacue "$out/"

echo "linked libraries:"
needed=$(readelf -d "$out/verbacue" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
echo "${needed:-none (static)}"
case ${FEATURES:-} in
    *cuda*) allowed='^(ld-linux.*|libc\.so.*|libm\.so.*|libgcc_s\.so.*|libpthread\.so.*|libdl\.so.*|librt\.so.*|libcuda\.so.*)$' ;;
    *) allowed='^$' ;;
esac
bad=$(echo "$needed" | grep -Ev "$allowed" || true)
if [ -n "$bad" ]; then
    echo "error: verbacue links libraries that it must not need: $bad" >&2
    exit 1
fi
