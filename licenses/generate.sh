#!/usr/bin/env bash
# Writes THIRD-PARTY-LICENSES.txt: the licenses of everything that the verbacue
# binaries contain. Run it again after a dependency changes.
# Needs cargo-about: cargo install cargo-about --locked --features cli
set -euo pipefail
cd "$(dirname "$0")/.."

out=THIRD-PARTY-LICENSES.txt
section() {
    printf '\n================================================================================\n'
    printf '%s\n' "$1"
    printf -- '--------------------------------------------------------------------------------\n'
    cat "$2"
}

{
    cat <<'EOF'
Third-party licenses for verbacue

The verbacue binaries contain the libraries below. verbacue itself is under the
MIT license (see LICENSE).

FFmpeg is under the LGPL 2.1 or later. The binaries use FFmpeg without its GPL
and nonfree parts. The source of FFmpeg is at https://ffmpeg.org, and the source
of verbacue is public, so you can build verbacue against a changed FFmpeg.

The CUDA builds also contain the NVIDIA CUDA runtime and cuBLAS. NVIDIA allows
this under the CUDA Toolkit license: https://docs.nvidia.com/cuda/eula/
EOF
    section "FFmpeg (LGPL-2.1-or-later), https://ffmpeg.org" licenses/LGPL-2.1.txt
    section "whisper.cpp (MIT), https://github.com/ggml-org/whisper.cpp" licenses/whisper.cpp-LICENSE.txt
    section "llama.cpp and ggml (MIT), https://github.com/ggml-org/llama.cpp" licenses/llama.cpp-LICENSE.txt
    section "DejaVu Sans Bold font (Bitstream Vera license), https://dejavu-fonts.github.io" crates/verbacue-media/assets/DejaVu-LICENSE.txt
    printf '\n\nRust crates\n'
    cargo about generate -c licenses/about.toml --workspace --all-features licenses/about.hbs
} > "$out"
echo "wrote $out"
