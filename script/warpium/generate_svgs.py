#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["fonttools>=4.47", "uharfbuzz>=0.39"]
# ///
"""Generate Warpium's SVG brand sources from one parametric definition of the mark.

The mark is two identical rounded parallelograms slanted at about 60 degrees, offset
diagonally so that they overlap in the middle. Every SVG that shows it (the files in
`branding/`, the in-app logos in `app/assets/bundled/svg/` and the layers of the macOS 26
`AppIcon.icon`) is written by this script, so the geometry and colours live in one place.

Text is converted to outlines with the Manrope variable font (SIL Open Font License 1.1),
so nothing depends on the font at runtime. Download it once and pass its path:

    curl -L -o /tmp/Manrope.ttf \
        'https://github.com/google/fonts/raw/main/ofl/manrope/Manrope%5Bwght%5D.ttf'
    uv run script/warpium/generate_svgs.py --font /tmp/Manrope.ttf
    uv run script/warpium/render_icons.py

The generated SVGs are committed; run this only after changing the brand definition.
"""

import argparse
import math
from dataclasses import dataclass
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

REPO_ROOT = Path(__file__).resolve().parents[2]
BRANDING = REPO_ROOT / "branding"
APP_SVG = REPO_ROOT / "app" / "assets" / "bundled" / "svg"
APP_ICON_ASSETS = REPO_ROOT / "app" / "channels" / "oss" / "icon" / "AppIcon.icon" / "Assets"

# --- Brand palette (the "Цвета" swatches of the brand board). ---------------------------
INK = "#1A1838"  # wordmark on light backgrounds
PALETTE = ["#23204A", "#6B45FF", "#A18CFF", "#DDD8FB", "#F5F3FD"]

# --- Mark geometry, in mark units (y grows downwards). -----------------------------------
SHAPE_H = 240.0  # height of one parallelogram
SHAPE_W = 290.0  # horizontal length of its top and bottom edges
SHEAR = SHAPE_H * math.tan(math.radians(31))  # top edge shifted right: sides at ~59 degrees
OFFSET_X = 110.0  # the lower-right shape relative to the upper-left one
OFFSET_Y = 100.0
OBTUSE_CUT = 84.0  # how far the rounding of the 120-degree corners reaches along each edge
ACUTE_CUT = 34.0  # the same for the 60-degree corners
MARK_W = SHAPE_W + SHEAR + OFFSET_X
MARK_H = SHAPE_H + OFFSET_Y

Point = tuple[float, float]


def _fmt(value: float) -> str:
    text = f"{value:.2f}".rstrip("0").rstrip(".")
    return "0" if text in ("-0", "") else text


def _pt(p: Point) -> str:
    return f"{_fmt(p[0])} {_fmt(p[1])}"


def _lerp(a: Point, b: Point, t: float) -> Point:
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def _toward(a: Point, b: Point, dist: float) -> Point:
    length = math.dist(a, b)
    return _lerp(a, b, dist / length)


def _corner(prev: Point, vertex: Point, nxt: Point, cut: float) -> tuple[Point, str]:
    """Round `vertex` with a cubic that approximates a circular arc.

    Returns the start point of the rounding and the path segment that ends on the next edge.
    """
    start = _toward(vertex, prev, cut)
    end = _toward(vertex, nxt, cut)
    v1 = (prev[0] - vertex[0], prev[1] - vertex[1])
    v2 = (nxt[0] - vertex[0], nxt[1] - vertex[1])
    interior = math.acos(
        (v1[0] * v2[0] + v1[1] * v2[1]) / (math.hypot(*v1) * math.hypot(*v2))
    )
    sweep = math.pi - interior
    radius = cut * math.tan(interior / 2)
    handle = 4 / 3 * math.tan(sweep / 4) * radius
    c1 = _toward(start, vertex, handle)
    c2 = _toward(end, vertex, handle)
    return start, f"C{_pt(c1)} {_pt(c2)} {_pt(end)}"


