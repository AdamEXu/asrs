"""Approximate PLAN gate 4 for a drawing-buffer capture against a port readback.

Geometry edges are not available from the port here, so an edge is any pixel whose colour
jumps by more than 8 levels to a 4-neighbour in either image; interior pixels are those more
than 2 px from such a pixel. SSIM is luma, Gaussian 11, sigma 1.5, as in the S3 spike.
usage: python gate4.py <upstream.png> <port.png>
"""
import sys

import numpy as np
from PIL import Image
from scipy.ndimage import binary_dilation, gaussian_filter


def load(p):
    return np.asarray(Image.open(p).convert("RGB")).astype(np.int32)


def edges(img):
    e = np.zeros(img.shape[:2], bool)
    for axis in (0, 1):
        d = np.abs(np.diff(img, axis=axis)).max(-1) > 8
        if axis == 0:
            e[1:] |= d
            e[:-1] |= d
        else:
            e[:, 1:] |= d
            e[:, :-1] |= d
    return e


def ssim(a, b):
    la = (a * [0.299, 0.587, 0.114]).sum(-1)
    lb = (b * [0.299, 0.587, 0.114]).sum(-1)
    g = lambda x: gaussian_filter(x, 1.5, truncate=3.5)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    ma, mb = g(la), g(lb)
    va, vb, cov = g(la * la) - ma * ma, g(lb * lb) - mb * mb, g(la * lb) - ma * mb
    s = ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2))
    return float(s.mean())


up, port = load(sys.argv[1]), load(sys.argv[2])
diff = np.abs(up - port).max(-1)
edge = binary_dilation(edges(up) | edges(port), iterations=2)
interior = ~edge
n = diff.size
s = ssim(up.astype(float), port.astype(float))
interior_diff = diff[interior]
res = {
    "ssim": round(s, 5),
    "differ_pct": round(100 * (diff > 0).sum() / n, 3),
    "interior_pct_of_image": round(100 * interior.sum() / n, 1),
    "interior_differ_pct": round(100 * (interior_diff > 0).sum() / max(1, interior_diff.size), 3),
    "interior_max": int(interior_diff.max()) if interior_diff.size else 0,
    "interior_over2": int((interior_diff > 2).sum()),
    "edge_max": int(diff[edge].max()) if edge.any() else 0,
    "mean_abs": round(float(np.abs(up - port).mean()), 3),
}
gate = res["ssim"] >= 0.995 and res["interior_max"] <= 2 and res["interior_differ_pct"] <= 0.05 and res["differ_pct"] <= 1.0
print(sys.argv[2].split("/")[-1], res, "PASS" if gate else "FAIL")
