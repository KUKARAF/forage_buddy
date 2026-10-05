#!/usr/bin/env python3
"""Regenerates the three master PNGs in this directory (app-icon.png,
app-icon-bg.png, app-icon-fg.png) from a hand-coded Amanita muscaria glyph.

Usage:
    python3 -m venv /tmp/icon_venv && source /tmp/icon_venv/bin/activate
    pip install Pillow
    python3 generate.py

Then regenerate the full platform icon set from these masters with:
    cargo tauri icon icon-source/manifest.json -o icons
(run from crates/mobile/; requires the tauri-cli: `cargo binstall tauri-cli`
or `cargo install tauri-cli`.)
"""

import os
from PIL import Image, ImageDraw

OUT_DIR = os.path.dirname(os.path.abspath(__file__))
S = 2048
FOREST = (47, 82, 51, 255)
CAP_RED = (202, 48, 34, 255)
CAP_OUTLINE = (58, 20, 16, 255)
SPOT = (250, 247, 236, 255)
STEM = (250, 247, 236, 255)

def draw_mushroom(d, cx, cy, cap_r, ow):
    """A single bold, simplified Amanita glyph: a flared dome cap fused
    directly onto a simple stem, no floating pieces, few large spots.
    cap_r = cap half-width. Everything positioned relative to cx/cy so the
    whole glyph's bounding box is predictable and centerable."""

    # Stem drawn first, full height incl. bulb base, so the cap's rim simply
    # overlaps/covers its top edge -> guaranteed no seam/gap.
    stem_top_w = cap_r * 0.62
    stem_top_y = cy + cap_r * 0.28
    stem_bot_y = cy + cap_r * 1.25
    base_w = stem_top_w * 1.5
    d.polygon(
        [
            (cx - stem_top_w / 2, stem_top_y),
            (cx + stem_top_w / 2, stem_top_y),
            (cx + base_w / 2, stem_bot_y),
            (cx - base_w / 2, stem_bot_y),
        ],
        fill=STEM,
    )
    d.ellipse([cx - base_w / 2, stem_bot_y - base_w * 0.25, cx + base_w / 2, stem_bot_y + base_w * 0.25], fill=STEM)
    d.ellipse([cx - stem_top_w / 2, stem_top_y - stem_top_w / 2, cx + stem_top_w / 2, stem_top_y + stem_top_w / 2], fill=STEM)

    # Cap: one flared dome, drawn as a single filled polygon-ish shape via a
    # wide ellipse (the "skirt") with a taller dome ellipse unioned on top,
    # same fill/outline on both so they read as one continuous silhouette.
    rim_cy = cy + cap_r * 0.18
    rim_w = cap_r * 2.0  # exactly the dome's width so the two outline arcs share endpoints (no seam)
    rim_h = cap_r * 0.60
    d.ellipse([cx - rim_w / 2, rim_cy - rim_h / 2, cx + rim_w / 2, rim_cy + rim_h / 2], fill=CAP_RED)
    dome_h = cap_r * 1.5
    d.pieslice([cx - cap_r, rim_cy - dome_h, cx + cap_r, rim_cy + dome_h], 180, 360, fill=CAP_RED)
    # one outline pass around the fused silhouette's visible outer edge
    d.arc([cx - cap_r, rim_cy - dome_h, cx + cap_r, rim_cy + dome_h], 180, 360, fill=CAP_OUTLINE, width=ow)
    d.arc([cx - rim_w / 2, rim_cy - rim_h / 2, cx + rim_w / 2, rim_cy + rim_h / 2], 0, 180, fill=CAP_OUTLINE, width=ow)

    # A few large, bold spots -- legible at tiny sizes beats botanical count.
    for dx, dy, r in [(-0.42, -0.55, 0.20), (0.40, -0.52, 0.19), (0.0, -0.95, 0.15), (-0.78, -0.10, 0.13), (0.80, -0.08, 0.13)]:
        px, py = cx + dx * cap_r, rim_cy + dy * cap_r
        pr = r * cap_r
        d.ellipse([px - pr, py - pr, px + pr, py + pr], fill=SPOT)


def render(bg_mode, out_path, cap_r_frac, cy_frac):
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    if bg_mode == "square":
        d.rectangle([0, 0, S, S], fill=FOREST)
    elif bg_mode == "none":
        pass
    draw_mushroom(d, S * 0.5, S * cy_frac, S * cap_r_frac, ow=max(4, S // 340))
    img.save(out_path)
    return img

# default (opaque, square bg) -- full icon for legacy/other platforms
render("square", os.path.join(OUT_DIR, "app-icon.png"), 0.27, 0.46)
# android adaptive bg (solid)
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
ImageDraw.Draw(img).rectangle([0, 0, S, S], fill=FOREST)
img.save(os.path.join(OUT_DIR, "app-icon-bg.png"))
# android adaptive fg (transparent, content within the safe zone -> smaller cap_r, centered)
render("none", os.path.join(OUT_DIR, "app-icon-fg.png"), 0.19, 0.52)

print("rendered ->", OUT_DIR)
