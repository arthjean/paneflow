# Early GPU broker and Chromium TSYNC experiment

This directory contains a desktop Linux source experiment pinned to Chromium
151.0.7922.174 and CEF 2384915b7b1f0fe5ad1107e48d80c34e86b698d7.
`experiment.json` records the complete source pins, GN arguments and patch hash.
It is not the production browser manifest or a qualified release runtime.

## Source build

Use CEF's pinned `tools/automate/automate-git.py` with branch 7922 and the CEF
commit above to obtain the exact Chromium dependency tree. Complete the normal
CEF hooks and apply the three numbered patches, in order, to Chromium's
`src/`. They cover early broker/TSYNC, offscreen Wayland scaling and early X11
GBM initialization. Keep `dcheck_always_on=true` to exercise the broker invariant.
The recorded experimental build disables LTO, CFI and symbols to fit the local
build machine; these artifacts do not establish production hardening or M1
performance.

Build `cefsimple chrome_sandbox` with the recorded GN configuration. The
standalone `policy-probe/` has separate instructions and exercises the real
Chromium policy before the full CEF build is available. The successful local
AMD and NVIDIA runs are recorded in the evidence path in `experiment.json`.

## Runtime staging

Create an isolated copy of the current PaneFlow source tree, including the
current local browser work, and record file hashes and the source HEAD. Make
its pinned GPUI and libghostty prerequisites available. Do not replace the
normal checkout's browser manifest or verified runtime.

After CEF finishes building, run the staging helper with fresh output paths:

```sh
python3 native/browser/experiments/tsync/stage-cef.py \
  --build-output /absolute/chromium/src/out/Release_GN_x64 \
  --workspace /absolute/paneflow-source-copy \
  --output /absolute/experimental-cef-bundle
```

The helper packages the exact runtime and resources, generates a manifest with
all file and archive hashes, and invokes PaneFlow's normal explicit fetcher
against the local archive. The fetcher checks extraction limits, hashes and
ELF GLIBC requirements. Only after verification succeeds does the helper copy
the candidate manifest into the isolated source tree. Its `stage.json` gives
the verified runtime path.

Build the isolated app and host with `PANEFLOW_CEF_ROOT` set to that verified
runtime, using `cargo build --locked -p paneflow-app -p paneflow-browser-host
--features paneflow-browser-host/cef-runtime`. Then run the existing prototype
qualification script from that isolated source tree with its resulting
binaries. Keep separate evidence directories for each GPU and display backend.
The source snapshot receipt accompanies the evidence because a directory copy
has no independent Git commit. When reusing a copied Cargo target cache, also
copy the matching `src-app/target/embed/bin` helper artifacts or regenerate
them: cached build-script output does not recreate this source-relative folder.

The X11 host must add `enable-native-gpu-memory-buffers` before Chromium
serializes GPU preferences. The current host does this only in X11 capture
mode. CEF's shared-texture setting does not enable that preference itself.

`run-wayland.py` creates an isolated headless Mutter on the explicitly selected
card/render node and runs input, resize, scale and host-loss checks. Supply
the isolated workspace, app/host binary paths, render node, card, EGL vendor,
GPUI device ID and a fresh output directory. These paths are machine-specific.

`run-shader-churn.py` accepts the same arguments and additionally `--display
x11` for an existing X server. It temporarily replaces only the isolated
workspace's WebGL fixture, verifies 120 distinct shader programs and pixel
checks with fresh native caches, checks sandbox/thread snapshots and observed
process exit, then restores the fixture. X11 results retain the session type:
a client of XWayland is not a native Xorg qualification.

## Recorded result

`bench/browser/evidence/prototype/cef-tsync-integrated-20260906/summary.json`
indexes the tested binaries, manifests, source snapshots and native receipts.
AMD headless Wayland and NVIDIA XWayland pass the complete functional lifecycle
and shader runs with the strict sandbox. NVIDIA native Wayland remains blocked
on Skia surface creation. Native Xorg, M1 and shipping-hardening qualification
remain open. Initial failing attempts are retained beside the passing runs.

Required runtime evidence includes strict GPU sandbox startup, every observed
GPU thread's seccomp state, renderer isolation, broker lifecycle, DMA-BUF/GPUI
frames, input, resize, scale-only changes, host loss and process shutdown.
Native Wayland and native X11 remain separate gates. A headless functional
capture does not measure physical presentation or satisfy M1 budgets.
