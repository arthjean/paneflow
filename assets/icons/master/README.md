# Icon master files

Source PNGs for `scripts/build-icons.sh`. Drop your masters here, run the script,
commit the regenerated outputs under `assets/icons/`, `assets/PaneFlow.icns`,
`assets/PaneFlow.ico`, `packaging/wix/paneflow.ico`, and
`src-app/assets/icons/paneflow.png`.

| File | Required | Used for |
|---|---|---|
| `paneflow-icon-1024.png` | yes | Transparent portable mark for the Windows ICO and the GPUI runtime icon, plus Linux hicolor when no Linux master is present. |
| `paneflow-icon-linux-1024.png` | no | Full-bleed Linux artwork, Linux hicolor only. Carries no margin of its own; the script applies the 87.5% keyline. Never reaches the ICO, the ICNS, or the embedded runtime icon. |
| `paneflow-icon-macos-1024.png` | yes | Plated macOS artwork. The legacy ICNS fallback applies the Apple-style inset and rounded mask only to this source. |
| `paneflow-icon-1024-simplified.png` | no | Transparent simplified mark for sizes <= 64. When absent, the portable master is downscaled directly. |
| `paneflow-icon-template-1024.png` | no | macOS menubar Template image. Pure black silhouette on alpha, no chrome, no fill. AppKit applies the system tint at runtime. |

## Regenerating

The complete cross-platform pipeline requires ImageMagick 6 or 7. It validates
the binary before writing any output, so Windows' unrelated `convert.exe`
cannot be selected accidentally.

```bash
bash scripts/build-icons.sh
git add assets/ packaging/wix/paneflow.ico src-app/assets/icons/paneflow.png
git commit -m "chore(brand): regenerate icons from master"
```

If no master is present the script no-ops with a warning and keeps the existing
committed icons. This is the safe state for the release pipeline.

## Linux plating geometry

`paneflow-icon-linux-1024.png` is **full-bleed**: the rounded tile fills the
whole 1024 canvas (`rx` 260, i.e. a 25.4% corner radius) with no margin of its
own. `scripts/build-icons.sh` applies the keyline itself, at
`LINUX_BODY_PCT = 8750`.

87.5% is GNOME's own 112/128 square keyline for app icons, and it is what the
neighbours on this machine actually render at:

| Icon | Tile in a 64 px canvas | Body |
|---|---|---|
| Obsidian (`MacTahoe-dark/obsidian.svg`) | `56x56+4+4` | 87.5% |
| Paneflow (this master) | `56x56+4+4` | 87.5% |
| ChatGPT (`/usr/share/pixmaps/chatgpt.png`) | `~52x52+6+6` | 80.5% |

Keep this master full-bleed. A pre-plated one would compound its own margin
with the keyline and shrink the tile to roughly 70% of the canvas.

The master is a 4096x4096 PNG export downsampled to 1024 with Lanczos. There is
no vector source: the artwork is raster, and the Figma SVG export only wrapped
the same bitmap in a `<pattern>`, so it bought nothing. This matches the
references, which both ship PNG too: ChatGPT installs a 1024 px PNG in
`/usr/share/pixmaps`, and the Obsidian Flatpak a 512 px one. The crisp Obsidian
icon on a themed desktop comes from the icon theme, not from Obsidian.

Two earlier revisions were rejected. One copied ChatGPT's inset (an 824/1024
tile inside a 100 px margin, plus a drop shadow) and was emitted 1:1: it
measured correct but read visibly smaller than its neighbours. Another kept
this geometry but ran the background gradient up to a pale lilac; because the
glyph is white and blends in luminosity mode, its contrast against the light
end of the tile collapsed to 2.1:1. Scaling the inner glyph up to ChatGPT's 79%
was tried and rejected as well: it reads as crowded.

This geometry is Linux-only. `assets/PaneFlow.ico`, `assets/PaneFlow.icns`,
`packaging/wix/paneflow.ico` and the rust-embed runtime icon
`src-app/assets/icons/paneflow.png` all stay on the portable master.

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
