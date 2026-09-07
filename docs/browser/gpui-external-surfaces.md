# ADR: GPUI external surfaces for the Browser dock

Status: accepted for R0 (2026-09-06). Owner: US-006 of the Linux browser
plan. Consumers: the Linux DMA-BUF adapter (US-007), the Wayland and X11
prototypes (US-008, US-009), later the Windows and macOS adapters.

## Context

GPUI at the pinned revision `fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8`
cannot compose a GPU image that another process produced. The
[qualification investigation](qualification.md) recorded the obstacles at
that pin, all still true before this extension:

| Path at the pin | File | Obstacle |
|---|---|---|
| `SurfaceSource` and `surface()` | `crates/gpui/src/elements/surface.rs` | The only variant is a macOS `CVPixelBuffer`; the constructor is `#[cfg(target_os = "macos")]` |
| `Window::paint_surface` | `crates/gpui/src/window.rs` | macOS-only; `PaintSurface` carries a `CVPixelBuffer` field on macOS and no payload elsewhere |
| `WgpuRenderer::record_frame` | `crates/gpui_wgpu/src/wgpu_renderer.rs` | `PrimitiveBatch::Surfaces` is an empty arm; the existing `surfaces` pipeline samples a two-plane YCbCr layout and is dead code |
| `WgpuContext::create_device` | `crates/gpui_wgpu/src/wgpu_context.rs` | `request_device` lets wgpu-hal enable `VK_KHR_external_memory_fd` and `VK_EXT_external_memory_dma_buf` when present, but never `VK_EXT_image_drm_format_modifier` or `VK_KHR_external_semaphore_fd`, so a DMA-BUF with a vendor modifier cannot be imported into the window's device |
| `PlatformWindow` | `crates/gpui/src/platform.rs` | Nothing exposes the window's wgpu device, queue or adapter; `gpu_specs` only reports names |
| `DirectXRenderer::draw_surfaces` | `crates/gpui_windows/src/directx_renderer.rs` | Returns without drawing; untouched by this ADR |
| `MetalRenderer::draw_surfaces` | `crates/gpui_apple/src/metal_renderer.rs` | Asserts a biplanar YCbCr pixel format; untouched by this ADR |

Paneflow pins GPUI through four `rev` values in `src-app/Cargo.toml`
(`gpui` twice, `gpui_platform` twice) and keeps `font-kit`, `wayland` and
`x11` features. The Cargo cache is read-only and a whole-shell change is out
of scope.

## Decision

Extend GPUI with a versioned, reviewable patch series applied to a sparse
upstream checkout, and route every Zed crate the pin pulls through that
checkout with one `[patch."https://github.com/zed-industries/zed"]` table.

### Delivery mechanism

- `native/gpui/manifest.toml` pins the upstream repository and commit, the
  sparse paths the closure needs (23 crates plus the two font directories
  `gpui` embeds) and the patch series with SHA-256 digests.
- `scripts/fetch-gpui.py` creates `native/gpui/checkout` with a shallow,
  blob-filtered sparse checkout of exactly that commit, applies the series
  with `git apply --check` then `git apply`, rewrites the upstream workspace
  `members` to the crates present, and writes a stamp that later runs and
  `--verify-only` compare against the manifest. `--pristine` produces the
  unpatched tree for authoring; `--pin-patches` rewrites the digests after
  review. The checkout is not tracked by git.
- The root `Cargo.toml` excludes the checkout from the Paneflow workspace and
  patches all 23 Zed packages to their checkout paths, so the git and path
  sources never coexist and no type is duplicated. The four `rev` values stay
  the upstream pin and must equal `upstream_sha` in the manifest; the
  lockfile records the path sources and remains `--locked` clean once the
  checkout exists. CI runs the fetch before any cargo invocation through
  `.github/actions/fetch-gpui`, next to the libghostty fetch.
- Bumping the pin means changing `upstream_sha` and the four `rev` values,
  running `--pristine`, re-applying the series, resolving conflicts, then
  `--pin-patches`. A fork would replace only the delivery mechanism; the
  patch content is the contract.

### Extension surface

