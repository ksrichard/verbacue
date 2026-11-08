#!/bin/sh
# Builds self-contained verbacue binaries in Docker and writes them to dist/<target>/.
# Usage: docker/build.sh <target>...
#   linux-arm64, linux-amd64              CPU only, fully static
#   linux-arm64-cuda, linux-amd64-cuda    with CUDA (NVIDIA GPUs)
#   windows-x64                           CPU only, cross-compiled with MinGW-w64
# A target for another CPU type than the host's uses emulation and is much slower.
# macOS cannot be built in Docker; build it on a Mac with `cargo build --release`.
set -eu
cd "$(dirname "$0")/.."
[ $# -gt 0 ] || { sed -n '3,8p' "$0"; exit 1; }
cuda_image=nvidia/cuda:12.9.1-devel-ubuntu22.04
for target in "$@"; do
    set --
    case $target in
        linux-*-cuda) set -- --build-arg "BASE=$cuda_image" --build-arg FEATURES=cuda ;;
    esac
    case $target in
        linux-arm64*) set -- "$@" -f docker/linux.Dockerfile --platform linux/arm64 ;;
        linux-amd64*) set -- "$@" -f docker/linux.Dockerfile --platform linux/amd64 ;;
        windows-x64) set -- -f docker/windows.Dockerfile ;;
        *) echo "unknown target: $target" >&2; exit 1 ;;
    esac
    echo "== building $target"
    docker buildx build "$@" --output "type=local,dest=dist/$target" .
    cp LICENSE THIRD-PARTY-LICENSES.txt "dist/$target/"
done
ls -l dist/*/
