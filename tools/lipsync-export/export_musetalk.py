"""Exports the lip sync models to ONNX for verbacue. For developers only.

The files go to out/, with the names that verbacue looks for:
- lipsync_whisper.onnx: Whisper-tiny encoder. Log-mel [1, 80, 3000] -> all hidden
  states [1, 1500, 5, 384].
- lipsync_vae_encoder.onnx, lipsync_vae_decoder.onnx: TAESD (MIT), the small
  autoencoder in the latent space of the SD VAE that MuseTalk uses. Image [B, 3, 256,
  256] in [-1, 1] <-> latents [B, 4, 32, 32]. It is about 15 times faster on a CPU.
- lipsync_unet.onnx (+ .data): MuseTalk 1.5 UNet (MIT), INT8 weights. Latents [B, 8, 32,
  32] and audio [B, 50, 384] -> latents [B, 4, 32, 32]. The graph adds the positional
  encoding and uses timestep 0, as MuseTalk does.
- lipsync_faces.onnx: YuNet (MIT) from the OpenCV model zoo, unchanged.

Run download_models.py first.
"""

import json
import math
import shutil
from pathlib import Path

import onnx
import torch
from diffusers import AutoencoderTiny, UNet2DConditionModel
from onnxruntime.quantization import QuantType, quantize_dynamic
from transformers import WhisperModel

CKPT = Path(__file__).parent / "ckpt"
OUT = Path(__file__).parent / "out"
OPSET = 17


def positional_encoding(d_model=384, max_len=50):
    pe = torch.zeros(max_len, d_model)
    position = torch.arange(0, max_len, dtype=torch.float).unsqueeze(1)
    div = torch.exp(torch.arange(0, d_model, 2).float() * (-math.log(10000.0) / d_model))
    pe[:, 0::2] = torch.sin(position * div)
    pe[:, 1::2] = torch.cos(position * div)
    return pe.unsqueeze(0)


class Unet(torch.nn.Module):
    def __init__(self):
        super().__init__()
        config = json.loads((CKPT / "musetalkV15/musetalk.json").read_text())
        self.model = UNet2DConditionModel(**config)
        weights = torch.load(CKPT / "musetalkV15/unet.pth", map_location="cpu")
        self.model.load_state_dict(weights)
        self.register_buffer("pe", positional_encoding())

    def forward(self, latents, audio):
        t = torch.zeros(latents.shape[0], dtype=torch.long)
        return self.model(latents, t, encoder_hidden_states=audio + self.pe).sample


class TinyEncoder(torch.nn.Module):
    def __init__(self, tiny):
        super().__init__()
        self.tiny = tiny

    def forward(self, image):
        # TAESD latents are already on the scale of SD latents times the scaling factor.
        return self.tiny.encode(image).latents


class TinyDecoder(torch.nn.Module):
    def __init__(self, tiny):
        super().__init__()
        self.tiny = tiny

    def forward(self, latents):
        return (self.tiny.decode(latents).sample / 2 + 0.5).clamp(0, 1)


class WhisperEncoder(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.encoder = WhisperModel.from_pretrained(CKPT / "whisper-tiny").encoder

    def forward(self, mel):
        states = self.encoder(mel, output_hidden_states=True).hidden_states
        return torch.stack(states, dim=2)


def export(module, inputs, names, outputs, path, dynamic):
    module.eval()
    with torch.no_grad():
        torch.onnx.export(
            module,
            inputs,
            str(path),
            input_names=names,
            output_names=outputs,
            dynamic_axes=dynamic,
            opset_version=OPSET,
            dynamo=False,
        )
    print("wrote", path, flush=True)


def main():
    OUT.mkdir(exist_ok=True)
    work = OUT / "work"
    work.mkdir(exist_ok=True)
    export(
        WhisperEncoder(),
        (torch.zeros(1, 80, 3000),),
        ["mel"],
        ["states"],
        OUT / "lipsync_whisper.onnx",
        None,
    )
    tiny = AutoencoderTiny.from_pretrained(CKPT / "taesd").eval()
    export(
        TinyEncoder(tiny),
        (torch.zeros(1, 3, 256, 256),),
        ["image"],
        ["latents"],
        OUT / "lipsync_vae_encoder.onnx",
        {"image": {0: "batch"}, "latents": {0: "batch"}},
    )
    export(
        TinyDecoder(tiny),
        (torch.zeros(1, 4, 32, 32),),
        ["latents"],
        ["image"],
        OUT / "lipsync_vae_decoder.onnx",
        {"latents": {0: "batch"}, "image": {0: "batch"}},
    )
    # Over 2 GB, so the FP32 weights go to external data, then to INT8 in one file.
    export(
        Unet(),
        (torch.zeros(1, 8, 32, 32), torch.zeros(1, 50, 384)),
        ["latents", "audio"],
        ["out"],
        work / "unet.onnx",
        {"latents": {0: "batch"}, "audio": {0: "batch"}, "out": {0: "batch"}},
    )
    quantize_dynamic(work / "unet.onnx", work / "unet_int8.onnx", weight_type=QuantType.QInt8,
                     use_external_data_format=True)
    model = onnx.load(work / "unet_int8.onnx", load_external_data=True)
    onnx.save_model(model, OUT / "lipsync_unet.onnx", save_as_external_data=True,
                    all_tensors_to_one_file=True, location="lipsync_unet.onnx.data")
    shutil.copy(CKPT / "yunet.onnx", OUT / "lipsync_faces.onnx")
    shutil.rmtree(work)
    print("done", flush=True)


if __name__ == "__main__":
    main()