def _vertices(ox: float, oy: float) -> list[Point]:
    """Top-left, top-right, bottom-right, bottom-left."""
    return [
        (ox + SHEAR, oy),
        (ox + SHEAR + SHAPE_W, oy),
        (ox + SHAPE_W, oy + SHAPE_H),
        (ox, oy + SHAPE_H),
    ]


def _cut(index: int) -> float:
    # The top-left and bottom-right corners are obtuse.
    return OBTUSE_CUT if index % 2 == 0 else ACUTE_CUT


def shape_path(ox: float, oy: float) -> str:
    verts = _vertices(ox, oy)
    parts = []
    for i, vertex in enumerate(verts):
        start, curve = _corner(verts[i - 1], vertex, verts[(i + 1) % 4], _cut(i))
        parts.append(("M" if i == 0 else "L") + _pt(start))
        parts.append(curve)
    return "".join(parts) + "Z"


def upper_path() -> str:
    return shape_path(0, 0)


def lower_path() -> str:
    return shape_path(OFFSET_X, OFFSET_Y)


def _overlap_points() -> tuple[Point, Point]:
    """Where the upper shape's right edge meets the lower shape's top edge (top-right of
    the overlap), and where its bottom edge meets the lower shape's left edge."""
    top_right = (SHAPE_W + SHEAR * (SHAPE_H - OFFSET_Y) / SHAPE_H, OFFSET_Y)
    bottom_left = (OFFSET_X + SHEAR * OFFSET_Y / SHAPE_H, SHAPE_H)
    return top_right, bottom_left


def overlap_path() -> str:
    """The intersection of the two shapes: bounded by the lower shape's rounded top-left
    corner and the upper shape's rounded bottom-right corner, which gives it a leaf form."""
    upper = _vertices(0, 0)
    lower = _vertices(OFFSET_X, OFFSET_Y)
    top_right, bottom_left = _overlap_points()
    lower_start, lower_curve = _corner(lower[1], lower[0], lower[3], OBTUSE_CUT)
    upper_start, upper_curve = _corner(upper[3], upper[2], upper[1], OBTUSE_CUT)
    return (
        f"M{_pt(top_right)}L{_pt(lower_start)}{lower_curve}"
        f"L{_pt(bottom_left)}L{_pt(upper_start)}{upper_curve}Z"
    )


# --- Paints. ------------------------------------------------------------------------------


@dataclass
class Gradient:
    stops: list[tuple[float, str]]
    start: Point
    end: Point

    def defs(self, gid: str) -> str:
        stops = "".join(f'<stop offset="{_fmt(o)}" stop-color="{c}"/>' for o, c in self.stops)
        return (
            f'<linearGradient id="{gid}" x1="{_fmt(self.start[0])}" y1="{_fmt(self.start[1])}" '
            f'x2="{_fmt(self.end[0])}" y2="{_fmt(self.end[1])}" gradientUnits="userSpaceOnUse">'
            f"{stops}</linearGradient>"
        )


@dataclass
class MarkStyle:
    upper: Gradient | str
    lower: Gradient | str
    overlap: Gradient | str
    # Monochrome styles punch the overlap out of the two shapes and fill it with the same
    # colour at this opacity, so a tinted icon keeps the "glass" overlap.
    overlap_opacity: float | None = None


_TOP_RIGHT, _BOTTOM_LEFT = _overlap_points()

