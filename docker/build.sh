#!/bin/sh
# Builds self-contained verbacue binaries in Docker and writes them to dist/<target>/.
# Usage: docker/build.sh <target>...
#   linux-arm64, linux-amd64              CPU only, fully static, command line only
#   linux-arm64-gui, linux-amd64-gui      CPU only, with the desktop app (glibc 2.35+)
#   linux-arm64-cuda, linux-amd64-cuda    with CUDA (NVIDIA GPUs)
#   linux-arm64-vulkan, linux-amd64-vulkan with Vulkan (AMD, Intel and NVIDIA GPUs)
#   windows-x64                           CPU only, cross-compiled with MinGW-w64
# A target for another CPU type than the host's uses emulation and is much slower.
# macOS cannot be built in Docker; build it on a Mac with `cargo build --release`.
set -eu
cd "$(dirname "$0")/.."
[ $# -gt 0 ] || { sed -n '3,10p' "$0"; exit 1; }
cuda_image=nvidia/cuda:12.9.1-devel-ubuntu22.04
vulkan_packages=libvulkan-dev,glslc,spirv-headers
for target in "$@"; do
    set --
    case $target in
        linux-*-cuda) set -- --build-arg "BASE=$cuda_image" --build-arg FEATURES=cuda ;;
        linux-*-gui) set -- --build-arg FEATURES=gui ;;
        # Ubuntu 22.04 has no glslc and too old Vulkan headers.
        linux-*-vulkan) set -- --build-arg BASE=ubuntu:24.04 --build-arg FEATURES=vulkan \
            --build-arg "PACKAGES=$vulkan_packages" ;;
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
