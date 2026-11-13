<p align="center"><img src="assets/wordmark.svg" alt="verbacue" width="520"></p>

# verbacue

`verbacue` makes translated subtitles for a video. It works fully offline, in one
self-contained binary, with a desktop app and a command line.

```sh
verbacue                    # opens the desktop app
verbacue movie.mkv --to hu
# writes movie.en.srt (the transcript) and movie.hu.srt (the translation)
```

The pipeline has four steps:

1. Decode the audio with FFmpeg. FFmpeg is built from source and linked statically.
2. Transcribe the audio with Whisper (whisper.cpp). Whisper detects the language.
3. Translate the subtitles with a local language model (llama.cpp, TranslateGemma 12B
   by default).
4. Write SRT or WebVTT files. Optionally, also write a video with the subtitles.

The binary needs no ffmpeg, Python, Ollama or other installed tools at run time. Only
the model weights are separate files. verbacue downloads them on first use, checks
their SHA256, and keeps them in the cache folder (`~/Library/Caches/verbacue/models` on
macOS, `~/.cache/verbacue/models` on Linux, `%LOCALAPPDATA%\verbacue\models` on Windows).

## Desktop app

`verbacue` with no arguments, or `verbacue gui`, opens the desktop app. A double-click
on the binary also opens it.

1. Pick a video: drop it on the grill, or click "Choose a video".
2. Look at the preview, and choose "Burned in" (a new MP4) or "Separate SRT file".
   Choose the target language and the audio track.
3. Click "Start grilling", and watch the barbecue while verbacue works.

All the options of the command line that make sense in the app are on the Settings
page: the models, the spoken language, the translator, the form of address, the
glossary, the subtitle format and the output folder. The app saves them in `gui.toml`
in the config folder (`~/.config/verbacue` on macOS and Linux, `%APPDATA%\verbacue` on
Windows). Keys for online services stay where the command line reads them.

[SCREENSHOT]

### Homebrew

On macOS (Apple Silicon and Intel), Linux (x86_64 and arm64) and Windows in WSL:

```sh
brew install ksrichard/verbacue/verbacue          # the command, and the app as `verbacue gui`
brew install --cask ksrichard/verbacue/verbacue-app   # macOS: verbacue.app in /Applications
```

Install one of the two; both include the `verbacue` command. The formula needs no
confirmation at the first start; the cask does, because the app is not notarized
(see the macOS row below). On Linux the formula installs the `cpu-gui` binary.

