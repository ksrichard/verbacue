# Dubbing: a translated voice track for the video

Date: 2026-11-13. Status: approved by the pilot on 2026-11-13.

## Intent

verbacue makes translated subtitles. This feature adds a translated voice track. The
new track speaks the translated subtitles at their times. Each actor speaks with a
copy of their own voice (voice cloning). The original sound stays: music, effects
and noise. Only the speech changes.

The feature follows the rules of the rest of the app:

- It works fully offline.
- It runs inside the one binary. It does not use Python or other installed tools at
  run time.
- The binary stays under the MIT licence. It contains no GPL code.
- The app downloads the model weights on first use and checks their SHA256.

## Goals

- A new audio track in the target language, in an MKV or MP4, next to the original
  track. The player lets the viewer pick the track.
- Each detected speaker gets a cloned voice.
- Each translated line starts at the start time of its subtitle and fits its time.
- The background (music and effects) of the original audio stays under the new voices.
- A command line flag and an output option in the desktop app.

## Non-goals

- Lip sync.
- Two speakers that talk at the same time in one subtitle line. The line gets one voice.
- Commercial use. The XTTS-v2 weights allow only non-commercial use.
- Languages that XTTS-v2 does not speak. The app refuses to dub into them.

## Anti-goals

The feature fails, even if it works, when:

- the binary contains GPL code, or needs Python;
- the original speech is still loud under the new voice;
- a line is so fast that it is hard to follow, or a line falls into the next line.

## Approaches

The pilot chose each part in the conversation.

| Part | Considered | Chosen | Why |
|---|---|---|---|
| Voice engine | Piper (with espeak-ng), XTTS-v2, F5-TTS | XTTS-v2 only | Piper needs espeak-ng, which is GPL-3. F5-TTS does not speak Hungarian. XTTS-v2 speaks 17 languages, including Hungarian, and clones voices. |
| How to run it in Rust | Python helper, candle port, ONNX export with ONNX Runtime | ONNX export with ONNX Runtime | The pilot chose it. Everything stays in the binary. Community exports show that XTTS-v2 splits cleanly into ONNX parts. |
| ONNX Runtime crate | `onnxruntime`, `ort` | `ort` 2.0 | The `onnxruntime` crate has had no updates since 2021. `ort` is maintained and links ONNX Runtime statically. |

## Design

### New crate: `verbacue-dub`

The crate holds the dubbing steps. It depends on `verbacue-core`, `verbacue-media`
and `ort`. `verbacue-media` does the audio decoding, encoding and the new track.

### Pipeline

1. **Decode the audio.** `verbacue-media` decodes the chosen audio track to 44.1 kHz
   stereo.
2. **Separate the speech from the background.** The HTDemucs vocals model (ONNX)
   splits the audio into a speech stem and a background stem. The ONNX graph covers
   only the network. Rust code does the STFT, the chunks and the overlap-add.
3. **Find the speakers.** The XTTS-v2 speaker encoder makes one 512-value embedding
   for the speech of each subtitle line. The app groups lines with similar embeddings
   (cosine distance, agglomerative clustering). Each group is one speaker. This step
   reuses an XTTS-v2 part, so it needs no extra model.
4. **Collect a voice sample for each speaker.** The app takes the clearest lines of
   each speaker from the speech stem, up to about 10 seconds. The XTTS-v2
   conditioning encoder turns the sample into conditioning latents.
5. **Speak each translated line.**
   - Normalise the text: numbers become words. The first version does this for
     Hungarian and English. Other languages keep the digits.
   - Tokenise the text with the XTTS-v2 BPE vocabulary and the language token.
   - Run the GPT decoder step by step with a KV cache until it gives the stop code.
     Sampling uses the XTTS-v2 defaults.
   - Run the HiFi-GAN vocoder on the GPT latents. The result is 24 kHz audio.
