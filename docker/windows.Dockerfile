# Cross-compiles a self-contained Windows x64 binary (CPU only) with MinGW-w64.
# It links everything statically, so it needs only the DLLs that come with Windows.
# Use docker/build.sh; it writes dist/windows-x64/verbacue.exe. CI builds Windows with
# MSVC instead, because CUDA does not work with MinGW.
FROM rust:1.94-trixie AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang libclang-dev cmake nasm pkg-config make \
        gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 \
    && rm -rf /var/lib/apt/lists/*
# whisper.cpp and llama.cpp use std::thread, which needs the POSIX thread variant.
RUN update-alternatives --set x86_64-w64-mingw32-gcc /usr/bin/x86_64-w64-mingw32-gcc-posix \
    && update-alternatives --set x86_64-w64-mingw32-g++ /usr/bin/x86_64-w64-mingw32-g++-posix
# whisper-rs-sys links the C++ library as a DLL. Without the import library, the
# linker must use the static libstdc++.a, so verbacue.exe needs no libstdc++-6.dll.
RUN find /usr/lib/gcc/x86_64-w64-mingw32 -name 'libstdc++.dll.a' -delete
RUN rustup target add x86_64-pc-windows-gnu

# ffmpeg-sys-next cannot build FFmpeg for Windows itself, so build it here.
# Use the same FFmpeg branch that ffmpeg-sys-next 9 builds on macOS and Linux.
# D3D11VA lets --burn decode AV1 video, which has no software decoder in this build.
# The D3D11VA and D3D12VA headers pull in <windows.h> with MinGW, and bindgen cannot
# convert the compiler intrinsics that come with it. ffmpeg-sys-next binds these
# headers only if they exist, and verbacue needs no D3D types, so delete them. FFmpeg
# itself keeps D3D11VA decoding.
RUN git clone --depth 1 --branch release/9.0 https://github.com/FFmpeg/FFmpeg /ffmpeg-src \
    && cd /ffmpeg-src \
    && ./configure --prefix=/opt/ffmpeg --target-os=mingw32 --arch=x86_64 \
        --cross-prefix=x86_64-w64-mingw32- --enable-static --disable-shared \
        --disable-programs --disable-doc --disable-autodetect \
        --enable-w32threads --enable-d3d11va --disable-d3d12va \
    && make -j"$(nproc)" install \
    && rm -f /opt/ffmpeg/include/libavutil/hwcontext_d3d1*.h \
    && rm -rf /ffmpeg-src

# Do not tune the code for the CPU of the build machine; x86-64-v3 needs AVX2.
# llama-cpp-sys-2 links advapi32 (registry calls of ggml) only for MSVC.
ENV FFMPEG_DIR=/opt/ffmpeg \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    RUSTFLAGS="-C link-arg=-static -C link-arg=-ladvapi32 -C target-cpu=x86-64-v3" \
    GGML_NATIVE=OFF GGML_AVX=ON GGML_AVX2=ON GGML_FMA=ON GGML_F16C=ON GGML_BMI2=ON
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    docker/build-windows.sh /out

FROM scratch
COPY --from=build /out/ /