COLOR = MarkStyle(
    upper=Gradient([(0, "#2B2170"), (0.55, "#35278F"), (1, "#5B3FE0")], (SHEAR, 0), (SHAPE_W, SHAPE_H)),
    lower=Gradient(
        [(0, "#5B42FF"), (0.5, "#6B4BFF"), (1, "#A886FF")],
        (OFFSET_X + SHEAR, OFFSET_Y),
        (OFFSET_X + SHAPE_W, OFFSET_Y + SHAPE_H),
    ),
    overlap=Gradient([(0, "#EE98FF"), (0.45, "#D58CFF"), (1, "#9D62FF")], _BOTTOM_LEFT, _TOP_RIGHT),
)
# On dark surfaces the deep indigo of the upper shape is lifted so it does not disappear.
COLOR_ON_DARK = MarkStyle(
    upper=Gradient([(0, "#3A2C9C"), (0.55, "#4632B8"), (1, "#6447EC")], (SHEAR, 0), (SHAPE_W, SHAPE_H)),
    lower=COLOR.lower,
    overlap=COLOR.overlap,
)
WHITE = MarkStyle("#FFFFFF", "#FFFFFF", "#FFFFFF", overlap_opacity=0.72)
WHITE_SMALL = MarkStyle("#FFFFFF", "#FFFFFF", "#FFFFFF", overlap_opacity=0.86)
BLACK = MarkStyle("#000000", "#000000", "#000000", overlap_opacity=0.45)
GRAPHITE = MarkStyle(
    upper=Gradient([(0, "#1B1D22"), (1, "#15171B")], (SHEAR, 0), (SHAPE_W, SHAPE_H)),
    lower=Gradient(
        [(0, "#2A2E35"), (1, "#353A42")],
        (OFFSET_X + SHEAR, OFFSET_Y),
        (OFFSET_X + SHAPE_W, OFFSET_Y + SHAPE_H),
    ),
    overlap=Gradient([(0, "#B9C7D4"), (1, "#7D8C9C")], _BOTTOM_LEFT, _TOP_RIGHT),
)


def _paint(paint: Gradient | str, gid: str, defs: list[str]) -> str:
    if isinstance(paint, str):
        return paint
    defs.append(paint.defs(gid))
    return f"url(#{gid})"


def mark(style: MarkStyle, prefix: str, transform: str = "") -> tuple[str, str]:
    """Return (defs, body) for the mark in mark units, optionally transformed."""
    defs: list[str] = []
    upper = _paint(style.upper, f"{prefix}-upper", defs)
    lower = _paint(style.lower, f"{prefix}-lower", defs)
    overlap = _paint(style.overlap, f"{prefix}-overlap", defs)
    attr = f' transform="{transform}"' if transform else ""
    if style.overlap_opacity is None:
        body = (
            f"<g{attr}>"
            f'<path d="{upper_path()}" fill="{upper}"/>'
            f'<path d="{lower_path()}" fill="{lower}"/>'
            f'<path d="{overlap_path()}" fill="{overlap}"/>'
            "</g>"
        )
    else:
        body = (
            f"<g{attr}>"
            f'<path d="{upper_path()}{lower_path()}" fill="{upper}" fill-rule="evenodd"/>'
            f'<path d="{overlap_path()}" fill="{overlap}" fill-opacity="{_fmt(style.overlap_opacity)}"/>'
            "</g>"
        )
    return "".join(defs), body


def mark_transform(cx: float, cy: float, width: float) -> str:
    scale = width / MARK_W
    x = cx - MARK_W * scale / 2
    y = cy - MARK_H * scale / 2
    return f"translate({_fmt(x)} {_fmt(y)}) scale({scale:.5f})"


# --- Text outlines. ------------------------------------------------------------------------