6. **Fit each line to its time.** The app stretches the GPT latents in time before
   the vocoder. This changes the speed without a change in pitch. The limit is
   1.3 times faster. If the line is still too long, it may use the silence before the
   next line. If it still does not fit, the app makes it faster up to 1.5 times and
   logs a warning.
7. **Mix.** The app matches the loudness of each new line to the original speech of
   that line. Then it adds the new lines to the background stem.
8. **Write the track.** `verbacue-media` encodes the mix to AAC and writes an MKV. It
   uses the FFmpeg that is already in the binary, with its native AAC encoder (LGPL),
   so this step adds no new dependency. The MKV holds all original streams, the new
   audio track, and the subtitles as soft tracks (the same as `--embed`). The new track
   is the default track. It gets the language tag and the title "<Language> (AI
   voice)".

### Lines that the app does not speak

- Lines without speech, such as "[music]" or "♪", stay silent.
- If the GPT decoder does not give the stop code, or the audio is more than twice the
  expected length, the app tries again once with another seed.
- If the second try fails too, or a speaker has less than 3 seconds of clean speech,
  the app uses the studio voice for that line and logs a warning.

### Long films and multichannel audio

- HTDemucs runs on chunks of the audio, so memory stays low. The stems go to temporary
  files in the cache folder, not to memory.
- The first version mixes multichannel audio (5.1) down to stereo. The new track is
  stereo.

### Models

| Model | Source | Size (about) |
|---|---|---|
| XTTS-v2 conditioning encoder, speaker encoder, GPT decoder, HiFi-GAN vocoder, embedding tables, BPE vocabulary | Our own ONNX export of `coqui/XTTS-v2` | 1.9 GB (FP32), less with an INT8 GPT |
| HTDemucs vocals | ONNX export of `htdemucs_ft` vocals | 170 MB |

`tools/xtts-export/` holds the export scripts. They are Python, for developers only.
A test compares the ONNX output with the PyTorch output.

The exported files need a download place. The app cannot convert the models itself,
because the conversion needs PyTorch. The files live in a Hugging Face repo under the
pilot's account, with the CPML licence attached.

### ONNX Runtime

`ort` 2.0 links ONNX Runtime statically. The app uses CUDA in the CUDA builds. The CPU
is the fallback.

XTTS-v2 runs on the CPU on macOS. CoreML gives `inf` in the conditioning encoder, refuses
the growing KV cache of the GPT, and is slower than the CPU for the vocoder. On an Apple
Silicon CPU, one short line takes about 2 seconds.

### Language

The voice speaks the target language of the translation: `--to` on the command line,
and the target language on the Choose screen of the desktop app. There is no separate
language setting for the voice. `verbacue dub` takes the language from the SRT file
name (`movie.hu.srt`), the same as `verbacue burn`.

XTTS-v2 speaks 17 languages. If the target language is not one of them:

- the command line stops before the transcription, with an error that lists the
  languages it can speak;
- the desktop app greys out the "Dubbed voice" option for that language and shows why.

### Settings

The command line and the desktop app have the same settings for dubbing.

| Setting | Values | Default | Command line | Desktop app |
|---|---|---|---|---|
| Dub the video | on, off | off | `--dub` | Output option "Dubbed voice" |
| Voice | `clone`, or a studio voice name | `clone` | `--dub-voice` | Settings, "Voice" list |
| Original speech | `remove` (separation), `lower` (lower the volume only, faster) | `remove` | `--dub-original remove\|lower` | Settings, "Original speech" switch |
| Speed limit | 1.0 to 1.5 | 1.3 | `--dub-speed-limit` | Settings, "Speed limit" slider |
| Voice model | `xtts-v2`, `xtts-v2-int8`, or a path | `xtts-v2` in a CUDA build, else `xtts-v2-int8` | `--dub-model` | Settings, "Voice model" list |

### Command line

