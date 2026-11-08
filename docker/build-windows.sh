#!/bin/sh
# Runs inside docker/windows.Dockerfile. Builds verbacue.exe, checks that it needs only
# Windows DLLs, and copies it to the folder in $1.
set -eu
out=$1
target=x86_64-pc-windows-gnu
build=target/$target/release/build

# FFmpeg comes from /opt/ffmpeg, outside the cargo cache, and ffmpeg-sys-next does not
# see changes there. Remove its old output, so the bindings match the headers.
rm -rf $build/ffmpeg-sys-next-*

# ggml drops the "lib" prefix of its archives on Windows (ggml.a), but whisper-rs-sys
# and llama-cpp-sys-2 look for libggml*.a. When the build stops for this reason, copy
# the archives with the prefix. Then delete the cargo fingerprints of those build
# script runs, so the scripts run again, find the copies, and link them.
pass=1
until cargo build --release --locked --target $target; do
    [ $pass -lt 4 ] || exit 1
    for dir in $build/whisper-rs-sys-*/out $build/llama-cpp-sys-2-*/out; do
        [ -d "$dir" ] || continue
        for f in $(find "$dir" -name 'ggml*.a'); do
            cp "$f" "$(dirname "$f")/lib$(basename "$f")"
        done
        unit=$(basename "$(dirname "$dir")")
        rm -rf "target/$target/release/.fingerprint/$unit"
    done
    pass=$((pass + 1))
done

mkdir -p "$out"
cp target/$target/release/verbacue.exe "$out/"
docker/check-dlls.sh "$out/verbacue.exe" x86_64-w64-mingw32-objdump
