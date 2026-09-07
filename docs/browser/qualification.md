# Browser GPU qualification at the current GPUI pin

Date: 2026-09-05. Paneflow baseline:
`fbfefd250a3f3c8d9968a23f8c358712859bc904`, branch `feat/agents-browser`, clean
before qualification work. GPUI pin:
`fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8`. Rust: 1.98.0. Locked wgpu: 29.0.4.

Decision: **NO-GO for product integration with the evidence currently
available**. This is not a measured claim that CEF cannot work. Native
prototypes and measurements are absent, and the current public GPUI surface
path cannot consume the required Linux/Windows external GPU images.

## Verified source obstacles

These observations come from the exact local Cargo checkout, read without
patching it. Paths and hashes are archived in
[preflight evidence](../../bench/browser/evidence/preflight.json).

| Execution path | Observation at the pinned source | Consequence |
|---|---|---|
| `gpui::surface` -> `SurfaceSource` -> `Window::paint_surface` | Both the constructor and painting method are macOS-only; `SurfaceSource` only carries `CVPixelBuffer` | No public Linux DMA-BUF or Windows shared-texture source can enter this path |
| GPUI Linux Wayland/X11 window -> `WgpuRenderer::draw` | `PrimitiveBatch::Surfaces` has an empty implementation | Supplying a foreign wgpu texture alone does not make it part of the GPUI scene |
| GPUI Windows window -> `DirectXRenderer::draw` -> `draw_surfaces` | The method returns successfully without drawing surfaces | A shared D3D texture is not presented through this path |
| GPUI Apple Metal renderer -> `draw_surfaces` | It asserts `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` and samples two planes | An arbitrary RGBA/BGRA browser IOSurface cannot be assumed directly compatible; conversion or renderer support still requires qualification |
| GPUI `RenderImage::new` -> image atlas | Input is CPU `image::Frame` data | A bitmap route does not satisfy the normal-path zero-full-frame-readback requirement |

