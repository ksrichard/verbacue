# Lip sync model export

These scripts turn the lip sync models into the ONNX files that verbacue runs from
Rust. They are for developers only. The verbacue binary does not use Python.

The files are:

| File | Model | Licence |
|---|---|---|
| `lipsync_unet.onnx` (+ `.data`) | MuseTalk 1.5 UNet, INT8 weights | MIT |
| `lipsync_vae_encoder.onnx`, `lipsync_vae_decoder.onnx` | TAESD, the small autoencoder for the SD latent space | MIT |
| `lipsync_whisper.onnx` | Whisper-tiny encoder, all hidden states | MIT |
| `lipsync_faces.onnx` | YuNet face finder (OpenCV model zoo) | MIT |

MuseTalk uses the SD VAE. TAESD works in the same latent space and is about 15 times
faster on a CPU; the INT8 UNet is about 25 % faster than FP32. Together, one frame
takes about 0.3 s on an Apple Silicon CPU instead of 1.35 s.

## Run

You need [uv](https://docs.astral.sh/uv/) and about 8 GB of free disk space.

```sh
cd tools/lipsync-export
uv sync
uv run download_models.py
uv run export_musetalk.py
```

The files go to `out/`. Copy them next to a local voice model (the folder for
`--dub-model`), or upload them to the `lipsync/` folder of the model repository and
update the hashes in `crates/verbacue-core/src/models.rs`.
