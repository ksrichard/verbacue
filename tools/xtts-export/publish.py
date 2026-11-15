"""Uploads the exported models to Hugging Face and writes publish.md.

Usage: uv run publish.py <export dir> <repo id>

It needs a login with write access (`uv run hf auth login`). The repo gets two
folders: xtts-v2/ (CPML licence) and htdemucs/ (MIT licence). publish.md lists the
URL at the upload commit, the SHA256 and the size of each file, for
crates/verbacue-core/src/models.rs.
"""

import hashlib
import sys
from pathlib import Path

from huggingface_hub import HfApi

XTTS_FILES = [
    "conditioning.onnx",
    "speaker.onnx",
    "gpt.onnx",
    "gpt_int8.onnx",
    "vocoder.onnx",
    "embeddings.safetensors",
    "speakers.safetensors",
    "vocab.json",
    "config.json",
]
DEMUCS_FILES = ["htdemucs_vocals.onnx"]

CARD = """---
license: other
license_name: coqui-public-model-license
license_link: https://coqui.ai/cpml
tags: [onnx, text-to-speech, voice-cloning, source-separation]
---

# verbacue voice models

ONNX exports for the dubbing feature of [verbacue](https://github.com/ksrichard/verbacue).
The scripts that made them are in `tools/xtts-export/` of that repository.

| Folder | Source | Licence |
|---|---|---|
| `xtts-v2/` | [coqui/XTTS-v2](https://huggingface.co/coqui/XTTS-v2) | [Coqui Public Model License](https://coqui.ai/cpml): non-commercial use only |
| `htdemucs/` | `htdemucs_ft` vocals model of [Demucs](https://github.com/adefossez/demucs) | MIT |

The XTTS-v2 files are a derived work of XTTS-v2, under the same licence. See
`xtts-v2/LICENSE.txt`.
"""


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    out, repo = Path(sys.argv[1]), sys.argv[2]
    api = HfApi()
    api.create_repo(repo, repo_type="model", exist_ok=True)
    stage = out / "publish"
    (stage / "xtts-v2").mkdir(parents=True, exist_ok=True)
    (stage / "htdemucs").mkdir(parents=True, exist_ok=True)
    for name in XTTS_FILES:
        link = stage / "xtts-v2" / name
        link.unlink(missing_ok=True)
        link.hardlink_to(out / name)
    for name in DEMUCS_FILES:
        link = stage / "htdemucs" / name
        link.unlink(missing_ok=True)
        link.hardlink_to(out / name)
    (stage / "xtts-v2" / "LICENSE.txt").write_bytes(Path("ckpt/LICENSE.txt").read_bytes())
    (stage / "README.md").write_text(CARD)
    commit = api.upload_folder(repo_id=repo, folder_path=str(stage), commit_message="Upload the verbacue voice models")
    rev = commit.oid

    rows = []
    for folder, names in (("xtts-v2", XTTS_FILES), ("htdemucs", DEMUCS_FILES)):
        for name in names:
            path = stage / folder / name
            url = f"https://huggingface.co/{repo}/resolve/{rev}/{folder}/{name}"
            rows.append(f"| `{folder}/{name}` | {url} | `{sha256(path)}` | {path.stat().st_size} |")
    Path("publish.md").write_text(
        "# Published voice models\n\n"
        f"Repo: https://huggingface.co/{repo}, commit `{rev}`.\n\n"
        "| File | URL | SHA256 | Size (bytes) |\n|---|---|---|---|\n" + "\n".join(rows) + "\n"
    )
    print("wrote publish.md")


if __name__ == "__main__":
    main()
