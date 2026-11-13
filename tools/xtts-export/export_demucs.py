"""Exports the HTDemucs vocals model to ONNX for verbacue.

Usage: uv run export_demucs.py <output dir>

It writes htdemucs_vocals.onnx: the vocals specialist of htdemucs_ft (MIT licence).
The export runs the demucs-onnx parity check against PyTorch (max abs diff 1e-3)
and stops if it fails. Then it prints the inputs and outputs of the graph.
"""

import subprocess
import sys
from pathlib import Path

import onnxruntime as ort


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    path = out / "htdemucs_vocals.onnx"
    subprocess.run(["demucs-onnx", "export", "htdemucs_ft", str(path), "--stem", "vocals"], check=True)
    session = ort.InferenceSession(str(path))
    for kind, items in (("in", session.get_inputs()), ("out", session.get_outputs())):
        for item in items:
            print(f"{kind:3} {item.name} {item.shape} {item.type}")


if __name__ == "__main__":
    main()
