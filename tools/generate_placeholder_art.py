#!/usr/bin/env python3
"""Generate placeholder pet art.

These are stand-ins so the bot looks right on first run. Replace any file with
real artwork of the same name and the bot picks it up on the next restart -
nothing here is referenced by the Rust code, which only globs the directory.

Usage:
    python tools/generate_placeholder_art.py [--out assets/pets] [--force]

Requires Pillow:
    pip install pillow
"""

from __future__ import annotations

import argparse
import colorsys
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SIZE = 480
SUPERSAMPLE = 2  # draw large, downscale for cheap antialiasing

# Every mood the bot can ask for. Keep in sync with `Mood::key` in src/model.rs.
MOODS = ["happy", "content", "hungry", "sad", "tired", "sick", "sleeping", "dead"]

# Relative size of the character at each life stage.
STAGE_SCALE = {"baby": 0.62, "child": 0.78, "teen": 0.9, "adult": 1.0, "elder": 0.96}

# key -> (body colour, shading colour, accent colour)
SPECIES = {
    "blob": ((0x6C, 0xC6, 0xE8), (0x47, 0xA2, 0xC6), (0xFF, 0xFF, 0xFF)),
    "dragon": ((0x7A, 0xC7, 0x74), (0x4F, 0x9B, 0x50), (0xF2, 0xB8, 0x4B)),
    "cat": ((0xF0, 0xA9, 0x62), (0xCB, 0x84, 0x43), (0xFF, 0xE0, 0xC2)),
    "cactus": ((0x5F, 0xA8, 0x63), (0x3F, 0x7F, 0x45), (0xE8, 0x6B, 0x8E)),
    "ghost": ((0xD8, 0xD4, 0xF0), (0xAE, 0xA8, 0xD6), (0x8B, 0x84, 0xC4)),
    "duck": ((0xF5, 0xDB, 0x6B), (0xD4, 0xB5, 0x3E), (0xEE, 0x8C, 0x3C)),
    # Fallback art, used for any species with no folder of its own.
    "_default": ((0xA9, 0xB2, 0xC3), (0x84, 0x8E, 0xA0), (0xFF, 0xFF, 0xFF)),
}


def shift(rgb, *, saturation=1.0, value=1.0):
    """Nudge a colour in HSV space."""
    r, g, b = (c / 255 for c in rgb)
    h, s, v = colorsys.rgb_to_hsv(r, g, b)
    s = max(0.0, min(1.0, s * saturation))
    v = max(0.0, min(1.0, v * value))
    return tuple(round(c * 255) for c in colorsys.hsv_to_rgb(h, s, v))


def mood_palette(base, shade, accent, mood):
    """Tint the palette to suit the mood."""
    if mood == "dead":
        return shift(base, saturation=0.05, value=0.55), shift(shade, saturation=0.05, value=0.45), accent
    if mood == "sick":
        return shift(base, saturation=0.75, value=0.85), shift(shade, saturation=0.75, value=0.75), accent
    if mood in ("sad", "tired"):
        return shift(base, saturation=0.8, value=0.92), shift(shade, saturation=0.8, value=0.85), accent
    if mood == "happy":
        return shift(base, saturation=1.15, value=1.05), shift(shade, saturation=1.1), accent
    return base, shade, accent


# -- body shapes -------------------------------------------------------------


def body_blob(d, cx, cy, r, base, shade, accent):
    d.ellipse([cx - r, cy - r * 0.78, cx + r, cy + r * 0.95], fill=base)
    d.ellipse([cx - r * 0.72, cy + r * 0.4, cx + r * 0.72, cy + r * 0.95], fill=shade)


def body_dragon(d, cx, cy, r, base, shade, accent):
    # Wings behind the body.
    d.polygon([(cx - r, cy - r * 0.2), (cx - r * 1.5, cy - r * 0.8), (cx - r * 0.5, cy - r * 0.6)], fill=shade)
    d.polygon([(cx + r, cy - r * 0.2), (cx + r * 1.5, cy - r * 0.8), (cx + r * 0.5, cy - r * 0.6)], fill=shade)
    d.ellipse([cx - r, cy - r * 0.85, cx + r, cy + r * 0.95], fill=base)
    # Dorsal spikes.
    for i, off in enumerate((-0.45, 0.0, 0.45)):
        h = r * (0.30 if i != 1 else 0.42)
        x = cx + r * off
        d.polygon([(x - r * 0.16, cy - r * 0.78), (x, cy - r * 0.78 - h), (x + r * 0.16, cy - r * 0.78)], fill=accent)