class TextOutliner:
    def __init__(self, font_path: Path):
        self.blob = hb.Blob.from_file_path(str(font_path))
        self.face = hb.Face(self.blob)
        self.upem = self.face.upem
        self.font_path = font_path
        self.variable = TTFont(font_path)
        self.instances: dict[int, TTFont] = {}

    def _instance(self, weight: int) -> TTFont:
        if weight not in self.instances:
            self.instances[weight] = instancer.instantiateVariableFont(
                TTFont(self.font_path), {"wght": weight}
            )
        return self.instances[weight]

    def outline(self, text: str, size: float, weight: int, tracking: float = 0.0) -> tuple[str, float]:
        """Return the SVG path for `text` with its baseline at y=0, and its advance width.

        `tracking` is extra letter spacing in em.
        """
        font = hb.Font(self.face)
        font.set_variations({"wght": weight})
        buf = hb.Buffer()
        buf.add_str(text)
        buf.guess_segment_properties()
        hb.shape(font, buf, {"kern": True, "liga": True})
        instance = self._instance(weight)
        glyph_set = instance.getGlyphSet()
        order = instance.getGlyphOrder()
        scale = size / self.upem
        pen = SVGPathPen(glyph_set, ntos=lambda v: _fmt(v))
        x = 0.0
        last = len(buf.glyph_infos) - 1
        for i, (info, pos) in enumerate(zip(buf.glyph_infos, buf.glyph_positions)):
            name = order[info.codepoint]
            transform = (scale, 0, 0, -scale, (x + pos.x_offset) * scale, -pos.y_offset * scale)
            glyph_set[name].draw(TransformPen(pen, transform))
            x += pos.x_advance + (tracking * self.upem if i < last else 0)
        return pen.getCommands(), x * scale

    def cap_height(self, size: float) -> float:
        return self.variable["OS/2"].sCapHeight * size / self.upem


# --- SVG documents. ------------------------------------------------------------------------


def svg_doc(width: float, height: float, body: str, comment: str, defs: str = "") -> str:
    defs_block = f"<defs>{defs}</defs>\n" if defs else ""
    return (
        f'<svg width="{_fmt(width)}" height="{_fmt(height)}" viewBox="0 0 {_fmt(width)} {_fmt(height)}" '
        f'fill="none" xmlns="http://www.w3.org/2000/svg">\n'
        f"<!-- {comment} -->\n{defs_block}{body}\n</svg>\n"
    )


def squircle_path(size: float, inset: float = 0.0) -> str:
    """Apple's app icon shape: a rounded square whose corners blend smoothly into the sides
    (a "continuous" corner of radius ~22.5% of the size, starting ~1.5 radii from the corner)."""
    lo, hi = inset, size - inset
    reach = (hi - lo) * 0.34  # where the corner curve leaves the straight side
    handle = (hi - lo) * 0.075  # distance of the control points from the corner
    return (
        f"M{_pt((lo + reach, lo))}L{_pt((hi - reach, lo))}"
        f"C{_pt((hi - handle, lo))} {_pt((hi, lo + handle))} {_pt((hi, lo + reach))}"
        f"L{_pt((hi, hi - reach))}"
        f"C{_pt((hi, hi - handle))} {_pt((hi - handle, hi))} {_pt((hi - reach, hi))}"
        f"L{_pt((lo + reach, hi))}"
        f"C{_pt((lo + handle, hi))} {_pt((lo, hi - handle))} {_pt((lo, hi - reach))}"
        f"L{_pt((lo, lo + reach))}"
        f"C{_pt((lo, lo + handle))} {_pt((lo + handle, lo))} {_pt((lo + reach, lo))}Z"
    )


def mark_only(style: MarkStyle, prefix: str, pad: float = 0.0) -> str:
    defs, body = mark(style, prefix, f"translate({_fmt(pad)} {_fmt(pad)})" if pad else "")
    return svg_doc(MARK_W + 2 * pad, MARK_H + 2 * pad, body, "Warpium mark. Generated by script/warpium/generate_svgs.py.", defs)


