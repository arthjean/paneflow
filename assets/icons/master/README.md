# Icon master files

Source PNGs for `scripts/build-icons.sh`. Drop your masters here, run the script,
commit the regenerated outputs under `assets/icons/`, `assets/PaneFlow.icns`,
`assets/PaneFlow.ico`, `packaging/wix/paneflow.ico`, and
`src-app/assets/icons/paneflow.png`.

| File | Required | Used for |
|---|---|---|
| `paneflow-icon-1024.png` | yes | Transparent portable mark for Linux hicolor, Windows ICO, and the GPUI runtime icon. |
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

## CI

The release workflow (`.github/workflows/release.yml`) runs the script on every
leg before the packaging steps. If you forget to commit a regenerated icon, CI
will still produce a release using fresh outputs from the committed masters --
no stale-icon shipping. The local-commit step exists so local `cargo build`
also picks up the new icons without needing ImageMagick.
