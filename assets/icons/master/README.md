# Icon master files

Source PNGs for `scripts/build-icons.sh`. Drop your masters here, run the script,
commit the regenerated outputs under `assets/icons/`, `assets/PaneFlow.icns`,
`assets/PaneFlow.ico`, and `src-app/assets/icons/paneflow.png`. The MSI reads
`assets/PaneFlow.ico` directly.

| File | Required | Used for |
|---|---|---|
| `paneflow-icon-1024.png` | yes | Transparent portable mark for the GPUI runtime icon, plus the Windows ICO when no Windows master is present and Linux hicolor when no Linux master is present. |
| `paneflow-icon-linux-1024.png` | no | Full-bleed Linux artwork, Linux hicolor only. Carries no margin of its own; the script applies the 87.5% keyline. Never reaches the ICO, the ICNS, or the embedded runtime icon. |
| `paneflow-icon-windows-1024.png` | no | Full-bleed Windows artwork, Windows ICO only. The script emits it with no inset. Never reaches the ICNS, Linux hicolor, or the embedded runtime icon. |
| `paneflow-icon-windows-1024-simplified.png` | no | Full-bleed Windows artwork for ICO sizes <= 64. When absent, the Windows master is downscaled directly. |
| `paneflow-icon-macos-1024.png` | yes | Plated macOS artwork. The legacy ICNS fallback applies the Apple-style inset and rounded mask only to this source. |
| `paneflow-icon-1024-simplified.png` | no | Transparent simplified mark for sizes <= 64, used wherever the portable master is. Not shipped today: with a Linux and a Windows master present, no size <= 64 reads the portable master. |
| `paneflow-icon-template-1024.png` | no | macOS menubar Template image. Pure black silhouette on alpha, no chrome, no fill. AppKit applies the system tint at runtime. |

## Vector sources

The artwork is drawn in SVG under `svg/`, and each master is a 1024 px render
of one source:

| Source | Master |
|---|---|
| `svg/paneflow-icon-plated.svg` | `paneflow-icon-macos-1024.png`, `paneflow-icon-linux-1024.png` and `paneflow-icon-windows-1024.png` (the same file) |
| `svg/paneflow-icon-plated-simplified.svg` | `paneflow-icon-windows-1024-simplified.png` |
| `svg/paneflow-icon-portable.svg` | `paneflow-icon-1024.png` |

The glass look relies on SVG filters (`feSpecularLighting` for the edge
highlights, `feGaussianBlur` for the frosted layers and shadows), masks and
blend modes, so render the sources with a browser engine. Headless Chrome at a
1024x1024 viewport with a transparent background produced the committed
masters. Renderers without full filter support, such as rsvg-convert or
Inkscape, do not reproduce them.

The plated sources show the plate through the stream. The transparent source
fills the stream with the plate's indigo instead, so the mark keeps its S on a
light background as well as a dark one, and its shadows stay inside the panes
so no halo surrounds the mark. The simplified plated source widens the stream
from 74 to 108 units so the S survives at 16 and 24 px. It keeps the glyph at
the size of the full source: an earlier version enlarged it by 12%, and the
smaller padding inside the tile was visible in the taskbar next to the large
renders.

## App icon variants

Settings > Appearance > App icon offers twenty more icons besides the glass
one: the mark drawn in horizontal bars, on a black tile and on a white tile,
the same panes as solid shapes on the same two tiles, and sixteen illustrated
skins. They
never reach the ICO, the ICNS, or Linux hicolor: the installed icon stays the
glass one, and `src-app/src/app_icon.rs` applies a variant to the running app
from the PNGs under `src-app/assets/icons/app/`. On Windows it also encodes
those PNGs into `icons/paneflow-<name>-<hash>.ico` in the Paneflow home and
points the user's own shortcuts to this Paneflow at that file, because a pinned
taskbar button shows the icon of its shortcut, not of the window. Windows
caches the image of that button and reads the shortcut again only when it
reloads the taskbar, at sign-in or when Explorer restarts: no shell
notification refreshed it on Windows 11 build 26300. A machine-wide Start
menu shortcut cannot be rewritten, so Paneflow copies it to the same
relative path in the user's Start menu folder, which the shell prefers,
lists the copy in `icons/start-menu-shortcuts`, and deletes it again when
Glass comes back. The hash in
the name changes with the artwork, so the shell never serves a stale cached
icon.