def icon_svg(variant: str, small: bool = False) -> str:
    """A full-bleed 1024 px app icon body (the squircle edge touches the canvas edge)."""
    size = 1024
    shape = squircle_path(size)
    defs = [f'<clipPath id="squircle"><path d="{shape}"/></clipPath>']
    layers = []
    mark_width = 740 if small else 670
    if variant == "brand":
        defs.append(
            Gradient([(0, "#8B5CFF"), (0.55, "#5B3CF5"), (1, "#3D6BF0")], (160, 0), (720, 1024)).defs("bg")
        )
        layers.append(f'<rect width="{size}" height="{size}" fill="url(#bg)"/>')
        if not small:
            defs.append(
                '<radialGradient id="glow" cx="512" cy="500" r="420" gradientUnits="userSpaceOnUse">'
                '<stop offset="0" stop-color="#8FE3FF" stop-opacity="0.55"/>'
                '<stop offset="0.45" stop-color="#6FB8FF" stop-opacity="0.22"/>'
                '<stop offset="1" stop-color="#6FB8FF" stop-opacity="0"/></radialGradient>'
            )
            layers.append(f'<rect width="{size}" height="{size}" fill="url(#glow)"/>')
            layers.append(construction_grid(size))
        # At 16-32 px a fainter overlap blurs into the background; keep it nearly white.
        style, prefix = (WHITE_SMALL if small else WHITE), "mark"
    elif variant == "light":
        defs.append(Gradient([(0, "#FFFFFF"), (1, "#EEEAFD")], (0, 0), (0, 1024)).defs("bg"))
        layers.append(f'<rect width="{size}" height="{size}" fill="url(#bg)"/>')
        style, prefix = COLOR, "mark"
    elif variant == "dark":
        defs.append(Gradient([(0, "#29254D"), (1, "#1A1733")], (0, 0), (0, 1024)).defs("bg"))
        layers.append(f'<rect width="{size}" height="{size}" fill="url(#bg)"/>')
        style, prefix = COLOR_ON_DARK, "mark"
    elif variant == "glass":
        defs.append(Gradient([(0, "#CFE0EE"), (0.6, "#B3CBDF"), (1, "#A8C3DA")], (0, 0), (0, 1024)).defs("bg"))
        defs.append(
            '<radialGradient id="haze" cx="300" cy="1010" r="620" gradientUnits="userSpaceOnUse">'
            '<stop offset="0" stop-color="#FFFFFF" stop-opacity="0.45"/>'
            '<stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/></radialGradient>'
        )
        layers.append(f'<rect width="{size}" height="{size}" fill="url(#bg)"/>')
        layers.append(f'<rect width="{size}" height="{size}" fill="url(#haze)"/>')
        style, prefix = GRAPHITE, "mark"
    else:
        raise ValueError(variant)
    mark_defs, mark_body = mark(style, prefix, mark_transform(512, 512, mark_width))
    defs.append(mark_defs)
    layers.append(mark_body)
    border = "#FFFFFF" if variant in ("brand", "dark", "glass") else "#5B3FE0"
    border_opacity = {"brand": 0.22, "dark": 0.10, "glass": 0.6, "light": 0.10}[variant]
    if not small:
        layers.append(
            f'<path d="{squircle_path(size, inset=4)}" stroke="{border}" stroke-opacity="{border_opacity}" stroke-width="8"/>'
        )
    body = f'<g clip-path="url(#squircle)">{"".join(layers)}</g>'
    note = "tuned for 16-32 px: bigger mark, no grid or border" if small else "full bleed"
    return svg_doc(size, size, body, f"Warpium app icon, {variant} variant ({note}). Generated by script/warpium/generate_svgs.py.", "".join(defs))


def construction_grid(size: int) -> str:
    """A faint Apple-style icon construction grid."""
    fine = "".join(
        f'<path d="M{v} 0V{size}M0 {v}H{size}"/>' for v in range(64, size, 64)
    )
    c = size / 2
    keylines = (
        f'<path d="M0 0L{size} {size}M{size} 0L0 {size}M{c} 0V{size}M0 {c}H{size}"/>'
        f'<circle cx="{c}" cy="{c}" r="{size * 0.19:.0f}"/>'
        f'<circle cx="{c}" cy="{c}" r="{size * 0.39:.0f}"/>'
        f'<rect x="{size * 0.16:.0f}" y="{size * 0.16:.0f}" width="{size * 0.68:.0f}" height="{size * 0.68:.0f}"/>'
    )
    return (
        f'<g stroke="#FFFFFF" fill="none">'
        f'<g stroke-opacity="0.07" stroke-width="2">{fine}</g>'
        f'<g stroke-opacity="0.16" stroke-width="2.5">{keylines}</g></g>'
    )


