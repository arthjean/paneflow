# Icon master files

Source PNGs for `scripts/build-icons.sh`. Drop your masters here, run the script,
commit the regenerated outputs under `assets/icons/`, `assets/PaneFlow.icns`,
`assets/PaneFlow.ico`, and `src-app/assets/icons/paneflow.png`. The MSI reads
`assets/PaneFlow.ico` directly.

| File | Required | Used for |
|---|---|---|
| `paneflow-icon-1024.png` | yes | Transparent portable mark for the Windows ICO and the GPUI runtime icon, plus Linux hicolor when no Linux master is present. |
| `paneflow-icon-linux-1024.png` | no | Full-bleed Linux artwork, Linux hicolor only. Carries no margin of its own; the script applies the 87.5% keyline. Never reaches the ICO, the ICNS, or the embedded runtime icon. |
| `paneflow-icon-macos-1024.png` | yes | Plated macOS artwork. The legacy ICNS fallback applies the Apple-style inset and rounded mask only to this source. |
| `paneflow-icon-1024-simplified.png` | no | Transparent simplified mark for sizes <= 64. When absent, the portable master is downscaled directly. |
| `paneflow-icon-template-1024.png` | no | macOS menubar Template image. Pure black silhouette on alpha, no chrome, no fill. AppKit applies the system tint at runtime. |

## Vector sources

The artwork is drawn in SVG under `svg/`, and each master is a 1024 px render
of one source:

| Source | Master |
|---|---|
| `svg/paneflow-icon-plated.svg` | `paneflow-icon-macos-1024.png` and `paneflow-icon-linux-1024.png` (the same file) |
| `svg/paneflow-icon-portable.svg` | `paneflow-icon-1024.png` |
| `svg/paneflow-icon-simplified.svg` | `paneflow-icon-1024-simplified.png` |

The glass look relies on SVG filters (`feSpecularLighting` for the edge
highlights, `feGaussianBlur` for the frosted layers and shadows), masks and
blend modes, so render the sources with a browser engine. Headless Chrome at a
1024x1024 viewport with a transparent background produced the committed
masters. Renderers without full filter support, such as rsvg-convert or
Inkscape, do not reproduce them.

The plated source shows the plate through the stream. The two transparent
sources fill the stream with the plate's indigo instead, so the mark keeps its
S on a light taskbar as well as a dark one, and their shadows stay inside the
panes so no halo surrounds the mark. The simplified source widens the stream
from 74 to 112 units for sizes <= 64.

## Regenerating

The complete cross-platform pipeline requires ImageMagick 6 or 7. It validates
the binary before writing any output, so Windows' unrelated `convert.exe`
cannot be selected accidentally.

```bash
bash scripts/build-icons.sh
git add assets/ src-app/assets/icons/paneflow.png
git commit -m "chore(brand): regenerate icons from master"
```

If no master is present the script no-ops with a warning and keeps the existing
committed icons. This is the safe state for the release pipeline.

## Linux plating geometry

`paneflow-icon-linux-1024.png` is **full-bleed**: the rounded tile fills the
whole 1024 canvas with no margin of its own beyond a 2 px inset.
`scripts/build-icons.sh` applies the keyline itself, at
`LINUX_BODY_PCT = 8750`.

The tile has continuous corners: each corner is a quarter superellipse of
exponent 2.9 spanning 318 px. That outline stays at least 2 px inside the
22.37% rounded rectangle that `resize_macos_png` cuts, so the macOS mask trims
no pixel of the plate and its rim highlight survives. Keep that property when
changing the outline, because the macOS master is the same file.

87.5% is GNOME's own 112/128 square keyline for app icons, and it is what the
neighbours on this machine actually render at:

| Icon | Tile in a 64 px canvas | Body |
|---|---|---|
| Obsidian (`MacTahoe-dark/obsidian.svg`) | `56x56+4+4` | 87.5% |
| Paneflow (this master) | `56x56+4+4` | 87.5% |
| ChatGPT (`/usr/share/pixmaps/chatgpt.png`) | `~52x52+6+6` | 80.5% |

Keep this master full-bleed. A pre-plated one would compound its own margin
with the keyline and shrink the tile to roughly 70% of the canvas.

The shipped files stay PNG, like the references: ChatGPT installs a 1024 px PNG
in `/usr/share/pixmaps`, and the Obsidian Flatpak a 512 px one. The crisp
Obsidian icon on a themed desktop comes from the icon theme, not from Obsidian.

Three earlier revisions were rejected. One copied ChatGPT's inset (an 824/1024
tile inside a 100 px margin, plus a drop shadow) and was emitted 1:1: it
measured correct but read visibly smaller than its neighbours. Another ran the
background gradient up to a pale lilac, and the contrast of the light glyph
against the light end of the tile collapsed to 2.1:1. Scaling the inner glyph
up to ChatGPT's 79% was tried and rejected as well: it reads as crowded. The
glyph spans about 65% of the tile.

This geometry is Linux-only. `assets/PaneFlow.ico`, `assets/PaneFlow.icns` and
the rust-embed runtime icon `src-app/assets/icons/paneflow.png` all stay on the
portable master.

The repository `README.md` header points at that runtime icon for the same
reason. Do not repoint it at `assets/icons/paneflow-128.png`: those files are
the Linux hicolor set, so the project page would advertise a mark that Windows,
macOS and the website do not ship.

## CI

The release workflow (`.github/workflows/release.yml`) runs the script on every
leg before the packaging steps. If you forget to commit a regenerated icon, CI
will still produce a release using fresh outputs from the committed masters --
no stale-icon shipping. The local-commit step exists so local `cargo build`
also picks up the new icons without needing ImageMagick.
