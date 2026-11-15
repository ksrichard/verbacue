"""Checks that the ONNX export of XTTS-v2 gives the same result as PyTorch.

Usage: uv run parity.py <checkpoint dir> <export dir>

The mel functions below use only numpy. They are the reference for the Rust code in
crates/verbacue-dub/src/mel.rs. The script also writes fixtures/tokens.json for the
Rust tokenizer test, and a few WAV files to listen to.
"""

import json
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
import soundfile as sf
import torch
import torchaudio
from safetensors.numpy import load_file

from export_xtts import load_model

SENTENCES = [
    ("hu", "Jó napot kívánok, hogy van?"),
    ("hu", "Van 3 macskám és 21 kutyám."),
    ("hu", "1956-ban születtem, most 2026 van."),
    ("hu", "Nem tudom, Péter. Talán holnap!"),
    ("hu", "Az árvíztűrő tükörfúrógép."),
    ("en", "Hello, how are you today?"),
    ("en", "I have 3 cats and 21 dogs."),
    ("en", "It was 1956 when I was born."),
    ("en", "Mr. Smith isn't here, is he?"),
    ("en", "Wait... what?!"),
    ("de", "Guten Morgen, wie geht es dir?"),
    ("de", "Ich habe 5 Äpfel."),
    ("fr", "Je ne sais pas, peut-être demain."),
    ("es", "¿Dónde está la biblioteca?"),
    ("it", "Non lo so, forse domani."),
    ("pl", "Dzień dobry, jak się masz?"),
    ("ru", "Привет, как дела?"),
    ("cs", "Dobrý den, jak se máte?"),
    ("tr", "Merhaba, nasılsın?"),
    ("nl", "Goedemorgen, hoe gaat het?"),
]
PARITY = [("hu", "Jó napot kívánok, hogy van?"), ("hu", "Nem tudom, Péter. Talán holnap!"),
          ("en", "Hello, how are you today?"), ("en", "It was a dark and stormy night."),
          ("hu", "Az árvíztűrő tükörfúrógép.")]
TOLERANCE = 1e-3
VOICE = "Ana Florence"


# --- mel spectrograms (the reference for Rust) ---

def hz_to_mel(f):
    return 2595.0 * np.log10(1.0 + f / 700.0)


def mel_to_hz(m):
    return 700.0 * (10.0 ** (m / 2595.0) - 1.0)


def mel_filters(sample_rate, n_fft, n_mels, f_min, f_max, slaney_norm):
    """Triangular HTK-scale filters, as torchaudio.functional.melscale_fbanks."""
    n_freqs = n_fft // 2 + 1
    all_freqs = np.linspace(0, sample_rate // 2, n_freqs)
    m_pts = np.linspace(hz_to_mel(f_min), hz_to_mel(f_max), n_mels + 2)
    f_pts = mel_to_hz(m_pts)
    f_diff = f_pts[1:] - f_pts[:-1]
    slopes = f_pts[None, :] - all_freqs[:, None]
    down = -slopes[:, :-2] / f_diff[:-1]
    up = slopes[:, 2:] / f_diff[1:]
    fb = np.maximum(0.0, np.minimum(down, up))
    if slaney_norm:
        fb *= 2.0 / (f_pts[2 : n_mels + 2] - f_pts[:n_mels])
    return fb  # [n_freqs, n_mels]


def power_spectrogram(x, n_fft, hop, win_length, window):
    """torch.stft with center=True, reflect padding, and the window centred in n_fft."""
    if window == "hann":
        w = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(win_length) / win_length)
    else:
        w = 0.54 - 0.46 * np.cos(2 * np.pi * np.arange(win_length) / win_length)
    left = (n_fft - win_length) // 2
    win = np.zeros(n_fft)
    win[left : left + win_length] = w
    pad = n_fft // 2
    x = np.pad(x, (pad, pad), mode="reflect")
    frames = 1 + (len(x) - n_fft) // hop
    idx = np.arange(n_fft)[None, :] + hop * np.arange(frames)[:, None]
    spec = np.fft.rfft(x[idx] * win[None, :], axis=1)
    return (spec.real**2 + spec.imag**2).T  # [n_freqs, frames]


def cond_mel(audio_22k, cfg):
    c = cfg["cond_mel"]
    p = power_spectrogram(audio_22k, c["n_fft"], c["hop_length"], c["win_length"], "hann")
    fb = mel_filters(c["sample_rate"], c["n_fft"], c["n_mels"], c["f_min"], c["f_max"], True)
    mel = np.log(np.maximum(fb.T @ p, c["log_clamp"]))
    return mel / np.array(c["mel_stats"])[:, None]