def logo_svg(text: TextOutliner, on_dark: bool) -> str:
    """Mark plus "Warpium" wordmark, for the About page."""
    height = 160
    mark_h = 112
    mark_w = MARK_W * mark_h / MARK_H
    pad = 12
    size = 84
    word, advance = text.outline("Warpium", size, 650, tracking=-0.012)
    cap = text.cap_height(size)
    gap = 26
    width = pad + mark_w + gap + advance + pad
    baseline = height / 2 + cap / 2
    style = COLOR_ON_DARK if on_dark else COLOR
    mark_defs, mark_body = mark(style, "logo", mark_transform(pad + mark_w / 2, height / 2, mark_w))
    fill = "#FFFFFF" if on_dark else INK
    body = mark_body + f'<path transform="translate({_fmt(pad + mark_w + gap)} {_fmt(baseline)})" fill="{fill}" d="{word}"/>'
    variant = "white title for dark backgrounds" if on_dark else f"{INK} title for light backgrounds"
    return svg_doc(round(width), height, body, f"Warpium logo with wordmark, {variant}. Generated by script/warpium/generate_svgs.py.", mark_defs)


def banner_svg(text: TextOutliner) -> str:
    width, height = 1280, 400
    defs = [
        f'<clipPath id="card"><rect width="{width}" height="{height}" rx="32"/></clipPath>',
        Gradient([(0, "#25204F"), (0.6, "#1F1B45"), (1, "#1C1840")], (0, 0), (width, height)).defs("bg"),
        '<radialGradient id="glow" cx="80" cy="420" r="520" gradientUnits="userSpaceOnUse">'
        '<stop offset="0" stop-color="#4A37C8" stop-opacity="0.75"/>'
        '<stop offset="1" stop-color="#4A37C8" stop-opacity="0"/></radialGradient>',
        '<radialGradient id="glow-right" cx="1050" cy="250" r="420" gradientUnits="userSpaceOnUse">'
        '<stop offset="0" stop-color="#7E5BFF" stop-opacity="0.35"/>'
        '<stop offset="1" stop-color="#7E5BFF" stop-opacity="0"/></radialGradient>',
    ]
    big = MarkStyle(
        upper=Gradient([(0, "#2E2580"), (0.45, "#4A3AD6"), (1, "#7563FF")], (0, SHAPE_H), (SHAPE_W + SHEAR, 0)),
        lower=Gradient(
            [(0, "#5B3FE8"), (0.6, "#6F4CFA"), (1, "#5A4BE0")],
            (OFFSET_X + SHEAR, OFFSET_Y),
            (OFFSET_X + SHAPE_W, OFFSET_Y + SHAPE_H),
        ),
        overlap=Gradient([(0, "#F79CFF"), (0.5, "#C784FF"), (1, "#8E66FF")], _BOTTOM_LEFT, _TOP_RIGHT),
    )
    big_defs, big_body = mark(big, "big", "translate(650 -6) scale(1.46)")
    defs.append(big_defs)
    title, _ = text.outline("Warpium", 96, 650, tracking=-0.012)
    line1, _ = text.outline("A lightweight IDE for AI-driven", 32, 500)
    line2, _ = text.outline("mobile development, built on Warp", 32, 500)
    body = (
        '<g clip-path="url(#card)">'
        f'<rect width="{width}" height="{height}" fill="url(#bg)"/>'
        f'<rect width="{width}" height="{height}" fill="url(#glow)"/>'
        f'<rect width="{width}" height="{height}" fill="url(#glow-right)"/>'
        f'<g opacity="0.92">{big_body}</g>'
        "</g>"
        f'<path transform="translate(96 196)" fill="#FFFFFF" d="{title}"/>'
        f'<path transform="translate(100 262)" fill="#C7BEF6" d="{line1}"/>'
        f'<path transform="translate(100 306)" fill="#C7BEF6" d="{line2}"/>'
    )
    return svg_doc(width, height, body, "README banner. Rendered to docs/images/warpium-banner.png by script/warpium/render_icons.py.", "".join(defs))