- `verbacue movie.mkv --to hu --dub` runs the full pipeline. It writes the subtitles and
  `movie.hu.dub.mkv`. `--dub` works together with `--embed` and `--burn`.
- `verbacue dub movie.mkv movie.hu.srt` dubs with an existing SRT file. It skips the
  transcription and the translation, the same as `verbacue burn`. The user can correct
  the subtitles first and then dub.
- `verbacue voices` lists the studio voices.
- `verbacue models` lists, downloads and removes the new models, the same as the other
  models.

### Desktop app

- **Choose screen:** a third output option, "Dubbed voice", next to "Burned in" and
  "Separate SRT file". It writes the dubbed MKV and the subtitle files.
- **Settings page:** a new "Dubbing" section with the settings in the table above.
  The app saves them in `gui.toml`. Old `gui.toml` files without these keys still load,
  with the defaults.
- **Licence:** before the first download of XTTS-v2, a dialog shows the licence
  (non-commercial use only). The user must accept it. The command line asks the same
  question, or takes `--accept-model-licence`.
- **Convert screen:** the progress shows the new steps: "Separating the voices",
  "Finding the speakers", "Speaking line N of M", "Mixing".
- **Done screen:** the "Open" button opens the dubbed MKV.
- **Recent list:** it remembers the output mode. The old `burn` key still loads.

## Delivery

The pilot wants one pull request from `feature/dubbing` for the whole feature. The
steps below come in this order. Each step has one or more commits. Each commit builds
and passes the tests, so a reviewer can read the pull request one commit at a time.

1. **Export spike** (go or no-go). The export scripts and the parity test (Python,
   developers only). If the GPT decoder with the KV cache does not export with matching
   output, the work stops before the other steps, and the pilot decides the next step.
2. `verbacue-dub` with `ort` and XTTS-v2 inference with a studio voice. A hidden
   `verbacue speak` command writes a WAV file for tests.
3. Time fit, mix and the new audio track, with `--dub-original lower`. The command line
   flags `--dub`, `--dub-voice`, `--dub-speed-limit`, `--dub-model`, and the `dub` and
   `voices` commands.
4. HTDemucs separation and `--dub-original remove`.
5. Speaker groups and voice cloning (`--dub-voice clone`).
6. The desktop app: the output option, the settings, the licence dialog and the
   progress steps.

Each step adds its settings to the command line at once. The desktop app gets all
dubbing settings in step 6.

### Tests

- Unit tests cover the text normalisation, the tokeniser, the time fit, the mix and
  the speaker groups. They need no model.
- Tests that need the 1.9 GB model run only on a developer machine. They are marked
  `#[ignore]`, and CI does not download the model.

### Platforms and speed

- All release targets get the feature: macOS, Windows and Linux. The fully static
  Linux CPU binaries get it too, if `ort` links statically there. Otherwise those
  binaries leave the feature out with a cargo feature.
- If CoreML or CUDA cannot run a part of the model, that part runs on the CPU.
- The target speed on an Apple M-series Mac: a 90-minute film in less than 90 minutes.

## Left to the builder

- The exact ONNX input and output names.
- The clustering threshold. It needs tuning on real films.
- The chunk size and overlap for HTDemucs.
- Number to word rules for Hungarian and English. They are our own code, with no new
  dependency.

## Calls made by Claude (please check)

- The dubbed MKV also holds the subtitles as soft tracks.
- `verbacue dub` works with an existing SRT file.
- The "lower" mode for the original speech is a faster choice, without separation.
- Failed lines use the studio voice. Non-speech lines stay silent.
- The first version makes a stereo track only.
- Tests that need the model do not run in CI.

## Decisions of the pilot

1. Every public release build has the feature. Each user accepts the XTTS-v2 licence
   before the first download.
2. The exported ONNX files live in a Hugging Face repo under the pilot's account, with
   the CPML licence attached.
3. The export scripts use a separate Python from `uv` (installed with Homebrew).