The sources live under `svg/bars/` and `svg/solid/`, flat fills with no
filter, on the plate outline of the glass icon. `<name>` is `black` or
`white`:

| Source | Render | Used for |
|---|---|---|
| `paneflow-icon-bars-<name>.svg` | none shipped | The reference drawing: 28 bars on a 24 px pitch, so bars land on whole pixels at 256 and 512 px. |
| `paneflow-icon-bars-<name>-<size>.svg`, sizes 16, 24, 32, 48, 64, 96, 128 | `<name>-<size>.png` at exactly `<size>` px | Windows `WM_SETICON` bitmaps, the X11 window icon (128), and the Settings preview (64, 96 or 128 by display scale). Each is drawn for its own pixel grid, with 6 to 21 bars, because 28 bars blur into grey below 256 px. Up to 64 px a bar is one pixel of ink over one pixel of gap: thicker bars leave too few of them to draw the panes and the wave. |
| `paneflow-icon-bars-<name>-shell-<size>.svg`, sizes 32, 40, 48, 64, 96 | `<name>-shell-<size>.png` at exactly `<size>` px | The large-icon entries of the Windows icon file. The Windows 11 taskbar never shows them at their own size: it takes the large icon, 32 px at 100% display scale, and draws it at three quarters, 24 px. One-pixel bars over an empty gap turn into uneven grey stripes under that reduction and the panes disappear, so these drawings fill each gap with the ink at 42% of the bar: the reduced image keeps the silhouette of the three panes and the wave, with the bars as a texture. Wider bars on a 4 px pitch stayed sharp but left six of them, too few to draw the mark. |
| `paneflow-icon-bars-<name>-macos.svg` | `<name>-macos-512.png` at 512 px | The Dock image: 21 bars on a tile inset to 80.47%, the body the ICNS uses. |
| `paneflow-icon-solid-<name>.svg` | none shipped | The reference drawing of the solid variant: the three pane outlines and the stream of `paneflow-icon-portable.svg`, as flat fills. Each pane is painted over the plate colour first, so a pane never shows through the one in front, and the stream is painted in the plate colour. An earlier version stacked the 28 rows of the bar mark with no gap: its stepped edges looked rough next to the glass icon. |
| `paneflow-icon-solid-<name>-<size>.svg`, sizes 16, 24, 32, 40, 48, 64, 96, 128 | `<name>-solid-<size>.png` | The solid variant at every size. One drawing serves both families, so no `-shell-` file exists: solid shapes carry no fine pattern, and the taskbar reduction costs them nothing. At 16 and 24 px the stream is widened by 34 units, as the simplified plated source does, so the cut survives. |
| `paneflow-icon-solid-<name>-macos.svg` | `<name>-solid-macos-512.png` at 512 px | The Dock image of the solid variant. |

### Illustrated skins

The sixteen skins are raster illustrations, not drawings: the same mark in
another material, on the plate outline of the glass icon, with transparent
corners. Their masters are `skins/<name>-1024.png`:
`aubergine`, `paper`, `midnight-matte`, `polar`,
`melon`, `honeydew`, `berry`, `rainbow`, `solarwave`, `neon`,
`phosphor`, `holo`, `amethyst`, `blueprint`, `chip`, `nostalgia`.

| Master | Render | Used for |
|---|---|---|
| `skins/<name>-1024.png` | `<name>-<size>.png`, sizes 16, 24, 32, 40, 48, 64, 96, 128 | Every Windows bitmap and icon file entry, the X11 window icon and the Settings preview. A skin has no pixel-fitted drawing and no `-shell-` file: each size is a Lanczos reduction of the master, computed on premultiplied alpha so the transparent corners leave no fringe. |
| `skins/<name>-1024.png` | `<name>-macos-512.png` | The Dock image: the master reduced to 412 px, the 80.47% body, centered on a transparent 512 px canvas. |
| `skins/<name>-1024.png` | `<name>-linux-256.png` | The Linux desktop entry icon: the master reduced to 224 px, the 87.5% keyline of the installed hicolor icon, centered on a transparent 256 px canvas. |

### Linux desktop entry icons