def preview_header_svg(text: TextOutliner) -> str:
    """Background and header for docs/images/warpium-preview.png; render_icons.py places the
    upstream product screenshot's window on top of it."""
    width, height = 1448, 828
    defs = [
        f'<clipPath id="card"><rect width="{width}" height="{height}" rx="32"/></clipPath>',
        Gradient([(0, "#26214F"), (1, "#1A1739")], (0, 0), (width, height)).defs("bg"),
        '<radialGradient id="glow" cx="724" cy="40" r="620" gradientUnits="userSpaceOnUse">'
        '<stop offset="0" stop-color="#5A41E6" stop-opacity="0.55"/>'
        '<stop offset="1" stop-color="#5A41E6" stop-opacity="0"/></radialGradient>',
    ]
    faint = MarkStyle("#7C5CFF", "#8D6BFF", "#E59BFF")
    left_defs, left = mark(faint, "bl", "translate(-260 330) scale(1.4)")
    right_defs, right = mark(faint, "br", "translate(1060 -120) scale(1.3)")
    defs += [left_defs, right_defs]
    mark_h = 92
    mark_w = MARK_W * mark_h / MARK_H
    word, advance = text.outline("Warpium", 80, 650, tracking=-0.012)
    cap = text.cap_height(80)
    gap = 24
    total = mark_w + gap + advance
    x0 = (width - total) / 2
    cy = 112
    logo_defs, logo = mark(COLOR_ON_DARK, "logo", mark_transform(x0 + mark_w / 2, cy, mark_w))
    defs.append(logo_defs)
    body = (
        '<g clip-path="url(#card)">'
        f'<rect width="{width}" height="{height}" fill="url(#bg)"/>'
        f'<rect width="{width}" height="{height}" fill="url(#glow)"/>'
        f'<g opacity="0.16">{left}{right}</g>'
        "</g>"
        f"{logo}"
        f'<path transform="translate({_fmt(x0 + mark_w + gap)} {_fmt(cy + cap / 2)})" fill="#FFFFFF" d="{word}"/>'
    )
    return svg_doc(width, height, body, "README product preview background. Composited by script/warpium/render_icons.py.", "".join(defs))


