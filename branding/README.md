# Warpium brand assets

The brand follows the two reference images in [`reference/`](reference): the brand board
(`brand-board.png`) and two macOS app icons (`app-icons.png`). Everything here is drawn as
vector SVG; the references are kept only for comparison.

## Mark

Two identical rounded parallelograms, slanted at about 60 degrees and offset diagonally
(one upper left, one lower right), overlapping in the middle. The overlap is the
intersection of the lower shape's rounded top-left corner and the upper shape's rounded
bottom-right corner, which gives it a leaf form that reads like translucent glass.

| File | Use |
| --- | --- |
| `mark.svg` | colour mark for light backgrounds: indigo `#2B2170` to violet `#5B3FE0` (upper), violet `#5B42FF` to `#A886FF` (lower), pink-violet overlap `#EE98FF` to `#9D62FF` |
| `mark-on-dark.svg` | the same with a lighter upper shape (`#3A2C9C` to `#6447EC`) so it stays visible on dark backgrounds |
| `mark-white.svg` | white mark with a 72% opaque overlap, for coloured backgrounds |
| `mark-black.svg` | black mark with a 45% overlap; tinted at runtime (the get-started pane) |
| `logo-light.svg`, `logo-dark.svg` | mark plus the "Warpium" wordmark in `#1A1838` (light backgrounds) or white (dark) |

## App icon variants

All icons are full-bleed 1024 px bodies in Apple's continuous-corner shape;
`render_icons.py` places them on Apple's macOS icon grid (an 824 px body with a soft
shadow) for the `.icns` and the README icon.

1. `icon.svg`: **primary icon** (Dock, `.app`, `.dmg`, `.ico`, Linux). White mark on a
   violet gradient (`#8B5CFF` top left, `#5B3CF5`, blue `#3D6BF0` bottom right) with a soft
   cyan glow and a faint construction grid.
2. `icon-small.svg`: the primary icon at small sizes (up to `SMALL_ICON_MAX_PX` in `render_icons.py`): a larger mark, a near-opaque
   overlap, no grid or border.
3. `icon-light.svg`: colour mark on white to light lavender.
4. `icon-dark.svg`: colour mark on `#29254D` to `#1E1B3A`.
5. `icon-glass.svg`: graphite mark on frosted light blue, as in the left icon of
   `app-icons.png`.

The macOS 26 icon (`app/channels/oss/icon/AppIcon.icon`) uses three white glass layers
(upper shape, lower shape, lavender overlap) on the primary gradient, with a dark
appearance on `#29254D` to `#1E1B3A`. macOS derives the tinted and clear appearances from
the glass layers.

## Palette

| Colour | Hex |
| --- | --- |
| Ink | `#23204A` |
| Violet | `#6B45FF` |
| Lavender | `#A18CFF` |
| Mist | `#DDD8FB` |
| Paper | `#F5F3FD` |
| Wordmark on light | `#1A1838` |

Gradients: indigo to violet-pink (`#2B2170` to `#B779FF`) and pink to blue-violet
(`#F5A8F0` to `#7A6CF5`).

## Wordmark font

The wordmark and all other text in these SVGs are set in
[Manrope](https://github.com/googlefonts/manrope) (weight 650 for the wordmark, 500 to 600
for small text) and converted to outlines, so neither the app nor the README depends on
the font. Manrope is licensed under the SIL Open Font License 1.1; see
[`Manrope-OFL.txt`](Manrope-OFL.txt).

## Regenerating

`script/warpium/generate_svgs.py` is the single source of the geometry and colours. It
writes every SVG here, the in-app SVGs (`app/assets/bundled/svg/warpium-*.svg`) and the
`AppIcon.icon` layers. `script/warpium/render_icons.py` then renders the PNGs, `icon.ico`,
`Warpium.icns`, the README banner and icon, and the `.dmg` background. It re-renders the
README product preview only when given upstream's screenshot (`--preview-screenshot`).

```bash
curl -L -o /tmp/Manrope.ttf \
  'https://github.com/google/fonts/raw/main/ofl/manrope/Manrope%5Bwght%5D.ttf'
uv run script/warpium/generate_svgs.py --font /tmp/Manrope.ttf
uv run script/warpium/render_icons.py
```

`preview-background.svg` is the backdrop for `docs/images/warpium-preview.png`; see the
docstring of `render_icons.py` for how to re-composite the preview.