The series adds one scene primitive payload, one paint entry, one window
accessor and one renderer path. Nothing else in GPUI changes.

| Concern | Addition | Where |
|---|---|---|
| Surface type | `gpui::ExternalSurface { id, generation, size, handle: Arc<dyn Any + Send + Sync> }`, `SurfaceSource::External`, `surface()` on every target | `crates/gpui/src/scene.rs`, `crates/gpui/src/elements/surface.rs` |
| Ownership | The handle is reference counted; the scene stores a clone per painted frame and the producer keeps its own. Dropping the last clone releases the renderer resource on the renderer's own schedule (wgpu defers destruction until the GPU is done). The generation is producer state the renderer never interprets; adapters must not paint a surface older than the generation they were told is current | `ExternalSurface::generation`, adapter code |
| Scene dispatch | `Window::paint_external_surface(bounds, external)` inserts a `PaintSurface` on non-macOS targets; batching, clipping and z-order reuse the existing `Primitive::Surface` path | `crates/gpui/src/window.rs`, unchanged `scene.rs` batching |
| Import point | `PlatformWindow::external_surface_context()` (default `None`) and `Window::external_surface_context()` return the renderer objects an adapter must create its resources with; Wayland and X11 windows return `gpui_wgpu::ExternalSurfaceContext { instance, adapter, device, queue }` | `crates/gpui/src/platform.rs`, `crates/gpui_linux/src/linux/{wayland,x11}/window.rs` |
| Presentation point | `WgpuRenderer::record_frame` composes every `PaintSurface` whose handle downcasts to `gpui_wgpu::ExternalWgpuSurface` with a dedicated `external_surfaces` pipeline: one uniform record per surface (bounds and content mask), the surface texture view and the atlas sampler, premultiplied blending, at most 64 surfaces per frame. Unknown handles draw nothing | `crates/gpui_wgpu/src/wgpu_renderer.rs`, `crates/gpui_wgpu/src/shaders.wgsl` |
| Device creation | On Linux and FreeBSD Vulkan adapters, `WgpuContext::create_device` opens the device through `wgpu-hal` `open_with_callback` and adds, when the physical device supports them, `VK_KHR_external_memory_fd`, `VK_EXT_external_memory_dma_buf`, `VK_EXT_image_drm_format_modifier`, `VK_KHR_image_format_list`, `VK_KHR_bind_memory2`, `VK_KHR_sampler_ycbcr_conversion`, `VK_KHR_external_semaphore_fd` and `VK_EXT_queue_family_foreign`. Other backends and targets keep `request_device` | `crates/gpui_wgpu/src/wgpu_context.rs` |
| Re-export | `gpui_linux` and `gpui_platform` re-export `gpui_wgpu` on Linux and FreeBSD so Paneflow reaches `wgpu`, `ExternalSurfaceContext` and `ExternalWgpuSurface` without a fifth pin | `crates/gpui_linux/src/gpui_linux.rs`, `crates/gpui_platform/src/gpui_platform.rs` |

### Explicit unavailability

- macOS keeps `SurfaceSource::Surface(CVPixelBuffer)`; `External` does not
  exist there and `external_surface_context()` returns `None`.
- Windows compiles `External` and `paint_external_surface` but
  `DirectXRenderer` still ignores surfaces and `external_surface_context()`
  returns `None`, so an adapter reports `unavailable` before creating a page.
- Headless Linux and the web target return `None` for the same reason.
- A GPU device recovery replaces the `ExternalSurfaceContext`; resources
  created for the previous device must be dropped and re-imported. Adapters
  compare the `device` `Arc` identity to detect this.

Paneflow maps `None` to `BrowserError::Unavailable` and keeps external URL
opening and terminals working (`FR-09`, C2).

### Contract checks

The Paneflow side, not GPUI, proves the contract from a real entry:

- `paneflow browser-prototype --source pattern` drives the scene path with
  frames produced on the window's own device, without CEF, through the same
  `FrameConsumer` the DMA-BUF path uses (`src-app/src/browser/linux.rs`,
  `PatternProducer`).