After each release, the `homebrew` workflow (`.github/workflows/homebrew.yml`) writes
both from `packaging/homebrew/`, installs them with Homebrew on macOS and Linux, and then
pushes them to the tap [ksrichard/homebrew-verbacue](https://github.com/ksrichard/homebrew-verbacue).
That needs the repository secret `HOMEBREW_TAP_TOKEN`, a token that may write to the tap's
contents. Start the workflow by hand with a version such as `0.1.0` to test or republish
the tap of an existing release.

### Installers

Each release also has installers for the desktop app:

| System | File | Notes |
|---|---|---|
| macOS | `verbacue-<version>-macos-universal.dmg` | For Apple Silicon and Intel Macs. Drag verbacue to Applications. The app is not notarized by Apple: at the first start, right-click it and choose Open (on macOS 15 or newer, open it once, then choose Open Anyway in System Settings → Privacy & Security). |
| Windows | `verbacue-<version>-windows-x64-<cpu\|cuda\|vulkan>-setup.exe` | A setup wizard with a Start menu entry and an uninstaller. |
| Linux | `verbacue-<version>-linux-<amd64\|arm64>-<cpu-gui\|vulkan\|cuda>.deb` | For Debian, Ubuntu and similar: `sudo apt install ./verbacue-….deb`. Adds a menu entry. |
| Linux | `verbacue-<version>-linux-<amd64\|arm64>-<cpu-gui\|vulkan\|cuda>.AppImage` | One file for most distributions: make it executable and start it. |

`packaging/` holds the scripts and icons that make them.

All binaries include the app, except the fully static Linux CPU binaries
(`linux-*-cpu`, for servers and containers) and the MinGW build from `docker/build.sh`.
On Linux without a GPU, use `linux-*-cpu-gui`; it links glibc 2.35 or newer
dynamically, because a static program cannot open a window. Without a display (for example
over SSH), `verbacue` with no arguments prints the help.

## Build

Ready-made binaries are on the [releases page](https://github.com/ksrichard/verbacue/releases).
They come from the `build` GitHub workflow
(`.github/workflows/build.yml`). Start it by hand, or push a tag such as `v0.2.0`;
a tag also attaches the binaries to a GitHub release.

| Binary                                     | GPU                         | Needs on the target system                 |
|--------------------------------------------|-----------------------------|--------------------------------------------|
| `macos-universal-metal`                    | Metal                       | macOS 11 or newer, Apple Silicon or Intel  |
| `linux-amd64-cpu`, `linux-arm64-cpu`       | none                        | nothing: fully static, runs on any Linux   |
| `linux-amd64-cpu-gui`, `linux-arm64-cpu-gui` | none                      | glibc 2.35 or newer                        |
| `linux-amd64-cuda`, `linux-arm64-cuda`     | NVIDIA (CUDA)               | glibc 2.35 or newer, NVIDIA driver         |
| `linux-amd64-vulkan`, `linux-arm64-vulkan` | AMD, Intel, NVIDIA (Vulkan) | glibc 2.39 or newer, Vulkan driver         |
| `windows-x64-cpu`                          | none                        | Windows 10 or 11                           |
| `windows-x64-cuda`                         | NVIDIA (CUDA)               | Windows 10 or 11, NVIDIA driver            |
| `windows-x64-vulkan`                       | AMD, Intel, NVIDIA (Vulkan) | Windows 10 or 11, GPU driver               |

Use a CUDA binary for an NVIDIA GPU. Use a Vulkan binary for other GPUs, including the
GPUs inside Intel and AMD processors.

All binaries are self-contained. They need no FFmpeg, Python or other installed tools,
and each build job checks what the binary links:

- The Linux CPU binaries are fully static, glibc included. They have no dynamic
  dependencies and run even in an empty container.
- The Linux CUDA binaries must load the NVIDIA driver (`libcuda.so`) at run time. A
  static program cannot do that, so they link glibc and the driver dynamically.
  CUDA and cuBLAS are inside the binary.
- The Linux Vulkan binaries load the Vulkan loader (`libvulkan.so.1`) in the same way.
  The GPU driver package installs it. glibc 2.39 means Ubuntu 24.04, Debian 13,
  Fedora 40 or newer.
- The Windows binaries need only the DLLs that are part of Windows and of the GPU
  driver (`vulkan-1.dll` for Vulkan). The CUDA zip has
  three CUDA DLLs next to `verbacue.exe`, because cuBLAS has no static library on
  Windows. Keep them in the same folder.
- macOS does not allow fully static programs. The binary links only system
  frameworks. It is universal: one file with code for Apple Silicon and for Intel.

The binaries contain the Mozilla root certificates, so model downloads also work on
systems without a certificate store. The x86_64 binaries and the Intel code of the macOS binary need a CPU with
AVX2 (Intel since 2013, AMD since 2015). The arm64 Linux binaries need ARMv8.2 (for example AWS
Graviton 2 or a Raspberry Pi 5).

### Build in Docker (Linux and Windows)

```sh
docker/build.sh linux-arm64 windows-x64     # writes dist/<target>/
```

The targets are `linux-arm64`, `linux-amd64`, `linux-arm64-gui`, `linux-amd64-gui`,
`linux-arm64-cuda`, `linux-amd64-cuda`,
`linux-arm64-vulkan`, `linux-amd64-vulkan` and `windows-x64`. A target for another CPU
type than the host's runs in emulation and is much slower. The Windows target is
cross-compiled with MinGW-w64 and has no GPU support. It is for local builds only: the
release binaries for Windows come from native MSVC builds in CI. For a Windows GPU build,
use the workflow or build on Windows.

### Build on macOS

```sh
xcode-select --install
brew install cmake nasm pkg-config
cargo build --release
# the binary is target/release/verbacue (about 33 MB)
```

You need Rust 1.88 or newer (edition 2024). The first build downloads the FFmpeg
source and compiles FFmpeg, whisper.cpp and llama.cpp. On an Apple M5 this takes
about 2 minutes. Later builds take seconds. The binary links only system libraries,
and the Metal shaders are inside it (check with `otool -L target/release/verbacue`).

### Build on Linux

```sh
sudo apt install build-essential clang libclang-dev cmake nasm pkg-config git
CC=clang CXX=clang++ cargo build --release
```

On arm64, clang is necessary: GCC 11 and 12 cannot compile the fp16 code of
whisper.cpp and llama.cpp ("target specific option mismatch"). This binary links
glibc and libstdc++ dynamically and is tuned for the build machine's CPU. For a
static, portable binary, use `docker/build.sh`.

### Build on Windows

The easiest way is `docker/build.sh windows-x64` (CPU only). For a native build, for
example with CUDA, install Visual Studio 2022 with the C++ tools, LLVM (for libclang),
Rust and vcpkg, and the CUDA toolkit or the Vulkan SDK for a GPU build. The CI
workflow uses these steps for all Windows builds.
FFmpeg comes from vcpkg, because the FFmpeg source build of ffmpeg-sys-next does not
support Windows targets. In a "Developer PowerShell for VS 2022":

```powershell
vcpkg install "ffmpeg[core,avcodec,avdevice,avfilter,avformat,swresample,swscale]:x64-windows-static"
$env:VCPKG_ROOT = "<vcpkg folder>"
$env:VCPKGRS_TRIPLET = "x64-windows-static"
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
$env:LLAMA_STATIC_CRT = "1"
cargo build --release
```

### GPU features

| Feature             | GPU                         | Platforms       | Build needs        |
|---------------------|-----------------------------|-----------------|--------------------|
| (always, on macOS)  | Apple GPU (Metal)           | macOS           | nothing            |
| `--features cuda`   | NVIDIA                      | Linux, Windows  | CUDA toolkit 12    |
| `--features vulkan` | AMD, Intel, NVIDIA (Vulkan) | Linux, Windows  | Vulkan SDK         |

Without a GPU feature, Whisper and the translation model run on the CPU. This works,
but it is much slower: plan for several hours for a full film.

## Usage

```sh
# Transcribe and translate to English (the default target)
verbacue movie.mkv

# Translate to Hungarian, with the formal form of address (ön)
verbacue movie.mkv --to hu --register formal

# Make Hungarian your default target
export VERBACUE_TO=hu

# Use a glossary for names and terms
verbacue movie.mkv --to hu --glossary names.txt

# Only transcribe, with a smaller and faster Whisper model
verbacue movie.mkv --translator none --model small

# Pick the second audio track; list the tracks first
verbacue tracks movie.mkv
verbacue movie.mkv --audio-track 1

# Translate an existing transcript again, without transcribing
verbacue translate movie.en.srt --to de

# Burn an existing (for example hand-corrected) SRT file into the video: movie.hu.mp4
verbacue burn movie.mkv movie.hu.srt

# WebVTT output, and a chosen output file
verbacue movie.mkv --to hu --format vtt -o subtitles.vtt

# A whole season: every media file in the folder. Files that already have an
# output are skipped; --force does them again.
verbacue ~/Videos/Season1 --to hu

# Video with subtitles
verbacue movie.mkv --to hu --embed   # movie.hu.mkv: streams copied, soft subtitle tracks
verbacue movie.mkv --to hu --burn    # movie.hu.mp4: subtitles drawn into the picture

# Language codes and models
verbacue languages
verbacue models list
verbacue models download large-v3
verbacue models rm large-v3
```

A glossary file has one `term = translation` pair per line. Lines that start with `#`
are comments:

```text
# names and places
Winterfell = Deres
Night's Watch = Éjjeli Őrség
```

### Options

| Option | Default | Meaning |
|---|---|---|
| `--to` | `en` (or `$VERBACUE_TO`) | Target language code, see `verbacue languages` |
| `--from` | `auto` | Source language code; `auto` detects it |
| `--model` | `large-v3` | Whisper model name (tiny, base, small, medium, large-v3, large-v3-turbo) or a file path |
| `--translator` | `local` | `local` (GGUF model), `deepl`, `google` or `azure` (online, see below), `openai-compat` (your own model server, see below), or `none` (transcript only) |
| `--endpoint` | `http://localhost:8080/v1` | Base URL of the server for `openai-compat` |
| `--endpoint-model` | first listed | Model name on the server |
| `--llm-model` | `translategemma-12b` | Translation model: `translategemma-12b`, `translategemma-27b`, `gemma-3-12b`, `gemma-4-26b-a4b`, or a path to any GGUF instruct model |
| `--register` | `informal` | Form of address: `informal` (Hungarian "te") or `formal` ("ön") |
| `--glossary` | | File with `term = translation` lines |
| `--context` | | What you know about the video (characters, relations, setting); the translator reads it |
| `--film-notes` | | Read the whole transcript first and write notes about the characters for the translator (slower; local model or own server) |
| `--audio-track` | `0` | Audio track index from `verbacue tracks` |
| `--list-tracks` | | List the audio tracks and stop (the same as `verbacue tracks`) |
| `--format` | `srt` | `srt` or `vtt` |
| `-o`, `--output` | | Output file for the translation (one input only) |
| `--embed` | | Also write `movie.<to>.mkv` with soft subtitle tracks |
| `--burn` | | Also write `movie.<to>.mp4` with burned-in subtitles |
| `--no-vad` | | Do not skip silence with voice activity detection |
| `--force` | | Process files that already have an output |
| `-v` | | Debug logs; `-vv` for more |

## Use as a Rust library

The library is split into crates. Each crate brings only its own C libraries, so a
program can use one step alone:

| Crate | Does | Compiles |
|---|---|---|
| `verbacue-core` | Errors, progress, SRT and WebVTT read, write and layout, the language list, the model list and downloads | Rust only |
| `verbacue-transcribe` | Speech to text with Whisper, from 16 kHz mono PCM | whisper.cpp |
| `verbacue-media` | Reads the audio of any video; writes MKV with subtitle tracks or MP4 with burned-in text | FFmpeg |
| `verbacue-translate` | Translates cues with a local model, DeepL, Google, Azure or an OpenAI-compatible server | llama.cpp (`local`), rustls (`online`) |
| `verbacue` | All of the above as modules, behind features | what the features turn on |
| `verbacue-gui` | The desktop app (iced) | all, plus the window and GPU libraries |
| `verbacue-app` | The `verbacue` command: command line and the desktop app (feature `gui`, on by default) | all |

The `verbacue` crate is the easy start. It has one module for each step: `audio`,
`transcribe`, `subtitle`, `translate`, `video` and `models`. The `pipeline` module
runs all steps for one video, the same way as the command and the app, and reports
each step, its progress and each new line; it can also stop a job early. Its features select the
crates:

| Feature | Adds | Compiles |
|---|---|---|
| (none) | `subtitle`, `models`, and the translation logic | Rust only |
| `audio` | `audio` | FFmpeg |
| `transcribe` | `audio` and `transcribe` (Whisper) | FFmpeg, whisper.cpp |
| `video` | `audio` and `video` (MKV and burned-in MP4) | FFmpeg |
| `local` | the local translation model | llama.cpp |
| `online` | DeepL, Google, Azure and OpenAI-compatible servers | rustls |
| `download` | model downloads into the cache | rustls |
| `cuda`, `vulkan` | GPU support on Linux and Windows | the GPU SDK |
| `clap` | `clap::ValueEnum` for the option enums | clap |

All default features except `clap` are on. For example, a program that only reads and
writes subtitles uses one of these:

```toml
verbacue = { version = "0.1", default-features = false }
verbacue-core = "0.1"
```

Two traits let you connect your own parts:

- `translate::Translator` translates one batch of lines. Implement it to use any
  other service.
- `translate::ChatModel` answers one prompt. Implement it to use any other language
  model; `translate::LlmTranslator` then does the batching, the prompts and the checks.

All functions block, because Whisper, llama.cpp and FFmpeg do their work on the
calling thread. From async code, call them with `spawn_blocking`. Long tasks report
their progress through the `Progress` trait; pass `&()` to ignore it. All crates use
the same `Error` type from `verbacue-core`. It has a `kind()` (for example
`QuotaExceeded` or `NotFound`) for the problems a program may want to handle. The
crate documentation (`cargo doc -p verbacue --open`) has a full example.

## How it works

### Transcription

Whisper runs with beam search. The Silero voice activity model skips silence and
music, because Whisper tends to invent text there. A filter then removes typical
invented lines ("Thanks for watching", subtitle credits), empty or punctuation-only
lines, and long runs of the same line.

Whisper often ends a segment in the middle of a sentence. verbacue joins such segments
and then splits them again at sentence ends. A cue is at most about 6 seconds and
84 columns long. Its time is shared out by text length.

### Translation

The cues go to the model in batches of 30 numbered lines as JSON:
`[{"id": 12, "text": "..."}]`. The last 5 translated lines go with each batch as
read-only context, so names, gender and the form of address stay the same.
Timestamps never go to the model; the translated text goes back onto the original
cues by id.

A GBNF grammar forces the model to answer with exactly the expected ids. The grammar
also allows only the script of the target language, plus ASCII and punctuation. So a
Hindi translation cannot contain Czech letters, and an Arabic one cannot contain
Hindi letters. The model sometimes tries this, even at 12B.

The answer is still checked. A line is invalid if it has a word in a wrong script, or
(in a non-Latin target) an English word that the source line does not have. If the
answer is not valid, the batch is sent once more with a stricter prompt, and after
that each line is sent alone.

The translation is then wrapped again (at most 2 lines of about 42 columns), because
many languages are longer than the source. Chinese and Japanese break between
characters and count as two columns each; Korean breaks at spaces.

### Online translators (optional)

`--translator deepl`, `google` or `azure` sends the subtitle text to an online
service instead of the local model. All other steps stay local. Each service reads
its key from an environment variable, or else from a file in the config folder
(`~/.config/verbacue/` on macOS and Linux, `%APPDATA%\verbacue\` on Windows):

| Translator | Key | Free amount |
|---|---|---|
| `deepl` | `DEEPL_API_KEY` or `deepl.key` | 1 million characters, once |
| `google` | `GOOGLE_TRANSLATE_API_KEY` or `google.key` | 500,000 characters per month |
| `azure` | `AZURE_TRANSLATOR_KEY` or `azure.key`, and `AZURE_TRANSLATOR_REGION` or `azure.region` | 2 million characters per month (F0 tier) |

One movie has about 80,000 characters. Only DeepL gets the earlier lines as context,
and only DeepL uses `--register`. The glossary works only with the local translator.

### Your own model server (optional)

`--translator openai-compat` sends the batches to a server with an OpenAI-compatible
chat API: llama-server, Ollama, LM Studio, vLLM or Colibri. The server runs as a
separate program, so it can host models that are too big for verbacue itself. The
prompts, the answer check, the retries and the glossary are the same as for the local
model.

```sh
llama-server -m some-model.gguf --port 8080 &
verbacue movie.mkv --to hu --translator openai-compat --endpoint http://localhost:8080/v1

# Ollama; the model name is required if the server has more than one model
verbacue movie.mkv --to hu --translator openai-compat \
  --endpoint http://localhost:11434/v1 --endpoint-model gemma3:12b
```

Without `--endpoint-model`, verbacue uses the first model that the server lists. A key
is optional; verbacue reads it from `OPENAI_API_KEY` or `openai.key` in the config
folder. If the endpoint is not on this computer, the subtitle text goes over the
network. verbacue asks the server for a JSON answer with a schema; if the server does
not support schemas, verbacue asks without one. A wrong answer makes verbacue ask again,
or translate that batch line by line.

Use a general instruct model on the server. llama-server has a special mode for
TranslateGemma that translates the whole prompt and ignores the schema, so
TranslateGemma works only with the local translator.

### Models

verbacue downloads models only from Hugging Face, and only when a model is used for the
first time. Each download must match the SHA256 that is fixed in
`crates/verbacue-core/src/models.rs`, so a changed file on the server is rejected. Use
`verbacue models download <name>` to download a model before you need it.

| Name | Used for | Size | Downloaded from |
|---|---|---|---|
| `tiny`, `base`, `small`, `medium` | Speech to text (`--model`) | 78 MB to 1.5 GB | `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-<name>.bin` |
| `large-v3` (default) | Speech to text (`--model`) | 3.1 GB | https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin |
| `large-v3-turbo` | Speech to text (`--model`) | 1.6 GB | https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin |
| `silero-vad` | Skips parts without speech (off with `--no-vad`) | 0.9 MB | https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin |
| `translategemma-12b` (default) | Translation (`--llm-model`) | 7.3 GB | https://huggingface.co/mradermacher/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it.Q4_K_M.gguf |
| `translategemma-27b` | Translation (`--llm-model`) | 14.9 GB | https://huggingface.co/mradermacher/translategemma-27b-it-GGUF/resolve/main/translategemma-27b-it.IQ4_XS.gguf |
| `gemma-3-12b` | Translation (`--llm-model`) | 7.3 GB | https://huggingface.co/ggml-org/gemma-3-12b-it-GGUF/resolve/main/gemma-3-12b-it-Q4_K_M.gguf |
| `gemma-4-26b-a4b` | Translation (`--llm-model`) | 13.6 GB | https://huggingface.co/unsloth/gemma-4-26B-A4B-it-GGUF/resolve/main/gemma-4-26B-A4B-it-UD-IQ4_XS.gguf |

The models have their own licenses. The Gemma models are under the Gemma Terms of Use.
To use a model file that you downloaded yourself, give its path to `--model` or
`--llm-model` instead of a name. A path is used as it is, without a download or a
checksum check.

### Memory

Whisper large-v3 needs about 4 GB and TranslateGemma 12B (Q4_K_M) about 8 GB.
TranslateGemma 27B (`--llm-model translategemma-27b`) and Gemma 4 26B-A4B
(`--llm-model gemma-4-26b-a4b`) need about 16 GB. Use `--model large-v3-turbo` for
faster transcription with more errors.
Whisper is unloaded before the translation model loads, so the peak is about 9 GB.

### Video output

- `--embed` writes MKV. All audio and video streams are copied without re-encoding,
  so any input that FFmpeg reads works. The translation becomes the default subtitle
  track; the transcript is a second track.
- `--burn` writes MP4. The video is encoded again with the hardware H.264 encoder
  (VideoToolbox) on macOS, or FFmpeg's MPEG-4 encoder on Linux and Windows. Audio is
  copied when MP4 supports the codec, and is encoded to AAC otherwise (for example DTS
  or TrueHD). AV1 video has no software decoder in this build. On macOS verbacue
  decodes it with VideoToolbox (Apple M3 or newer), and on Windows with Direct3D 11
  (a GPU with AV1 decoding). On Linux, `--burn` cannot read AV1 video.
  verbacue checks this before it transcribes, so it does not waste time on such a file.
  The embedded DejaVu Sans font covers Latin, Greek, Cyrillic, Hebrew and Arabic.
  For other scripts (Chinese, Japanese, Korean, Indic, Thai and others) verbacue uses
  the system fonts. macOS and Windows have them. On Linux, install the Noto fonts (for
  example `fonts-noto-core` and `fonts-noto-cjk` on Debian). The text is shaped with
  rustybuzz, so Arabic letters join, Indic syllables form correctly, and right-to-left
  lines are in the right order. If no font has the letters of a script, `--burn`
  stops with an error; `--embed` always works.

## Tests

```sh
cargo test --workspace                              # unit tests
cargo test --workspace --release -- --ignored       # end-to-end tests; download models
```

The end-to-end tests run the real binary. On macOS they use the `say` command to make
speech. The full-pipeline test also needs the `ffmpeg` command line tool to make a
test video (verbacue itself never needs it).

## License

verbacue is under the MIT license (see `LICENSE`).

The binaries contain the code below. `THIRD-PARTY-LICENSES.txt` holds all license
texts, also those of the Rust crates, and each release archive includes it. Run
`licenses/generate.sh` to make it again after a dependency changes.

- FFmpeg (LGPL 2.1+, statically linked, without its GPL parts)
- whisper.cpp and llama.cpp (MIT)
- DejaVu Sans Bold font (free license, see `crates/verbacue-media/assets/DejaVu-LICENSE.txt`)
- Model weights have their own licenses: Whisper (MIT), Silero VAD (MIT), Gemma 3 and
  TranslateGemma (Gemma Terms of Use), Gemma 4 (Apache 2.0)
