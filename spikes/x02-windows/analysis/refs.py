"""Compare Windows and macOS Chromium canvas references and look for LCD (subpixel) text.

usage: uv run --with numpy --with pillow refs.py <windows canvas dir> <macos canvas dir>
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def load(p):
    return np.asarray(Image.open(p).convert("RGBA")).astype(np.int16)


def chroma_stats(img):
    """Pixels whose RGB channels disagree by more than 2 levels: colour fringes on grey text."""
    rgb = img[..., :3]
    spread = rgb.max(axis=2) - rgb.min(axis=2)
    return int((spread > 2).sum()), int(spread.max())


def main():
    win, mac = Path(sys.argv[1]), Path(sys.argv[2])
    rows = []
    for ref in sorted((win / "ref").glob("*.cpu.png")):
        name = ref.name[: -len(".cpu.png")]
        w = load(ref)
        g = load(win / "ref" / f"{name}.gpu.png")
        m_path = mac / "ref" / ref.name
        row = {"scene": name, "pixels": int(w.shape[0] * w.shape[1])}
        row["win_cpu_vs_gpu"] = int((np.abs(w - g).max(axis=2) > 0).sum())
        if m_path.exists():
            m = load(m_path)
            if m.shape == w.shape:
                d = np.abs(w - m).max(axis=2)
                row["win_vs_mac_cpu"] = int((d > 0).sum())
                row["win_vs_mac_max"] = int(d.max())
        scene = json.loads((win / "scenes" / f"{name}.json").read_text())
        row["gate"] = scene.get("gate")
        if "text" in name or row["gate"] == "calibration":
            premult = w.copy()
            n, mx = chroma_stats(premult)
            row["coloured_px"] = n
            row["max_spread"] = mx
        rows.append(row)
    for r in rows:
        print(json.dumps(r))
    print("--- dom text scenes")
    for png in sorted((win / "dom").glob("*.png")):
        img = load(png)
        n, mx = chroma_stats(img)
        print(json.dumps({"dom": png.name, "coloured_px": n, "max_spread": mx, "size": list(img.shape[:2])}))


main()