def body_cat(d, cx, cy, r, base, shade, accent):
    for sign in (-1, 1):
        x = cx + sign * r * 0.55
        d.polygon([(x - r * 0.28, cy - r * 0.55), (x, cy - r * 1.15), (x + r * 0.28, cy - r * 0.55)], fill=base)
        d.polygon([(x - r * 0.14, cy - r * 0.6), (x, cy - r * 0.95), (x + r * 0.14, cy - r * 0.6)], fill=accent)
    d.ellipse([cx - r, cy - r * 0.8, cx + r, cy + r * 0.95], fill=base)
    d.ellipse([cx - r * 0.55, cy + r * 0.25, cx + r * 0.55, cy + r * 0.95], fill=shade)


def body_cactus(d, cx, cy, r, base, shade, accent):
    d.rounded_rectangle([cx - r * 0.45, cy - r * 0.95, cx + r * 0.45, cy + r * 0.95], radius=r * 0.42, fill=base)
    d.rounded_rectangle([cx - r * 0.95, cy - r * 0.25, cx - r * 0.3, cy + r * 0.15], radius=r * 0.2, fill=base)
    d.rounded_rectangle([cx + r * 0.3, cy - r * 0.45, cx + r * 0.95, cy - r * 0.05], radius=r * 0.2, fill=base)
    # Ribs.
    for off in (-0.18, 0.18):
        d.line([(cx + r * off, cy - r * 0.7), (cx + r * off, cy + r * 0.7)], fill=shade, width=max(2, int(r * 0.05)))
    # Flower.
    d.ellipse([cx - r * 0.16, cy - r * 1.18, cx + r * 0.16, cy - r * 0.86], fill=accent)


def body_ghost(d, cx, cy, r, base, shade, accent):
    d.pieslice([cx - r, cy - r, cx + r, cy + r * 0.6], start=180, end=360, fill=base)
    d.rectangle([cx - r, cy - r * 0.2, cx + r, cy + r * 0.55], fill=base)
    # Wavy hem.
    lobes = 4
    width = (2 * r) / lobes
    for i in range(lobes):
        left = cx - r + i * width
        d.pieslice([left, cy + r * 0.25, left + width, cy + r * 0.85], start=0, end=180, fill=base)
    d.ellipse([cx - r * 0.5, cy + r * 0.1, cx + r * 0.5, cy + r * 0.45], fill=shade)


def body_duck(d, cx, cy, r, base, shade, accent):
    d.ellipse([cx - r, cy - r * 0.35, cx + r, cy + r * 0.95], fill=base)
    d.ellipse([cx - r * 0.62, cy - r * 1.05, cx + r * 0.62, cy + r * 0.2], fill=base)
    d.ellipse([cx - r * 0.9, cy + r * 0.3, cx + r * 0.2, cy + r * 0.9], fill=shade)
    # Beak.
    d.ellipse([cx - r * 0.2, cy - r * 0.42, cx + r * 0.5, cy - r * 0.12], fill=accent)


BODIES = {
    "blob": body_blob,
    "dragon": body_dragon,
    "cat": body_cat,
    "cactus": body_cactus,
    "ghost": body_ghost,
    "duck": body_duck,
    "_default": body_blob,
}

# Where the face sits, as a fraction of the radius above centre.
FACE_OFFSET = {
    "blob": -0.18,
    "dragon": -0.25,
    "cat": -0.20,
    "cactus": -0.32,
    "ghost": -0.30,
    "duck": -0.50,
    "_default": -0.18,
}


# -- faces -------------------------------------------------------------------

INK = (0x2B, 0x2D, 0x3A)


def draw_eyes(d, cx, cy, r, mood):
    dx = r * 0.30
    eye_r = r * 0.13

    if mood in ("sleeping", "tired"):
        for sign in (-1, 1):
            x = cx + sign * dx
            d.arc([x - eye_r, cy - eye_r, x + eye_r, cy + eye_r], start=200, end=340, fill=INK,
                  width=max(3, int(r * 0.05)))
        return

    if mood == "dead":
        w = max(3, int(r * 0.055))
        for sign in (-1, 1):
            x = cx + sign * dx
            d.line([(x - eye_r, cy - eye_r), (x + eye_r, cy + eye_r)], fill=INK, width=w)
            d.line([(x - eye_r, cy + eye_r), (x + eye_r, cy - eye_r)], fill=INK, width=w)
        return

    for sign in (-1, 1):
        x = cx + sign * dx
        d.ellipse([x - eye_r, cy - eye_r, x + eye_r, cy + eye_r], fill=(0xFF, 0xFF, 0xFF))
        # Sad and hungry eyes look down; the rest look straight ahead.
        pupil_dy = eye_r * 0.35 if mood in ("sad", "hungry", "sick") else 0
        pr = eye_r * 0.55
        d.ellipse([x - pr, cy - pr + pupil_dy, x + pr, cy + pr + pupil_dy], fill=INK)
        if mood == "happy":
            gr = eye_r * 0.2
            d.ellipse([x - pr * 0.2, cy - pr * 0.8, x - pr * 0.2 + gr * 2, cy - pr * 0.8 + gr * 2],
                      fill=(0xFF, 0xFF, 0xFF))


