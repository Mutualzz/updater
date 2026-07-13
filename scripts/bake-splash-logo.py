#!/usr/bin/env python3

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image
from psd_tools import PSDImage

LOGO_PX = 108
R = LOGO_PX / 2
ICON_PX = max(32, int(round(R * 0.26 * 3)))
ANARCHY_PX = max(32, int(round(R * 0.251 * 3)))
PENTA_PX = LOGO_PX * 3

EXPORTS = {
    "Anarchy": ("anarchy.png", ANARCHY_PX),
    "Emo Hair": ("emo_hair.png", ICON_PX),
    "Scene Hair": ("scene_hair.png", ICON_PX),
    "Guitar": ("guitar.png", ICON_PX),
    "Microphone": ("microphone.png", ICON_PX),
    "Cathedral": ("cathedral.png", ICON_PX),
}


def find_layer(psd: PSDImage, name: str):
    for layer in psd.descendants():
        if getattr(layer, "name", None) == name:
            return layer
    raise KeyError(f"Layer not found: {name}")


def export_layer(layer, size: int) -> Image.Image:
    img = layer.composite()
    if img.mode != "RGBA":
        img = img.convert("RGBA")
    return img.resize((size, size), Image.Resampling.LANCZOS)


def build_pentagram(psd: PSDImage, size: int) -> Image.Image:
    candidates = [
        "Pentagram",
        "pentagram",
        "Star",
        "Ring",
        "Outer Ring",
    ]
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    found = False
    for name in candidates:
        try:
            layer = find_layer(psd, name)
        except KeyError:
            continue
        part = layer.composite()
        if part.mode != "RGBA":
            part = part.convert("RGBA")
        part = part.resize((size, size), Image.Resampling.LANCZOS)
        canvas = Image.alpha_composite(canvas, part)
        found = True
    if not found:
        raise RuntimeError("No pentagram/ring layers found in PSD")
    return canvas


def main() -> None:
    parser = argparse.ArgumentParser(description="Bake splash logo assets from Mutualzz PSD")
    parser.add_argument(
        "psd",
        nargs="?",
        default=r"h:\Azrael\Mutualzz Assets\logo.psd\logo.psd",
        help="Path to logo.psd",
    )
    parser.add_argument(
        "-o",
        "--out",
        default=None,
        help="Output directory (default: packages/updater/resources/logo)",
    )
    args = parser.parse_args()

    psd_path = Path(args.psd)
    if not psd_path.is_file():
        raise SystemExit(f"PSD not found: {psd_path}")

    out_dir = (
        Path(args.out)
        if args.out
        else Path(__file__).resolve().parents[1] / "resources" / "logo"
    )
    out_dir.mkdir(parents=True, exist_ok=True)

    psd = PSDImage.open(str(psd_path))

    for layer_name, (filename, size) in EXPORTS.items():
        layer = find_layer(psd, layer_name)
        export_layer(layer, size).save(out_dir / filename)
        print(f"wrote {filename} ({size}px)")

    penta = build_pentagram(psd, PENTA_PX)
    penta.save(out_dir / "pentagram_overlay.png")
    print(f"wrote pentagram_overlay.png ({PENTA_PX}px)")
    print(f"done → {out_dir}")


if __name__ == "__main__":
    main()
