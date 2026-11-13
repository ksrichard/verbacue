"""Exports XTTS-v2 to ONNX graphs and tables that verbacue reads from Rust.

Usage: uv run export_xtts.py <checkpoint dir> <output dir>

The checkpoint dir holds the files of the Hugging Face repo coqui/XTTS-v2
(model.pth, config.json, vocab.json, speakers_xtts.pth).

The output dir gets:
  conditioning.onnx      mel [1, 80, T] -> cond_latents [1, 32, 1024]
  speaker.onnx           mel [1, 64, T] (16 kHz) -> speaker_emb [1, 512, 1]
  gpt.onnx               embeds [1, T, 1024] + KV cache -> logits, latents, new KV cache
  gpt_int8.onnx          gpt.onnx with INT8 weights
  vocoder.onnx           latents [1, T, 1024] + speaker_emb -> wav [1, N] at 24 kHz
  embeddings.safetensors the token and position embeddings that Rust adds up
  speakers.safetensors   the studio voices
  vocab.json             the BPE tokenizer, unchanged
  config.json            token ids, limits and the mel settings
"""

import json
import math
import shutil
import sys
from pathlib import Path

import torch
from safetensors.torch import save_file
from torch import nn

from TTS.tts.configs.xtts_config import XttsConfig
from TTS.tts.models.xtts import Xtts

OPSET = 17


def load_model(ckpt: Path) -> Xtts:
    config = XttsConfig()
    config.load_json(str(ckpt / "config.json"))
    model = Xtts.init_from_config(config)
    model.load_checkpoint(config, checkpoint_dir=str(ckpt), eval=True, use_deepspeed=False)
    model.float()
    model.eval()  # Xtts.eval() returns None, so it cannot be chained.
    return model


class Conditioning(nn.Module):
    """The GPT conditioning encoder and the perceiver, for one mel chunk."""

    def __init__(self, gpt):
        super().__init__()
        self.gpt = gpt

    def forward(self, mel):
        return self.gpt.get_style_emb(mel).transpose(1, 2)


class Speaker(nn.Module):
    """The speaker encoder of the vocoder, without its spectrogram.

    ONNX cannot export the torchaudio STFT, so Rust makes the mel spectrogram
    (pre-emphasis, then the settings in config.json "speaker_mel").
    """

    def __init__(self, decoder):
        super().__init__()
        self.encoder = decoder.speaker_encoder

    def forward(self, mel):
        self.encoder.use_torch_spec = False
        try:
            return self.encoder.forward(mel, l2_norm=True).unsqueeze(-1)
        finally:
            self.encoder.use_torch_spec = True


class Gpt(nn.Module):
    """The XTTS-v2 GPT-2 with an explicit KV cache, so one graph does the prefill and the steps.

    Rust adds the token and position embeddings, so the graph takes embeddings. Query i
    of T new positions may see the P cached keys and the new keys up to i.
    """

    def __init__(self, gpt):
        super().__init__()
        self.blocks = gpt.gpt.h
        self.ln_f = gpt.gpt.ln_f
        self.final_norm = gpt.final_norm
        self.mel_head = gpt.mel_head
        self.heads = gpt.heads
        self.dim = gpt.model_dim

    def attention(self, block, x, past_k, past_v):
        b, t, d = x.shape
        hd = d // self.heads
        q, k, v = block.attn.c_attn(x).split(d, dim=2)
        q = q.view(b, t, self.heads, hd).transpose(1, 2)
        k = torch.cat([past_k, k.view(b, t, self.heads, hd).transpose(1, 2)], dim=2)
        v = torch.cat([past_v, v.view(b, t, self.heads, hd).transpose(1, 2)], dim=2)
        p = past_k.shape[2]
        scores = q @ k.transpose(-1, -2) / math.sqrt(hd)
        rows = torch.arange(t).unsqueeze(1) + p
        cols = torch.arange(p + t).unsqueeze(0)
        scores = scores.masked_fill(cols > rows, float("-inf"))
        out = (scores.softmax(dim=-1) @ v).transpose(1, 2).reshape(b, t, d)
        return block.attn.c_proj(out), k, v

    def forward(self, embeds, *past):
        x = embeds
        present = []
        for i, block in enumerate(self.blocks):
            a, k, v = self.attention(block, block.ln_1(x), past[2 * i], past[2 * i + 1])
            x = x + a
            x = x + block.mlp(block.ln_2(x))
            present += [k, v]
        latents = self.final_norm(self.ln_f(x))
        logits = self.mel_head(latents[:, -1])
        return (logits, latents, *present)


class Vocoder(nn.Module):
    def __init__(self, decoder):
        super().__init__()
        self.decoder = decoder

    def forward(self, latents, speaker_emb):
        return self.decoder(latents, g=speaker_emb).squeeze(1)


def export(module, args, path, inputs, outputs, axes):
    torch.onnx.export(
        module,
        args,
        str(path),
        input_names=inputs,
        output_names=outputs,
        dynamic_axes=axes,
        opset_version=OPSET,
        do_constant_folding=True,
        dynamo=False,
    )
    print("wrote", path)


def kv_names(layers, prefix):
    return [f"{prefix}_{kind}_{i}" for i in range(layers) for kind in ("k", "v")]