def draw_mouth(d, cx, cy, r, mood):
    w = max(3, int(r * 0.055))
    mw = r * 0.28
    my = cy + r * 0.34

    if mood == "happy":
        d.arc([cx - mw, my - mw * 0.8, cx + mw, my + mw * 0.9], start=0, end=180, fill=INK, width=w)
    elif mood == "content":
        d.arc([cx - mw * 0.8, my - mw * 0.5, cx + mw * 0.8, my + mw * 0.6], start=20, end=160, fill=INK, width=w)
    elif mood in ("sad", "hungry"):
        d.arc([cx - mw, my + mw * 0.3, cx + mw, my + mw * 1.5], start=180, end=360, fill=INK, width=w)
    elif mood == "sick":
        # A wavy, queasy line.
        pts = []
        for i in range(21):
            t = i / 20
            pts.append((cx - mw + 2 * mw * t, my + math.sin(t * math.pi * 3) * r * 0.06))
        d.line(pts, fill=INK, width=w, joint="curve")
    elif mood == "tired":
        d.line([(cx - mw * 0.7, my), (cx + mw * 0.7, my)], fill=INK, width=w)
    elif mood == "sleeping":
        d.ellipse([cx - r * 0.09, my - r * 0.09, cx + r * 0.09, my + r * 0.09], fill=INK)
    elif mood == "dead":
        d.line([(cx - mw * 0.6, my), (cx + mw * 0.6, my)], fill=INK, width=w)
    else:
        d.arc([cx - mw * 0.8, my - mw * 0.5, cx + mw * 0.8, my + mw * 0.6], start=20, end=160, fill=INK, width=w)


def draw_extras(img, d, cx, cy, r, mood):
    if mood == "sleeping":
        font = ImageFont.load_default(size=int(r * 0.42))
        d.text((cx + r * 0.75, cy - r * 1.05), "z", font=font, fill=INK)
        font = ImageFont.load_default(size=int(r * 0.30))
        d.text((cx + r * 1.05, cy - r * 1.30), "z", font=font, fill=INK)
    elif mood == "hungry":
        # A rumbling-stomach spiral.
        d.arc([cx - r * 0.28, cy + r * 0.55, cx + r * 0.28, cy + r * 1.05], start=0, end=300,
              fill=INK, width=max(2, int(r * 0.04)))
    elif mood == "sick":
        # Sweat drop.
        x, y = cx + r * 0.55, cy - r * 0.28
        d.ellipse([x - r * 0.09, y, x + r * 0.09, y + r * 0.22], fill=(0x6C, 0xC6, 0xE8))
        d.polygon([(x - r * 0.09, y + r * 0.05), (x, y - r * 0.14), (x + r * 0.09, y + r * 0.05)],
                  fill=(0x6C, 0xC6, 0xE8))
    elif mood == "sad":
        # Tear.
        x, y = cx - r * 0.30, cy + r * 0.12
        d.ellipse([x - r * 0.055, y, x + r * 0.055, y + r * 0.16], fill=(0x6C, 0xC6, 0xE8))


def render(species_key, mood, stage="adult"):
    base, shade, accent = mood_palette(*SPECIES[species_key], mood)
    s = SIZE * SUPERSAMPLE
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    r = s * 0.28 * STAGE_SCALE[stage]
    cx, cy = s / 2, s / 2 + r * 0.1

    # Ground shadow, so the character does not float.
    d.ellipse([cx - r * 0.85, cy + r * 0.82, cx + r * 0.85, cy + r * 1.06], fill=(0, 0, 0, 38))

    BODIES[species_key](d, cx, cy, r, base, shade, accent)

    face_y = cy + r * FACE_OFFSET[species_key]
    draw_eyes(d, cx, face_y, r, mood)
    draw_mouth(d, cx, face_y, r, mood)
    draw_extras(img, d, cx, face_y, r, mood)

    if mood == "dead":
        # Fade the whole character out a little.
        alpha = img.getchannel("A").point(lambda v: int(v * 0.72))
        img.putalpha(alpha)

    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", default="assets/pets", type=Path, help="output directory")
    parser.add_argument("--force", action="store_true", help="overwrite existing files")
    args = parser.parse_args()

    written = skipped = 0

    for species_key in SPECIES:
        for mood in MOODS:
            path = args.out / species_key / f"{mood}.png"
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists() and not args.force:
                skipped += 1
                continue
            render(species_key, mood).save(path)
            written += 1

    # A couple of per-stage overrides, to exercise the stage lookup and show
    # how it is wired up.
    for stage in ("baby", "elder"):
        for mood in MOODS:
            path = args.out / "blob" / stage / f"{mood}.png"
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists() and not args.force:
                skipped += 1
                continue
            render("blob", mood, stage).save(path)
            written += 1

    print(f"wrote {written} file(s), skipped {skipped} existing")


if __name__ == "__main__":
    main()
