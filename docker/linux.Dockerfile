# Builds a Linux binary for the platform of the build (amd64 or arm64). The amd64 CPU
# build is fully static and runs on any Linux. The arm64 CPU build needs glibc 2.35 or
# newer. A CUDA build needs glibc 2.35 or newer and the NVIDIA driver; CUDA and cuBLAS
# are static in it. A Vulkan build needs the glibc of its base image (2.39 for Ubuntu
# 24.04) and a Vulkan driver.
# Use docker/build.sh locally; CI uses this file too.
ARG BASE=ubuntu:22.04
FROM ${BASE} AS build
# Cargo features, for example "cuda". With "cuda", BASE must be an nvidia/cuda devel image.
ARG FEATURES=""
# More apt packages, separated by commas, for example the Vulkan parts for a Vulkan build.
ARG PACKAGES=""
ARG RUST_VERSION=1.94.0
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl git build-essential clang libclang-dev cmake nasm pkg-config $(echo "$PACKAGES" | tr , ' ') \
    && rm -rf /var/lib/apt/lists/*
# The prebuilt ONNX Runtime needs the C++ library of GCC 13. Ubuntu 22.04 has GCC 11,
# so take GCC 13 from the Ubuntu toolchain PPA. clang then uses its headers and its
# static libstdc++, so the binary still needs only glibc 2.35.
RUN . /etc/os-release && if [ "$VERSION_ID" = "22.04" ]; then \
        apt-get update \
        && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
            software-properties-common gpg-agent \
        && add-apt-repository -y ppa:ubuntu-toolchain-r/test \
        && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends g++-13 \
        && rm -rf /var/lib/apt/lists/*; \
    fi
RUN curl --proto '=https' -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION"
ENV PATH=/root/.cargo/bin:$PATH
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/src/target \
    FEATURES="$FEATURES" docker/build-linux.sh /out

FROM scratch
COPY --from=build /out/ /
