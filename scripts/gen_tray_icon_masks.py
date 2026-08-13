"""Regenerate src/tray/icon_masks.rs from DeeCtx-web/public/favicon.svg.

The favicon is a rounded-square background with a stroked glyph (a script
"D"/flag shape) on top. We rasterize both shapes at high resolution and
downsample for anti-aliasing, then bake the alpha masks into Rust byte
arrays. The tray icon composites these masks with the current state color
at runtime (see src/tray/icon.rs), so no image-decoding dependency or
external asset file is needed at runtime -- only at generation time here.

Run manually with `python scripts/gen_tray_icon_masks.py` whenever
DeeCtx-web/public/favicon.svg changes. Requires Pillow and numpy
(`pip install Pillow numpy`).
"""

import numpy as np
from PIL import Image, ImageDraw

SCALE = 16       # supersample factor
VB = 48          # svg viewBox size
HR = VB * SCALE  # high-res canvas size
OUT = 32         # final icon size, must match icon::ICON_SIZE


def cubic_bezier(p0, p1, p2, p3, steps=200):
    pts = []
    for i in range(steps + 1):
        t = i / steps
        mt = 1 - t
        x = (mt**3) * p0[0] + 3 * (mt**2) * t * p1[0] + 3 * mt * (t**2) * p2[0] + (t**3) * p3[0]
        y = (mt**3) * p0[1] + 3 * (mt**2) * t * p1[1] + 3 * mt * (t**2) * p2[1] + (t**3) * p3[1]
        pts.append((x, y))
    return pts


def sc(pt):
    return (pt[0] * SCALE, pt[1] * SCALE)


def main():
    # rounded-square background, matches <rect rx="10">
    bg = Image.new("L", (HR, HR), 0)
    ImageDraw.Draw(bg).rounded_rectangle([0, 0, HR - 1, HR - 1], radius=10 * SCALE, fill=255)

    # glyph strokes, matches the two <path> elements + <line> in the favicon
    glyph = Image.new("L", (HR, HR), 0)
    gd = ImageDraw.Draw(glyph)
    stroke_w = int(round(4.4 * SCALE))
    r = stroke_w / 2

    def stroked_line(a, b):
        pa, pb = sc(a), sc(b)
        gd.line([pa, pb], fill=255, width=stroke_w, joint="curve")
        for p in (pa, pb):
            gd.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=255)

    def stroked_polyline(pts):
        scaled = [sc(p) for p in pts]
        gd.line(scaled, fill=255, width=stroke_w, joint="curve")
        for p in (scaled[0], scaled[-1]):
            gd.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=255)

    stroked_line((15, 11), (15, 37))  # M15 11v26
    curve = (
        cubic_bezier((15, 11), (29, 11), (37, 17.5), (37, 24))
        + cubic_bezier((37, 24), (37, 30.5), (29, 37), (15, 37))
    )  # M15 11c14 0 22 6.5 22 13s-8 13-22 13
    stroked_polyline(curve)
    stroked_line((34, 35.5), (41.5, 35.5))  # flag serif

    bg_small = np.array(bg.resize((OUT, OUT), Image.LANCZOS), dtype=np.uint8)
    glyph_small = np.array(glyph.resize((OUT, OUT), Image.LANCZOS), dtype=np.uint8)
    glyph_small = np.minimum(glyph_small, bg_small)  # glyph never exceeds bg coverage

    def emit(name, arr):
        flat = arr.flatten().tolist()
        lines = [
            "    " + ", ".join(str(v) for v in flat[i : i + 16]) + ","
            for i in range(0, len(flat), 16)
        ]
        return f"pub const {name}: [u8; {len(flat)}] = [\n" + "\n".join(lines) + "\n];"

    src = (
        "//! Alpha masks for the tray icon, generated from\n"
        "//! `DeeCtx-web/public/favicon.svg` by `scripts/gen_tray_icon_masks.py`.\n"
        "//! Regenerate with that script if the favicon artwork changes; do not\n"
        "//! hand-edit these arrays.\n\n"
        + emit("BG_MASK", bg_small)
        + "\n\n"
        + emit("GLYPH_MASK", glyph_small)
        + "\n"
    )

    out_path = "src/tray/icon_masks.rs"
    with open(out_path, "w") as f:
        f.write(src)
    print(f"wrote {out_path}: bg max={bg_small.max()} glyph max={glyph_small.max()}")


if __name__ == "__main__":
    main()
