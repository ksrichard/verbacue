# Voice model export

These scripts turn XTTS-v2 into ONNX files that verbacue runs from Rust. They are for
developers only. The verbacue binary does not use Python; it downloads the exported
files on first use.

## Run

You need [uv](https://docs.astral.sh/uv/) (`brew install uv`) and about 8 GB of free
disk space.

```sh
cd tools/xtts-export
uv sync
uv run python -c "from huggingface_hub import snapshot_download; \
  snapshot_download('coqui/XTTS-v2', local_dir='ckpt', \
  allow_patterns=['model.pth','config.json','vocab.json','speakers_xtts.pth','LICENSE.txt'])"
uv run export_xtts.py ckpt out
uv run parity.py ckpt out
```

`parity.py` must end with `PARITY OK`. It checks the export against PyTorch:

- The numpy mel spectrograms match torchaudio. These functions are the reference for
  the Rust code.
- The conditioning latents, the speaker embedding, the GPT latents and the vocoder
  audio differ by less than 0.001.
- The FP32 GPT gives the same codes as PyTorch for 5 test lines.
- The INT8 GPT gives audio within 30 % of the FP32 length.

It also writes `out/fixtures/`: `tokens.json` for the Rust tokenizer test, and WAV
files to listen to.

## The files

| File | What it does |
|---|---|
| `conditioning.onnx` | Turns a 22.05 kHz mel spectrogram of a voice sample into the GPT conditioning latents. |
| `speaker.onnx` | Turns a 16 kHz mel spectrogram into the speaker embedding for the vocoder. |
| `gpt.onnx`, `gpt_int8.onnx` | GPT-2 with a KV cache. One graph does the first pass and each next step. |
| `vocoder.onnx` | HiFi-GAN: GPT latents and the speaker embedding to 24 kHz audio. |
| `embeddings.safetensors` | The token and position embeddings. Rust adds them up. |
| `speakers.safetensors` | The 58 studio voices of XTTS-v2. |
| `vocab.json` | The BPE tokenizer. |
| `config.json` | Token ids, limits, the mel settings and the sampling settings. |

ONNX cannot export the torchaudio STFT, so the mel spectrograms are not in the graphs.

## Voice separation

```sh
uv run export_demucs.py out
```

This writes `htdemucs_vocals.onnx`, the vocals model of `htdemucs_ft` (MIT licence).
The input `mix` is 7.8 s of 44.1 kHz stereo audio, `[1, 2, 343980]`. The output
`stems` is `[1, 4, 2, 343980]`, in the order drums, bass, other, vocals. Use only
index 3: the other sources of this model are not good.

## Publish

```sh
uv run hf auth login
uv run publish.py out <user>/verbacue-voice
```

This uploads all files to Hugging Face and writes `publish.md` with the URL, SHA256
and size of each file.

## Licence

The XTTS-v2 weights are under the
[Coqui Public Model License](https://coqui.ai/cpml): non-commercial use only. The
exported files are under the same licence.