def speaker_mel(audio_16k, cfg):
    c = cfg["speaker_mel"]
    x = np.concatenate([[audio_16k[1]], audio_16k])
    x = x[1:] - c["preemphasis"] * x[:-1]
    p = power_spectrogram(x, c["n_fft"], c["hop_length"], c["win_length"], c["window"])
    fb = mel_filters(c["sample_rate"], c["n_fft"], c["n_mels"], 0.0, float(c["sample_rate"] // 2), False)
    return fb.T @ p


# --- the ONNX pipeline (the reference for Rust) ---

class OnnxXtts:
    def __init__(self, out: Path, gpt_file="gpt.onnx"):
        opts = ort.SessionOptions()
        self.cond = ort.InferenceSession(str(out / "conditioning.onnx"), opts)
        self.speaker = ort.InferenceSession(str(out / "speaker.onnx"), opts)
        self.gpt = ort.InferenceSession(str(out / gpt_file), opts)
        self.vocoder = ort.InferenceSession(str(out / "vocoder.onnx"), opts)
        self.emb = load_file(str(out / "embeddings.safetensors"))
        self.cfg = json.loads((out / "config.json").read_text())

    def prefix(self, cond_latents, tokens):
        c = self.cfg
        text = np.array([c["start_text_token"], *tokens, c["stop_text_token"]])
        t = self.emb["text_embedding"][text] + self.emb["text_pos_embedding"][: len(text)]
        m = self.emb["mel_embedding"][c["start_audio_token"]] + self.emb["mel_pos_embedding"][0]
        return np.concatenate([cond_latents.reshape(32, -1), t, m[None]], axis=0)[None].astype(np.float32)

    def generate(self, cond_latents, tokens, greedy=True, max_codes=None):
        """Greedy decoding with the HF repetition penalty. Returns the codes and the latents."""
        c = self.cfg
        layers, heads, dim = c["layers"], c["heads"], c["model_dim"]
        max_codes = max_codes or c["max_gen_mel_tokens"]
        embeds = self.prefix(cond_latents, tokens)
        past = {f"past_{k}_{i}": np.zeros((1, heads, 0, dim // heads), np.float32)
                for i in range(layers) for k in ("k", "v")}
        # HF generate sees a prefix of id 1 (one per prefix position) and the start code.
        seen = {1, c["start_audio_token"]}
        penalty = c["sampling"]["repetition_penalty"]
        codes, latents = [], []
        for step in range(max_codes):
            outs = self.gpt.run(None, {"embeds": embeds, **past})
            logits, lat = outs[0][0].copy(), outs[1][0, -1]
            latents.append(lat)
            for tok in seen:
                logits[tok] = logits[tok] * penalty if logits[tok] < 0 else logits[tok] / penalty
            code = int(np.argmax(logits))
            codes.append(code)
            if code == c["stop_audio_token"]:
                break
            seen.add(code)
            past = {name.replace("present", "past"): v
                    for name, v in zip([o.name for o in self.gpt.get_outputs()][2:], outs[2:])}
            embeds = (self.emb["mel_embedding"][code] + self.emb["mel_pos_embedding"][step + 1])[None, None]
            embeds = embeds.astype(np.float32)
        return codes, np.stack(latents)[None]

    def vocode(self, latents, speaker_emb):
        return self.vocoder.run(None, {"latents": latents.astype(np.float32),
                                       "speaker_emb": speaker_emb.reshape(1, 512, 1).astype(np.float32)})[0][0]


def check(name, a, b, tol=TOLERANCE):
    diff = float(np.max(np.abs(np.asarray(a, np.float64) - np.asarray(b, np.float64))))
    status = "OK" if diff < tol else "FAIL"
    print(f"{status:4} {name}: max abs diff {diff:.2e}")
    return diff < tol


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    ckpt, out = Path(sys.argv[1]), Path(sys.argv[2])
    torch.manual_seed(0)
    model = load_model(ckpt)
    onnx = OnnxXtts(out)
    cfg = onnx.cfg
    ok = True

    # Token fixture for Rust.
    fixtures = out / "fixtures"
    fixtures.mkdir(exist_ok=True)
    cases = []
    for lang, text in SENTENCES:
        clean = model.tokenizer.preprocess_text(text.strip().lower(), lang)
        cases.append({"lang": lang, "text": text, "clean": clean,
                      "ids": model.tokenizer.encode(text.strip().lower(), lang)})
    (fixtures / "tokens.json").write_text(json.dumps(cases, ensure_ascii=False, indent=1))
    print("wrote", fixtures / "tokens.json")

    # A reference voice: PyTorch speech from a studio voice.
    spk = torch.load(ckpt / "speakers_xtts.pth", map_location="cpu", weights_only=True)[VOICE]
    with torch.no_grad():
        ref = model.inference("This is a test of the voice cloning. It needs a few seconds of clean speech.",
                              "en", spk["gpt_cond_latent"], spk["speaker_embedding"], do_sample=False)["wav"]
    ref = torch.tensor(ref)[None]
    sf.write(fixtures / "reference.wav", ref[0].numpy(), 24000)

    # Mel spectrograms and both encoders.
    audio22 = torchaudio.functional.resample(ref, 24000, 22050)[:, : 22050 * 6]
    audio16 = torchaudio.functional.resample(ref, 24000, 16000)
    mel_np = cond_mel(audio22[0].numpy().astype(np.float64), cfg)
    from TTS.tts.models.xtts import wav_to_mel_cloning
    mel_t = wav_to_mel_cloning(audio22, mel_norms=model.mel_stats, n_fft=2048, hop_length=256, win_length=1024,
                               power=2, normalized=False, sample_rate=22050, f_min=0, f_max=8000, n_mels=80)[0]
    ok &= check("cond mel (numpy vs torchaudio)", mel_np, mel_t.numpy(), 1e-3)
    np.save(fixtures / "cond_mel.npy", mel_np.astype(np.float32))

    with torch.no_grad():
        cond_t = model.get_gpt_cond_latents(ref, 24000, length=6, chunk_length=6)
        spk_t = model.get_speaker_embedding(ref, 24000)
        sm_t = model.hifigan_decoder.speaker_encoder.torch_spec(audio16)[0]
    cond_o = onnx.cond.run(None, {"mel": mel_np[None].astype(np.float32)})[0]
    ok &= check("conditioning latents", cond_o, cond_t.numpy())
    sm_np = speaker_mel(audio16[0].numpy().astype(np.float64), cfg)
    ok &= check("speaker mel (numpy vs torchaudio, relative)", sm_np / (np.abs(sm_t.numpy()).max()),
                sm_t.numpy() / np.abs(sm_t.numpy()).max(), 1e-4)
    spk_o = onnx.speaker.run(None, {"mel": sm_np[None].astype(np.float32)})[0]
    ok &= check("speaker embedding", spk_o, spk_t.numpy())

    # GPT codes, latents and audio, with a studio voice.
    cond = spk["gpt_cond_latent"]
    emb = spk["speaker_embedding"]
    int8 = OnnxXtts(out, "gpt_int8.onnx")
    for i, (lang, text) in enumerate(PARITY):
        tokens = model.tokenizer.encode(text.strip().lower(), lang)
        with torch.no_grad():
            codes_t = model.gpt.generate(cond_latents=cond, text_inputs=torch.IntTensor(tokens)[None],
                                         do_sample=False, num_beams=1, repetition_penalty=10.0,
                                         length_penalty=1.0, output_attentions=False)[0].tolist()
            codes_t = codes_t[: codes_t.index(1025) + 1] if 1025 in codes_t else codes_t
            lat_t = model.gpt(torch.IntTensor(tokens)[None], torch.tensor([len(tokens)]),
                              torch.tensor([codes_t]), torch.tensor([len(codes_t) * 1024]),
                              cond_latents=cond, return_attentions=False, return_latent=True)
            wav_t = model.hifigan_decoder(lat_t, g=emb)[0, 0].numpy()
        codes_o, lat_o = onnx.generate(cond.numpy(), tokens)
        same = codes_o == codes_t
        print(f"{'OK' if same else 'FAIL':4} codes '{text}': {len(codes_o)} onnx, {len(codes_t)} torch")
        ok &= same
        if same:
            ok &= check(f"latents '{text}'", lat_o, lat_t.numpy())
            wav_o = onnx.vocode(lat_o, emb.numpy())
            ok &= check(f"vocoder '{text}'", wav_o, wav_t)
            sf.write(fixtures / f"onnx_{i}.wav", wav_o, 24000)
        codes_q, lat_q = int8.generate(cond.numpy(), tokens)
        ratio = len(codes_q) / max(len(codes_t), 1)
        print(f"{'OK' if 0.7 < ratio < 1.3 else 'FAIL':4} int8 length '{text}': {len(codes_q)} vs {len(codes_t)}")
        ok &= 0.7 < ratio < 1.3
        sf.write(fixtures / f"int8_{i}.wav", int8.vocode(lat_q, emb.numpy()), 24000)

    print("PARITY OK" if ok else "PARITY FAILED")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
