#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["cairosvg>=2.7", "pillow>=10"]
# ///
"""Render Warpium's raster branding assets from the SVG sources in `branding/`.

Run from anywhere with `uv run script/warpium/render_icons.py` (cairosvg also needs the
system cairo library). Outputs are committed, so this only needs to run after the SVG
sources change (they are written by `script/warpium/generate_svgs.py`).

The README product preview places the window from upstream Warp's README screenshot on
`branding/preview-background.svg`. It is only re-rendered when that screenshot is passed:

    curl -L -o /tmp/warp-preview.png \
        https://github.com/user-attachments/assets/9976b2da-2edd-4604-a36c-8fd53719c6d4
    uv run script/warpium/render_icons.py --preview-screenshot /tmp/warp-preview.png
"""

import argparse
import io
import struct
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw, ImageFilter

REPO_ROOT = Path(__file__).resolve().parents[2]
BRANDING = REPO_ROOT / "branding"
OSS_ICON_DIR = REPO_ROOT / "app" / "channels" / "oss" / "icon"
NO_PADDING_DIR = OSS_ICON_DIR / "no-padding"
DOCS_IMAGES = REPO_ROOT / "docs" / "images"
MAC_RESOURCES = REPO_ROOT / "app" / "assets" / "resources" / "mac"

# At or below this size, render icon-small.svg (bigger mark, no grid or border) instead.
SMALL_ICON_MAX_PX = 40

NO_PADDING_SIZES = [16, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

# ICNS entry types that hold PNG data, keyed by pixel size. Retina variants reuse the
# same pixels as the next size up, e.g. `ic11` is the 16pt@2x (32 px) image.
ICNS_TYPES = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
]

# Apple's macOS icon grid: the icon body is 824 px inside a 1024 px canvas, with a soft
# drop shadow underneath.
MAC_BODY_RATIO = 824 / 1024


def render_svg(svg: Path, width: int, height: int | None = None) -> Image.Image:
    png = cairosvg.svg2png(url=str(svg), output_width=width, output_height=height or width)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def render_icon(size: int) -> Image.Image:
    source = "icon-small.svg" if size <= SMALL_ICON_MAX_PX else "icon.svg"
    return render_svg(BRANDING / source, size)


def render_mac_icon(size: int) -> Image.Image:
    body_size = round(size * MAC_BODY_RATIO)
    body = render_icon(body_size)
    offset = (size - body_size) // 2
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    shadow_alpha = body.getchannel("A").point(lambda a: a * 0.3)
    shadow = Image.new("RGBA", body.size, (0, 0, 0, 0))
    shadow.putalpha(shadow_alpha)
    shadow_layer = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    shadow_layer.paste(shadow, (offset, offset + max(1, round(size * 10 / 1024))))
    shadow_layer = shadow_layer.filter(ImageFilter.GaussianBlur(radius=size * 12 / 1024))

    canvas.alpha_composite(shadow_layer)
    canvas.alpha_composite(body, (offset, offset))
    return canvas


def png_bytes(image: Image.Image) -> bytes:
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=True)
    return buffer.getvalue()


def write_icns(path: Path) -> None:
    rendered = {size: png_bytes(render_mac_icon(size)) for size in {s for _, s in ICNS_TYPES}}
    entries = b"".join(
        kind + struct.pack(">I", len(rendered[size]) + 8) + rendered[size]
        for kind, size in ICNS_TYPES
    )
    path.write_bytes(b"icns" + struct.pack(">I", len(entries) + 8) + entries)


def write(image: Image.Image, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, format="PNG", optimize=True)
    print(f"wrote {path.relative_to(REPO_ROOT)}")


# The app window in upstream's 1448x828 README screenshot: left, top, right (exclusive) and
# corner radius. It runs off the bottom of the image.
PREVIEW_SIZE = (1448, 828)
PREVIEW_WINDOW = (152, 213, 1293, 10)


def render_preview(screenshot: Path) -> Image.Image:
    shot = Image.open(screenshot).convert("RGBA")
    if shot.size != PREVIEW_SIZE:
        raise SystemExit(f"expected a {PREVIEW_SIZE} screenshot (upstream's README image), got {shot.size}")
    canvas = render_svg(BRANDING / "preview-background.svg", shot.width, shot.height)
    left, top, right, radius = PREVIEW_WINDOW
    window = Image.new("L", shot.size, 0)
    ImageDraw.Draw(window).rounded_rectangle(
        (left, top, right - 1, shot.height + radius), radius=radius, fill=255
    )
    # Clip the window to the card's rounded corners as well.
    card = canvas.getchannel("A")
    shot.putalpha(Image.composite(card, Image.new("L", shot.size, 0), window))
    canvas.alpha_composite(shot)
    return canvas


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--preview-screenshot",
        type=Path,
        help="upstream Warp's README screenshot; re-renders docs/images/warpium-preview.png",
    )
    args = parser.parse_args()

    for size in NO_PADDING_SIZES:
        write(render_icon(size), NO_PADDING_DIR / f"{size}x{size}.png")

    ico_images = [render_icon(size) for size in ICO_SIZES]
    ico_path = NO_PADDING_DIR / "icon.ico"
    ico_images[-1].save(
        ico_path,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=ico_images[:-1],
    )
    print(f"wrote {ico_path.relative_to(REPO_ROOT)}")

    icns_path = OSS_ICON_DIR / "Warpium.icns"
    write_icns(icns_path)
    print(f"wrote {icns_path.relative_to(REPO_ROOT)}")

    write(render_mac_icon(512), DOCS_IMAGES / "warpium-icon.png")
    write(render_svg(BRANDING / "banner.svg", 2560, 800), DOCS_IMAGES / "warpium-banner.png")
    write(
        render_svg(BRANDING / "dmg-background.svg", 700, 500),
        MAC_RESOURCES / "warpium_install_image.png",
    )
    if args.preview_screenshot:
        write(render_preview(args.preview_screenshot), DOCS_IMAGES / "warpium-preview.png")


if __name__ == "__main__":
    main()
