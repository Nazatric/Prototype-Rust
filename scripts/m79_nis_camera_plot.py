#!/usr/bin/env python3
"""m79 (rebuild of m77): plot the REAL NIS camera takes in 3D.

Decodes the CAM_NisCam_Take TRAN keyframe streams from the committed
a0+a1 fixtures (the intro cutscene NIS_0_3_a's real track members) and
plots the camera PATHS — the game's OWN camera data, not a synthetic
path.  Output: media/12_nis_0_3_a_real_camera_paths.png (1320x825).
"""

import struct
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

FIX = "crates/prototype-bin/tests/fixtures"
SECTION_MARKERS = [b"LOOK", b"TRAN", b"FOV", b"ROT", b"LVY", b"BWL", b"SJL"]
TRACK = 0x00121000


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def f32(b, o):
    return struct.unpack_from("<f", b, o)[0]


def find_marker(buf, marker, start):
    return buf.find(marker, start)


def section_vec3s(chunk, marker):
    start = find_marker(chunk, marker, 40)
    if start < 0:
        return []
    ends = [find_marker(chunk, m, 40) for m in SECTION_MARKERS]
    ends = [e for e in ends if e > start]
    end = min(ends) if ends else len(chunk)
    if end <= start + 8:
        return []
    count = u32(chunk, start + len(marker))
    if count == 0 or count > 1_000_000:
        return []
    base = start + len(marker) + 4 + count * 2
    if base >= end:
        return []
    keys = []
    for k in range((end - base) // 12):
        off = base + k * 12
        key = [f32(chunk, off), f32(chunk, off + 4), f32(chunk, off + 8)]
        finite = all(abs(v) != float("inf") and v == v for v in key)
        if k > 0 and (key == [0.0, 0.0, 0.0] or not finite):
            break
        keys.append(key)
    return keys


def camera_takes(p3d):
    takes = []
    pos = 12
    while pos + 12 <= len(p3d):
        ctype = u32(p3d, pos)
        size = u32(p3d, pos + 8)
        if size < 12 or pos + size > len(p3d):
            break
        if ctype == TRACK:
            chunk = p3d[pos : pos + size]
            payload = chunk[12:]
            if len(payload) > 5 and payload[0:4] == b"\0\0\0\0":
                slot = payload[4]
                if 0 < slot <= len(payload) - 5:
                    name = payload[5 : 5 + slot].split(b"\0")[0].decode("utf-8", "replace")
                    if name.startswith("CAM_"):
                        takes.append(
                            (name, section_vec3s(chunk, b"TRAN"), section_vec3s(chunk, b"LOOK"))
                        )
        pos += size
    return takes


def main():
    fig = plt.figure(figsize=(13.2, 8.25), dpi=100, constrained_layout=True)
    ax = fig.add_subplot(111, projection="3d")
    colors = plt.cm.viridis(np.linspace(0.05, 0.9, 6))
    total = 0
    for fi, fname in enumerate(
        ["art_nis_nis_0_3_a0_camera_takes.p3d", "art_nis_nis_0_3_a1_camera_takes.p3d"]
    ):
        p3d = open(f"{FIX}/{fname}", "rb").read()
        for ti, (name, tran, look) in enumerate(camera_takes(p3d)):
            if len(tran) < 2:
                continue
            xs, ys, zs = zip(*tran)
            c = colors[(fi * 3 + ti) % len(colors)]
            ax.plot(xs, zs, ys, "-", color=c, lw=1.6, label=f"{name} ({len(tran)} keys)")
            ax.scatter([xs[0]], [zs[0]], [ys[0]], color=c, marker="o", s=28)
            ax.scatter([xs[-1]], [zs[-1]], [ys[-1]], color=c, marker="^", s=28)
            total += 1
    ax.set_xlabel("TRAN x")
    ax.set_ylabel("TRAN z")
    ax.set_zlabel("TRAN y")
    ax.set_title(
        "NIS_0_3_a (intro cutscene) REAL in-engine camera takes\n"
        "CAM_NisCam_Take TRAN keyframe streams from the game's own track data (o=start ^=end)"
    )
    ax.legend(loc="upper left", fontsize=8)
    out = sys.argv[1] if len(sys.argv) > 1 else "media/12_nis_0_3_a_real_camera_paths.png"
    fig.savefig(out)
    print(f"plotted {total} real takes -> {out}")


if __name__ == "__main__":
    main()
