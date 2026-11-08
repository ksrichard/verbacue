# Builds a Linux binary for the platform of the build (amd64 or arm64). A CPU build is
# fully static and runs on any Linux. A CUDA build needs glibc 2.35 or newer and the
# NVIDIA driver; everything else in it, including CUDA and cuBLAS, is static.
# Use docker/build.sh locally; CI uses this file too.
ARG BASE=ubuntu:22.04
FROM ${BASE} AS build
# Cargo features, for example "cuda". With "cuda", BASE must be an nvidia/cuda devel image.
ARG FEATURES=""
ARG RUST_VERSION=1.94.0
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl git build-essential clang libclang-dev cmake nasm pkg-config \
    && rm -rf /var/lib/apt/lists/*
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