def dmg_background_svg(text: TextOutliner) -> str:
    """The .dmg window (700x500). The app icon sits at (150, 250) and the Applications link
    at (550, 250); see `create_warp_dmg` in script/macos/bundle."""
    width, height = 700, 500
    defs = [
        Gradient([(0, "#26214F"), (1, "#18152F")], (0, 0), (width, height)).defs("bg"),
        Gradient([(0, "#A18CFF"), (1, "#6B45FF")], (262, 0), (438, 0)).defs("arrow"),
    ]
    faint = MarkStyle("#7C5CFF", "#8D6BFF", "#E59BFF")
    shapes_defs, shapes = mark(faint, "bg", "translate(380 300) scale(0.9)")
    defs.append(shapes_defs)
    caption, cap_w = text.outline("Drag Warpium to Applications", 13, 600)
    footer, foot_w = text.outline("Warpium", 30, 650, tracking=-0.012)
    mark_h = 26
    mark_w = MARK_W * mark_h / MARK_H
    footer_total = mark_w + 10 + foot_w
    fx = (width - footer_total) / 2
    logo_defs, logo = mark(COLOR_ON_DARK, "logo", mark_transform(fx + mark_w / 2, 440, mark_w))
    defs.append(logo_defs)
    body = (
        f'<rect width="{width}" height="{height}" fill="url(#bg)"/>'
        f'<g opacity="0.10">{shapes}</g>'
        f'<path transform="translate({_fmt((width - cap_w) / 2)} 226)" fill="#DDD8FB" fill-opacity="0.85" d="{caption}"/>'
        '<path d="M262 250H430" stroke="url(#arrow)" stroke-width="4" stroke-linecap="round"/>'
        '<path d="M418 238L432 250L418 262" stroke="#6B45FF" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>'
        f'<g opacity="0.7">{logo}'
        f'<path transform="translate({_fmt(fx + mark_w + 10)} 451)" fill="#FFFFFF" d="{footer}"/></g>'
    )
    return svg_doc(width, height, body, ".dmg window background. Rendered to app/assets/resources/mac/warpium_install_image.png by script/warpium/render_icons.py.", "".join(defs))


def icon_composer_layers() -> dict[str, str]:
    """White layers for the macOS 26 AppIcon.icon, on Icon Composer's 1024 px canvas."""
    t = mark_transform(512, 512, 640)
    header = "Layer of the Warpium AppIcon.icon. Generated by script/warpium/generate_svgs.py."
    return {
        "warpium-upper.svg": svg_doc(1024, 1024, f'<path transform="{t}" d="{upper_path()}" fill="#FFFFFF"/>', header),
        "warpium-lower.svg": svg_doc(1024, 1024, f'<path transform="{t}" d="{lower_path()}" fill="#FFFFFF"/>', header),
        "warpium-overlap.svg": svg_doc(1024, 1024, f'<path transform="{t}" d="{overlap_path()}" fill="#DCD6FF"/>', header),
    }


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    print(f"wrote {path.relative_to(REPO_ROOT)}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--font", type=Path, required=True, help="path to the Manrope variable TTF")
    args = parser.parse_args()
    text = TextOutliner(args.font)

    write(BRANDING / "mark.svg", mark_only(COLOR, "mark"))
    write(BRANDING / "mark-on-dark.svg", mark_only(COLOR_ON_DARK, "mark"))
    write(BRANDING / "mark-white.svg", mark_only(WHITE, "mark"))
    write(BRANDING / "mark-black.svg", mark_only(BLACK, "mark"))
    write(BRANDING / "logo-light.svg", logo_svg(text, on_dark=False))
    write(BRANDING / "logo-dark.svg", logo_svg(text, on_dark=True))
    write(BRANDING / "icon.svg", icon_svg("brand"))
    write(BRANDING / "icon-small.svg", icon_svg("brand", small=True))
    for variant in ("light", "dark", "glass"):
        write(BRANDING / f"icon-{variant}.svg", icon_svg(variant))
    write(BRANDING / "banner.svg", banner_svg(text))
    write(BRANDING / "preview-background.svg", preview_header_svg(text))
    write(BRANDING / "dmg-background.svg", dmg_background_svg(text))

    write(APP_SVG / "warpium-mark.svg", mark_only(COLOR, "warpium-mark"))
    write(APP_SVG / "warpium-mark-on-dark.svg", mark_only(COLOR_ON_DARK, "warpium-mark"))
    write(APP_SVG / "warpium-mark-neutral.svg", mark_only(BLACK, "warpium-mark"))
    write(APP_SVG / "warpium-logo-with-dark-title.svg", logo_svg(text, on_dark=False))
    write(APP_SVG / "warpium-logo-with-light-title.svg", logo_svg(text, on_dark=True))
    for name, content in icon_composer_layers().items():
        write(APP_ICON_ASSETS / name, content)


if __name__ == "__main__":
    main()
