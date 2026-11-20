"""Gate 4 approximation for port vs upstream 3D frames.

Geometry edges come from the images themselves (a luminance step over 4 levels in a 3x3
neighbourhood of either image), dilated by 2 px; everything else counts as interior.

usage: uv run --with numpy --with pillow --with scikit-image compare.py <upstream dir> <port dir> <tag>...
"""

import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from skimage.metrics import structural_similarity


def load(p):
    return np.asarray(Image.open(p).convert("RGB")).astype(np.int16)


def neighbourhood(a, r, fn):
    pad = np.pad(a, r, mode="edge")
    h, w = a.shape
    out = None
    for dy in range(2 * r + 1):
        for dx in range(2 * r + 1):
            s = pad[dy : dy + h, dx : dx + w]
            out = s if out is None else fn(out, s)
    return out


def edges(img):
    lum = img @ np.array([0.299, 0.587, 0.114])
    return (neighbourhood(lum, 1, np.maximum) - neighbourhood(lum, 1, np.minimum)) > 4


def main():
    up_dir, port_dir, tags = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3:]
    for tag in tags:
        for up in sorted(up_dir.glob(f"upstream_*_{tag}.png")):
            name = up.name[len("upstream_") : -len(f"_{tag}.png")]
            port_path = next(port_dir.glob(f"port_{name}_{tag}_msaa*.png"), None)
            if port_path is None:
                continue
            a, b = load(up), load(port_path)
            if a.shape != b.shape:
                print(json.dumps({"scene": name, "tag": tag, "error": f"shape {a.shape} vs {b.shape}"}))
                continue
            diff = np.abs(a - b).max(axis=2)
            edge = edges(a) | edges(b)
            near = neighbourhood(edge.astype(np.uint8), 2, np.maximum).astype(bool)
            interior = ~near
            ssim = float(structural_similarity(a.astype(np.uint8), b.astype(np.uint8), channel_axis=2))
            row = {
                "scene": name,
                "tag": tag,
                "ssim": round(ssim, 5),
                "differ_pct": round(100 * float((diff > 0).mean()), 3),
                "exact_pct": round(100 * float((diff == 0).mean()), 3),
                "mean_abs": round(float(np.abs(a - b).mean()), 3),
                "max": int(diff.max()),
                "interior_max": int(diff[interior].max()) if interior.any() else 0,
                "interior_differ_pct": round(100 * float((diff[interior] > 0).mean()), 4),
                "interior_over2": int((diff[interior] > 2).sum()),
                "interior_pct_of_image": round(100 * float(interior.mean()), 1),
            }
            row["gate4"] = (
                ssim >= 0.995
                and row["interior_max"] <= 2
                and row["interior_differ_pct"] <= 0.05
                and row["differ_pct"] <= 1.0
            )
            print(json.dumps(row))


main()