Pinned source references:
[surface element](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui/src/elements/surface.rs#L9),
[window painting](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui/src/window.rs#L4591),
[wgpu surface dispatch](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui_wgpu/src/wgpu_renderer.rs#L1529),
[DirectX surface dispatch](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui_windows/src/directx_renderer.rs#L810),
[Metal surface format](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui_apple/src/metal_renderer.rs#L1118),
[CPU RenderImage input](https://github.com/zed-industries/zed/blob/fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8/crates/gpui/src/assets.rs#L43).

This proves a gap in the existing composition path. It is not a proof of
impossibility across every native compositor API. A custom platform/window
implementation would itself introduce an architectural and maintenance change.

## Architecture choices still requiring a decision

| Approach | GPU / clipping | Focus / accessibility | Sandbox / maintenance |
|---|---|---|---|
| Dedicated CEF host, accelerated OSR | Requires GPUI external-surface import/composition, explicit handle ownership, fences and bounded pools; no current Linux/Windows entry point | Native input forwarding and an OS accessibility bridge still unproven | Preserves process separation, but CEF bootstrap/helpers and GPU bridge need native validation |
| Native hosted web view where possible | Avoids GPUI texture import only if OS composition can host and clip the view correctly; native Wayland path unproven | Native view may provide a usable focus/accessibility path; must test overlays, IME and screen readers | Requires platform-specific embedding; using different browser engines would change the agreed product scope |
| CEF inside Paneflow process | Moving the host does not add the missing GPUI surface primitive | Adds CEF/AppKit main-thread coordination to the app; does not establish an OSR accessibility bridge | Changes bootstrap/lifecycle and fault isolation; still requires sandbox and package qualification |

The dedicated host remains a hypothesis, not an approved working adapter.
Before continuing that path, plan an explicit GPUI change covering external
image source types, scene ownership, Linux wgpu import/composition, Windows
D3D composition, Metal browser-compatible formats, producer/consumer
synchronization, device loss and compositor presentation timestamps. Keep the
four GPUI revision values synchronized and preserve `font-kit`. No upstream
patch, fork, alternate pin or renderer rewrite is included here.

This identifies at least three platform renderers plus the common scene/window
boundary. Patch size, maintenance cost and latency are **not measured**;
estimating them credibly needs a separately scoped integration investigation.
Do not assign an invented line count or assume the existing cef-rs wgpu import
helpers can reach GPUI's private renderer resources.

## Native qualification state

| Target / condition | State | Evidence still required |
|---|---|---|
| Linux x86_64, Fedora 44, NVIDIA 4070 Ti SUPER, driver 610.57.04 | Inventory only, no browser execution | Separate CEF host, native Wayland with no X11 connection, X11 run, DMA-BUF format/modifier/FD lifetime/fences, sandbox, M1, resize/scale/host loss |
| Linux x86_64, Intel/AMD Mesa and additional compositors | Not executed | Real GPU/compositor access and the same runtime evidence |
| Linux aarch64 | Not executed | Real ARM GPU, sandbox, creation/input/presentation |
| macOS aarch64 | Not executed | Signed sandboxed bundle, AppKit main-thread bootstrap, owned IOSurface, Retina/IME, M1 and VoiceOver |
| Windows x86_64 | Not executed | Bootstrap/client DLL at the chosen pin, protected installation, standard-user DLL loading, GPU sharing, Windows 10/11, DPI/IME, M1 and Narrator |

The local Wayland session also has `DISPLAY=:0`. This inventory does not prove
that a future CEF page avoids XWayland. No CEF process was launched. No current
CEF/Chromium/cef-rs version, binary hash, minimum OS/glibc, license inventory or
artifact size is certified. There is no runtime pin in the application or
lockfile; inventing a qualification manifest without tested binaries would
misstate compatibility.

## Delivered and remaining evidence

The [M1 protocol and fixtures](../../bench/browser/README.md) are reachable
through `bun scripts/browser-qualification.mjs`. HTTP tests exercise this CLI
and every fixture route. Capture tests exercise `inspect` and `compare`,
rejecting invalid timestamps, uncontrolled loads, changed pairing, missing or
tampered raw artifacts and CPU metrics mislabeled as presentation. These tests
do not open a browser or replace the native graphical qualification.

The protocol defines the controls, loads, pixel resolution, frequency
conditions, percentile calculation and missing baselines. Driver versions are
only known for the local inventory; no comparative experiment has started.
The Paneflow A and minimal CEF B raw baseline archives were captured locally
afterwards (see the M1 inventory). The GPUI boundary is now addressed by the
[external surfaces ADR](gpui-external-surfaces.md) and the presentation mode
in the [runtime contract](runtime.md); their local prototype evidence lives
under `bench/browser/evidence/prototype/`. The original runtime failed to
produce sandboxed accelerated paints on the local NVIDIA driver. The later
[source-built CEF experiment](gpu-sandbox-tsync.md) passes the functional
lifecycle and shader workload on AMD native Wayland and NVIDIA XWayland.
NVIDIA native Wayland, native Xorg and physical M1 remain open; the normal
runtime remains non-qualified.

R0 is not GO: the bounded sandbox/functionality evidence does not satisfy all
performance, accessibility and compatibility gates. Neither an HTTP fixture
test nor a source investigation grants GO. Shipping qualification still needs
the missing native evidence and the intended hardened runtime artifact.

## Validation of this delivery

`bun test scripts/browser-qualification/qualification.test.mjs` passed after
the final source change: 27 tests, 140 assertions, no failures, on Bun 1.3.14
under Linux. The suite starts the actual CLI server and invokes both capture
commands as subprocesses. It closes its server and removes its synthetic data.

No Rust source, Cargo manifest, lockfile, shipped helper or platform adapter
changed. Cargo compilation, lint, format and supply-chain gates are not
applicable to this fixture/tooling-only diff. Native graphics, sandbox,
performance, signing and non-Linux execution remain unverified.