- `src-app/src/browser/presentation.rs` tests cover generation change
  (`a_newer_generation_retires_the_older_one_and_refuses_its_frames`),
  resource removal
  (`a_retired_pool_or_a_lost_host_clears_the_presented_surface`) and
  UI-thread nonblocking
  (`frame_intake_never_imports_or_waits_and_releases_are_deferred_to_the_caller`):
  intake is bookkeeping only, and releases are handed to the caller, which
  schedules them from `Render` with `Window::defer`, after the old scene has
  been replaced. Only the retired frames enter that batch. The deferred
  callback registers `Queue::on_submitted_work_done`; a background worker
  uses nonblocking `Device::poll(PollType::Poll)` and waits outside wgpu's
  fence lock. A successful poll alone cannot authorize a release: the GPU
  callback is mandatory. This also covers input-driven draws that replace
  the scene before the next presentation, since future submissions use the
  replacement scene and the callback covers prior submissions. Scheduling
  from host intake would not establish that scene boundary.
- `scripts/fetch-gpui.py --verify-only` proves the checkout matches the
  reviewed series before any cargo gate runs.

### External ownership correction awaiting native qualification

The initial prototype allowed wgpu's first UNDEFINED transition after producer
content arrived and lacked explicit consumer queue-family transfers. That path
cannot certify content preservation, regardless of observed driver behavior.
The current correction initializes neutral GPU content before import and uses
contract-v2 PoolReady to finish wgpu's initial tracking transition before useful
producer writes. Separate raw Vulkan encoders acquire and release the FOREIGN
queue family around each consumer lifetime. Pool initialization and release
callbacks retain the textures until GPU completion.

The host checks the effective CEF GaneshGL/EGLANGLE/Vulkan backend through the
compiled CEF bridge before acquiring source images in GENERAL layout. It returns
them to GENERAL/FOREIGN before the CEF callback ends. A GPU process replacement
invalidates the host's contract. These changes still require native first-frame,
resize, restart and performance qualification; earlier captures do not validate
this corrected path.

- The consumer imports single-plane pools only and binds one dedicated
  allocation per image. Explicit modifiers are used when
  `VK_EXT_image_drm_format_modifier` was enabled on the window device;
  otherwise only the linear modifier is accepted and the driver's linear
  layout must match the host's stride and offset.

## Delta and maintenance

Measured on the committed series (`git diff --stat` of the checkout after
`scripts/fetch-gpui.py`):

| Patch | Files | Insertions | Deletions |
|---|---|---|---|
| `0001-gpui-external-surface-source.patch` | 4 | 135 | 5 |
| `0002-gpui-wgpu-external-surface-composition.patch` | 5 | 311 | 15 |
| `0003-gpui-linux-external-surface-context.patch` | 3 | 9 | 0 |
| `0004-gpui-platform-external-surface-reexport.patch` | 1 | 2 | 0 |

The series touches no rendering path other than the surface batch, no text
system, no event loop and no public type Paneflow already used. Upstream
changes to `PaintSurface`, `PrimitiveBatch::Surfaces`, `WgpuContext::create_device`
or the `PlatformWindow` trait are the conflict points to expect at a pin bump.
The `perf`, `zlog` and `ztracing` crates are checked out only because the
lockfile already carried them from the git pin; they are unpatched.

## Rejected alternatives

- Bitmap route through `RenderImage` and the sprite atlas: a full-frame CPU
  readback per frame, forbidden by C2 and NFR-04.
- Wayland subsurface or X11 child window owned by CEF: no GPUI clipping,
  overlays lose priority (SEC-05), no X11 alpha guarantee, and a second
  presentation path per compositor family.
- Patching the Cargo git cache in place: not versioned, not reviewable,
  forbidden by the plan.
- Vendoring the four crates with rewritten manifests: every `workspace = true`
  dependency would need hand-written replacements and drift from the pin.

## Escalation conditions

Per US-006 the following would require a plan revision before any scope
expansion: a renderer change that forces terminal or text pipeline changes, a
dependency on a private or non-distributable API, or a change of application
shell. None applies to this series; the Windows DirectX and macOS Metal
composition paths remain open work owned by their platform plans.