A Wayland compositor draws the icon of the desktop entry that matches the
window's app id, never one the window sends, and GNOME offers no protocol for a
window icon at all. So on Linux Paneflow writes the chosen icon to
`icons/paneflow-<name>-<hash>.png` in the Paneflow home and points the
`paneflow.desktop` entry at that file. An entry installed by a package is copied
to `$XDG_DATA_HOME/applications`, which the desktop prefers, with an
`X-Paneflow-Mirror-Of` key naming its source and a `TryExec` on this Paneflow;
the copy is refreshed from the source at each launch and deleted when Glass
comes back or the source goes. The entry a `.tar.gz` install writes there gets
its `Icon` line rewritten instead. `src-app/src/app_icon/desktop_entry.rs` holds
those rules.

Every variant ships one `<name>-linux-256.png` for this, because the desktop
scales a single file to the dock, the app grid and the window switcher. It
carries the 87.5% keyline of `paneflow-icon-linux-1024.png`, so a variant reads
at the size of the glass icon next to it. The skins reduce their master; `black`,
`white`, `black-solid` and `white-solid` render their reference drawing
(`paneflow-icon-bars-<name>.svg`, `paneflow-icon-solid-<name>.svg`) at 1024 px
first. Regenerate the twenty files with
`bash scripts/build-linux-app-icons.sh`, which needs ImageMagick 7 with its
librsvg delegate.

### macOS Dock tile plug-in

`NSApplication.applicationIconImage` changes the Dock only while Paneflow
runs. To keep the chosen icon once it quits, the bundle carries
`Contents/PlugIns/PaneflowDockTile.docktileplugin`, named by the
`NSDockTilePlugIn` key of `assets/Info.plist`. The Dock loads it into its
helper process at login and when the tile is pinned, whether Paneflow runs or
not, the mechanism Ghostty ships for its own icon setting.

The plug-in is the `paneflow-dock-tile` crate: a Rust static library that
`scripts/build-dock-tile-plugin.sh` links into a Mach-O bundle with
`clang -bundle`, because a dylib is not a loadable bundle. The `bundle`
feature registers its `PaneflowDockTilePlugIn` class when the bundle loads,
since `objc2` otherwise registers a class at its first use. Its resources
are every `<name>-macos-512.png`, renamed `<name>.png`. The app writes the
chosen name under `PaneflowAppIcon` in its own preferences and posts
`<bundle id>.app-icon-changed`; the plug-in reads the preferences of the app
that contains it and draws that image, or clears its view for Glass. The
helper process runs sandboxed, so the plug-in never reads the Paneflow home.
`scripts/check-dock-tile-plugin.sh` loads the plug-in the way the Dock does
and checks both directions in the macOS CI job.

`src-app/src/assets.rs` leaves every `*-macos-*.png` out of the Windows and
Linux binaries, and every `*-linux-*.png` out of the Windows and macOS ones,
which never read them.

`glass-64.png`, `glass-96.png` and `glass-128.png` are the Settings preview of
the glass icon, downscaled from `paneflow-icon-windows-1024.png`.

Render each SVG source at the size its name states, never from a larger
render: a downscale breaks the pixel fit the size-specific drawings exist for.
Any SVG renderer works, since these sources use no filter. A test in
`src-app/src/app_icon.rs` checks that every PNG is embedded at its named size.

## Windows plating geometry

The three desktop platforms ship the same plated icon. On Windows the tile
fills the whole canvas, `WINDOWS_BODY_PCT = 10000`, because that is what its
neighbours do: the 256 px icons of Zed, VS Code and Chrome, extracted from
their executables, all span 100% of the canvas, and Zed's is a plated tile of
the same kind. The 83.33% inset of the transparent mark would make the tile
read visibly smaller than Zed's in the taskbar.

The embedded runtime icon `src-app/assets/icons/paneflow.png`, shown inside
the app and in Windows notifications, stays on the transparent portable mark.

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

The 87.5% keyline is Linux-only. `assets/PaneFlow.ico` takes the Windows master
with no inset, `assets/PaneFlow.icns` takes the macOS master at 80.47% under
the rounded mask, and the rust-embed runtime icon
`src-app/assets/icons/paneflow.png` stays on the portable master.

The repository `README.md` header points at that runtime icon. Do not repoint
it at `assets/icons/paneflow-128.png`: those files are the Linux hicolor set
and carry the keyline margin.

## CI

The release workflow (`.github/workflows/release.yml`) runs the script on every
leg before the packaging steps. If you forget to commit a regenerated icon, CI
will still produce a release using fresh outputs from the committed masters --
no stale-icon shipping. The local-commit step exists so local `cargo build`
also picks up the new icons without needing ImageMagick.
