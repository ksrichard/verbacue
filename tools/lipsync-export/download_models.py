"""Downloads the source weights of the lip sync models to ckpt/. For developers only."""

import urllib.request
from pathlib import Path

from huggingface_hub import hf_hub_download, snapshot_download

CKPT = Path(__file__).parent / "ckpt"
YUNET = (
    "https://github.com/opencv/opencv_zoo/raw/main/models/face_detection_yunet/"
    "face_detection_yunet_2023mar.onnx"
)

CKPT.mkdir(exist_ok=True)
for f in ["musetalkV15/unet.pth", "musetalkV15/musetalk.json"]:
    hf_hub_download("TMElyralab/MuseTalk", f, local_dir=CKPT)
snapshot_download("madebyollin/taesd", local_dir=CKPT / "taesd",
                  allow_patterns=["config.json", "diffusion_pytorch_model.safetensors"])
snapshot_download("openai/whisper-tiny", local_dir=CKPT / "whisper-tiny",
                  allow_patterns=["config.json", "model.safetensors", "preprocessor_config.json"])
urllib.request.urlretrieve(YUNET, CKPT / "yunet.onnx")
print("done")
