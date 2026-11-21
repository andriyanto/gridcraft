# GridKu app icon

GridKu's icon, designed by andriyanto (20 November 2026): an emerald-green G above an open book
whose pages read "Grid" and "Ku", on a brick-red tile with rounded corners.

The letters on the pages are outlines of DejaVu Sans Condensed Bold glyphs set along the curve of
each page, so the icon looks the same everywhere without depending on installed fonts (the
owner's original SVG set them as text with Helvetica/Arial).

## Palette

| Colour | Hex | Used for |
|---|---|---|
| Brick red | `#B5472B` | The tile and the letters on the pages |
| Emerald | `#50C878` | The G |
| Ghost white | `#F8F8FF` | The pages |

## Geometry

A 512 × 512 viewBox with a rounded square (`rx=104`). Windows and Linux use the full tile. The
macOS icons (`.icns`, `gridcraft-macos-512.png`) sit on Apple's grid: an 824/1024 body with a
transparent margin.

## Files

| File | Use |
|---|---|
| `gridcraft.svg` | Canonical artwork; also `hicolor/scalable` |
| `gridcraft-1024.png` | 1024 px render of the full tile |
| `gridcraft-macos-512.png` | Runtime Dock icon on macOS (for `apps/gridcraft` to embed) |
| `gridcraft.icns` | macOS bundle icon (`CFBundleIconFile`) |
| `gridcraft.ico` | Windows executable and installer icon |
| `hicolor/` | Linux icon theme sizes |

The file names keep `gridcraft`, like the package names. Regenerate everything with
`packaging/icons.sh` after changing `gridcraft.svg`.
