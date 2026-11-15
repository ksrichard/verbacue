"""Writes fixtures/clone.safetensors for the Rust voice cloning tests.

Usage: uv run clone_fixtures.py <export dir>

Run parity.py first: it writes fixtures/reference.wav. This script reads that file
as Rust does, then saves each step of the cloning: the resampled audio, both mel
spectrograms, and the outputs of conditioning.onnx and speaker.onnx on the CPU.
"""

import json
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
import soundfile as sf
import torch
import torchaudio
from safetensors.numpy import save_file

from parity import cond_mel, speaker_mel


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    cfg = json.loads((out / "config.json").read_text())
    audio, rate = sf.read(out / "fixtures/reference.wav", dtype="float32")
    assert rate == 24000
    audio = torch.from_numpy(audio)[None]
    a22 = torchaudio.functional.resample(audio, 24000, 22050)[0].numpy()
    a16 = torchaudio.functional.resample(audio, 24000, 16000)[0].numpy()
    cm = cond_mel(a22[: 22050 * 6].astype(np.float64), cfg).astype(np.float32)
    sm = speaker_mel(a16[: 16000 * 30].astype(np.float64), cfg).astype(np.float32)
    cond = ort.InferenceSession(str(out / "conditioning.onnx")).run(None, {"mel": cm[None]})[0]
    spk = ort.InferenceSession(str(out / "speaker.onnx")).run(None, {"mel": sm[None]})[0]
    save_file(
        {
            "audio_22k": a22,
            "audio_16k": a16,
            "cond_mel": cm,
            "speaker_mel": sm,
            "cond_latents": cond.reshape(32, 1024),
            "speaker_emb": spk.reshape(512),
        },
        str(out / "fixtures/clone.safetensors"),
    )
    print("wrote", out / "fixtures/clone.safetensors")


if __name__ == "__main__":
    main()