def export_gpt(gpt, out: Path):
    wrapper = Gpt(gpt).eval()
    layers = len(wrapper.blocks)
    hd = wrapper.dim // wrapper.heads
    embeds = torch.randn(1, 5, wrapper.dim)
    past = [torch.zeros(1, wrapper.heads, 3, hd) for _ in range(2 * layers)]
    past_in = kv_names(layers, "past")
    present_out = kv_names(layers, "present")
    axes = {"embeds": {1: "t"}, "latents": {1: "t"}}
    axes |= {n: {2: "p"} for n in past_in}
    axes |= {n: {2: "p_t"} for n in present_out}
    export(
        wrapper,
        (embeds, *past),
        out / "gpt.onnx",
        ["embeds", *past_in],
        ["logits", "latents", *present_out],
        axes,
    )


def quantize(out: Path):
    from onnxruntime.quantization import QuantType, quantize_dynamic

    quantize_dynamic(
        str(out / "gpt.onnx"),
        str(out / "gpt_int8.onnx"),
        weight_type=QuantType.QInt8,
        op_types_to_quantize=["MatMul", "Gemm"],
        # Per-tensor scales made one Hungarian test line 45 % longer; per-channel stays within 10 %.
        per_channel=True,
    )
    print("wrote", out / "gpt_int8.onnx")


def save_tables(gpt, out: Path):
    tensors = {
        "text_embedding": gpt.text_embedding.weight,
        "text_pos_embedding": gpt.text_pos_embedding.emb.weight,
        "mel_embedding": gpt.mel_embedding.weight,
        "mel_pos_embedding": gpt.mel_pos_embedding.emb.weight,
    }
    save_file({k: v.detach().contiguous().float() for k, v in tensors.items()}, str(out / "embeddings.safetensors"))
    print("wrote", out / "embeddings.safetensors")


def save_speakers(ckpt: Path, out: Path):
    speakers = torch.load(ckpt / "speakers_xtts.pth", map_location="cpu", weights_only=True)
    tensors = {}
    for name, voice in speakers.items():
        tensors[f"{name}.cond_latents"] = voice["gpt_cond_latent"].float().reshape(32, 1024).contiguous()
        tensors[f"{name}.speaker_emb"] = voice["speaker_embedding"].float().reshape(512).contiguous()
    save_file(tensors, str(out / "speakers.safetensors"))
    print("wrote", out / "speakers.safetensors", f"({len(speakers)} voices)")


def save_config(model, out: Path):
    gpt = model.gpt
    se = model.hifigan_decoder.speaker_encoder_audio_config
    config = {
        "start_text_token": gpt.start_text_token,
        "stop_text_token": gpt.stop_text_token,
        "start_audio_token": gpt.start_audio_token,
        "stop_audio_token": gpt.stop_audio_token,
        "max_gen_mel_tokens": model.args.gpt_max_audio_tokens - 3,
        "max_text_tokens": model.args.gpt_max_text_tokens,
        "layers": gpt.layers,
        "heads": gpt.heads,
        "model_dim": gpt.model_dim,
        "char_limits": model.tokenizer.char_limits,
        "output_sample_rate": model.hifigan_decoder.output_sample_rate,
        "cond_mel": {
            "sample_rate": 22050,
            "n_fft": 2048,
            "hop_length": 256,
            "win_length": 1024,
            "n_mels": 80,
            "f_min": 0,
            "f_max": 8000,
            "mel_norm": "slaney",
            "log_clamp": 1e-5,
            "chunk_seconds": 6,
            "mel_stats": model.mel_stats.float().tolist(),
        },
        "speaker_mel": {
            "sample_rate": se["sample_rate"],
            "preemphasis": se["preemphasis"],
            "n_fft": se["fft_size"],
            "win_length": se["win_length"],
            "hop_length": se["hop_length"],
            "n_mels": se["num_mels"],
            "window": "hamming",
            "mel_norm": None,
            "log_input": bool(model.hifigan_decoder.speaker_encoder.log_input),
        },
        "sampling": {"temperature": 0.75, "top_k": 50, "top_p": 0.85, "repetition_penalty": 10.0},
    }
    (out / "config.json").write_text(json.dumps(config, indent=2, ensure_ascii=False))
    print("wrote", out / "config.json")


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    ckpt, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    model = load_model(ckpt)
    gpt = model.gpt

    with torch.no_grad():
        export(
            Conditioning(gpt).eval(),
            (torch.randn(1, 80, 400),),
            out / "conditioning.onnx",
            ["mel"],
            ["cond_latents"],
            {"mel": {2: "t"}},
        )
        export(
            Speaker(model.hifigan_decoder).eval(),
            (torch.rand(1, 64, 300),),
            out / "speaker.onnx",
            ["mel"],
            ["speaker_emb"],
            {"mel": {2: "t"}},
        )
        export(
            Vocoder(model.hifigan_decoder).eval(),
            (torch.randn(1, 40, gpt.model_dim), torch.randn(1, 512, 1)),
            out / "vocoder.onnx",
            ["latents", "speaker_emb"],
            ["wav"],
            {"latents": {1: "t"}, "wav": {1: "n"}},
        )
        export_gpt(gpt, out)
    quantize(out)
    save_tables(gpt, out)
    save_speakers(ckpt, out)
    save_config(model, out)
    shutil.copy(ckpt / "vocab.json", out / "vocab.json")


if __name__ == "__main__":
    main()
