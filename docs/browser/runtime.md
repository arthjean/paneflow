# CEF Linux candidate and witness

The terminal application does not depend on the browser crates. The separate
`paneflow-browser-host` binary defaults to an unavailable diagnostic; Linux
qualification explicitly enables `cef-runtime`. Other targets keep that
diagnostic and require no CEF archive. No Browser capability is publicly
qualified by this preparation.

## Candidate provenance

[manifest.toml](../../native/browser/manifest.toml) pins CEF
`151.3.24+g2384915+chromium-151.0.7922.174`, Chromium `151.0.7922.174`, CEF
commit `2384915b7b1f0fe5ad1107e48d80c34e86b698d7` and cef-rs commit
`71a3eba55bb46957b7bdd627ef7caf5c70acf4bb`. Its `contract_version = 3` matches
the common Rust wire protocol. Linux x86_64 and aarch64 are development
candidates; Windows and macOS entries remain absent.

Protocol 3 adds document editing, pointer-capture cancellation and asynchronous
context-menu replies. Rebuild both the application and `paneflow-browser-host`
together; a protocol 2 host is rejected at the handshake. Revalidate an existing
archive with `python3 scripts/fetch-browser.py --target x86_64-unknown-linux-gnu
--verify-only` after updating the manifest. This updates the verification stamp
without downloading or rebuilding CEF.

The dock consumes forwarded key events so GPUI cannot also insert their text
through its input-handler fallback. Native IME commits keep their separate
path. Document focus follows GPUI focus, visibility and window activation;
pointer capture delivers movement and button release outside the viewport and
is cancelled when the document loses focus.

Clipboard integration currently exchanges plain text, up to 32 KiB. Copy reads
the focused renderer's current DOM selection through a CEF process message and
DOM visitor, without page JavaScript or console-message transport. GPUI owns
the system clipboard. Cut waits for the GPUI write acknowledgement and compares
the selection again before deleting it. Paste checks the page generation and
focus after the asynchronous system read. Navigation, focus loss and closing
invalidate pending work. Password selections are excluded. Rich clipboard
formats and page-defined clipboard transformations are not implemented by this
text bridge.

Context menus use the CEF root command model, rendered by GPUI with keyboard
navigation and viewport-independent placement. The host retains the callback
and validates the selected command; dismissal, navigation and focus loss cancel
it. Submenus are currently omitted. Clipboard menu commands use the same native
selection and system-clipboard path as keyboard shortcuts.

Both minimal archives were downloaded from the [CEF distribution index](https://cef-builds.spotifycdn.com/index.json),
checked against its size and SHA-1, then hashed locally with SHA-256. Every
distributed runtime/resource file, both notices and the implicated headers have
individual SHA-256 pins. The generated Rust bindings are pinned through cef-rs
and Cargo.lock. CEF uses BSD-3-Clause; cef-rs uses Apache-2.0 OR MIT. The actual
archives contain `LICENSE.txt` and Chromium's `CREDITS.html`; both are retained
and verified. This is a development candidate, not a completed release SBOM or
codec redistribution verdict.

The archive inventories are 1,556,438,625 uncompressed bytes x86_64 and
2,639,771,835 bytes aarch64. These include upstream unstripped libraries. They
do not meet the 600 MiB installed release budget as-is. Packaging/stripping and
all compressed/installed deltas remain a later release qualification, with no
change to that budget.

## Explicit fetch and ABI inspection

```sh
python3 scripts/fetch-browser.py --target x86_64-unknown-linux-gnu
python3 scripts/fetch-browser.py --target aarch64-unknown-linux-gnu
python3 scripts/fetch-browser.py --target x86_64-unknown-linux-gnu --verify-only
```

Requires Python 3.11+ with tarfile's data filter and GNU readelf. `--archive`
accepts an already downloaded archive and still verifies its size and digest.
The command checks the archive before extraction, rejects traversal, absolute
paths, links, special entries, duplicates and sizes beyond the pinned budgets.
Extraction happens in a temporary sibling and installation uses a new
content-addressed directory. A mismatch never replaces an operational runtime.
There is no fetch in a Paneflow build script.

The archived [x86_64 ELF audit](../../bench/browser/evidence/linux64-elf.json)
and [aarch64 ELF audit](../../bench/browser/evidence/linuxarm64-elf.json) list
SONAME dependencies and version requirements for all six shipped ELF objects.
Maximum GLIBC requirement is 2.25 for libcef on both targets, below Ubuntu
22.04's [glibc 2.35](https://packages.ubuntu.com/jammy/libc6). No GLIBCXX or
CXXABI dependency was found. All 32 libcef NEEDED entries have a package in
the official Jammy amd64 and arm64 file lists, including the dynamic loaders.

Required packages: libc6, libgcc-s1, libglib2.0-0, libnspr4, libnss3, libatk1.0-0,
libatk-bridge2.0-0, libdbus-1-3, libcups2, libx11-6, libxcomposite1, libxdamage1,
libxext6, libxfixes3, libxrandr2, libgbm1, libexpat1, libxcb1, libxkbcommon0,
libcairo2, libpango-1.0-0, libudev1, libasound2 and libatspi2.0-0. Sources follow
`https://packages.ubuntu.com/jammy/{amd64,arm64}/{package}/filelist`, for example
[NSS amd64](https://packages.ubuntu.com/jammy/amd64/libnss3/filelist) and
[libc arm64](https://packages.ubuntu.com/jammy/arm64/libc6/filelist).
This confronts the candidate with the minimum; it does not prove every imported
symbol against a Jammy sysroot, installation or GPU execution there. The
minimum remains Ubuntu 22.04. Native ARM and distribution qualification remain
open.

## Native entry point and lifecycle

Build with `PANEFLOW_CEF_ROOT` pointing to the verified content-addressed
directory printed by fetch:

```sh
PANEFLOW_CEF_ROOT=/absolute/verified/runtime cargo build -p paneflow-browser-host --release --features cef-runtime --locked
bun scripts/browser-qualification.mjs witness target/release/paneflow-browser-host target/browser-qualification/new-run empty 2
```

The cef-rs `dox` feature intentionally disables upstream build-time downloading,
copying and linking. Its generated bindings remain real bindings. The local
host build script only links the explicitly supplied runtime. This behavior
was inspected at the exact [sys/build.rs pin](https://github.com/tauri-apps/cef-rs/blob/71a3eba55bb46957b7bdd627ef7caf5c70acf4bb/sys/build.rs).
The supervisor verifies runtime hashes again before loading any executable,
stages a separate witness executable and runtime links, and creates a 0700
profile outside all personal browser profiles. The fixture HTTP server is
shared with the existing qualification CLI.

The control transport is inherited stdin/stdout pipes, length-prefixed JSON
with the common 256 KiB boundary, bounded serialization and four queued requests.
Page console diagnostics are capped at 1,024 UTF-16 units before conversion
and decoded into a fixed fixture-state schema; malformed reports reject the run. There is no TCP
control or CDP port, page-callable control API, arbitrary evaluate or attachment to an
existing browser. The first request negotiates the contract before initialize.
The harness scope is assigned by the parent, not by page content. Native
callbacks are recorded separately from deterministic protocol state.

The host follows pinned [cef_app.h](https://github.com/chromiumembedded/cef/blob/2384915b7b1f0fe5ad1107e48d80c34e86b698d7/include/cef_app.h)
and [cef_browser.h](https://github.com/chromiumembedded/cef/blob/2384915b7b1f0fe5ad1107e48d80c34e86b698d7/include/cef_browser.h):
API hash, ExecuteProcess for children, Initialize on the main thread,
RunMessageLoop, asynchronous CloseBrowser, OnBeforeClose, then Shutdown.
`no_sandbox` is zero; disabling switches are rejected before ExecuteProcess,
including single-hyphen, case and `=value` variants. Linux sandbox proof
requires actual renderer/GPU seccomp and no_new_privs, plus renderer namespace
isolation from /proc. GPU namespace policy is platform/driver dependent; a
namespace difference is not assumed for that process.
Without a frame channel the host creates a CEF Views BrowserView and a
top-level frameless window, both with the ALLOY runtime style, and its Ozone
backend is Wayland. The
window holds the BrowserView until asynchronous close completes. Creation
occurs outside the controller's RefCell borrow because CEF can re-enter its
callbacks synchronously. No GPUI dock or external-texture adapter is involved.

The supervisor starts Chromium `viz,cc` tracing before page creation. Trace
`process_name` metadata identifies Renderer and GPU Process PIDs, which are
joined to the live `/proc` snapshot before shutdown. Forked zygote command lines
alone are insufficient role evidence. The isolated profile uses the basic
password store and disables extensions, sync and background networking.

`native.json` records fixture reports, lifecycle and process topology.
`stderr.txt` retains the native Wayland protocol, and
`profile/chromium-trace.json` retains Chromium's trace. Trace flushing finishes
before CloseBrowser. Successful runs require OnBeforeClose, Shutdown and exit;
failed runs keep their diagnostics and terminate only the witness process group.

## Presentation mode and the application supervisor

The same host binary runs in presentation mode when `PANEFLOW_BROWSER_FRAME_FD`
names the frame channel. It then initializes CEF with
`windowless_rendering_enabled`, creates the browser with
`browser_host_create_browser` (windowless, `shared_texture_enabled`, ALLOY
runtime style, 60 frames per second) instead of a Views window, and installs a
render handler. `OnAcceleratedPaint` imports the CEF native pixmap into a
private Vulkan device (`VK_KHR_external_memory_fd`,
`VK_EXT_external_memory_dma_buf`, `VK_EXT_image_drm_format_modifier`), blits
the visible rectangle into the free slot of an exported DMA-BUF pool, waits for
its fence and only then returns to CEF, as the pinned
[cef_render_handler.h](https://github.com/chromiumembedded/cef/blob/2384915b7b1f0fe5ad1107e48d80c34e86b698d7/include/cef_render_handler.h)
requires for callback-scoped handles. The pool prefers the linear DRM modifier
and otherwise the first single-plane modifier the device can export. A software
`OnPaint` is refused with `unsupported_format`; three consecutive failures
disable presentation and are reported, never converted to a bitmap refresh.
The Vulkan device is selected by `PANEFLOW_BROWSER_GPU=vendor:device` (the
window adapter's identifiers); no match is `wrong_device`. `PANEFLOW_BROWSER_OZONE`
selects `wayland` (default) or `x11`, and `PANEFLOW_BROWSER_OWNER` carries the
trusted `workspace/session` scope. In presentation mode the origin may be a
loopback `http://127.0.0.1:port` or a plain `https://host`.

The application side lives in `src-app/src/browser/`. `HostSupervisor` is the
controller entry the application calls: it runs `Inactive`, `Starting`,
`Ready`, `Stopping` and `Failed` on a shared state machine, performs every
verification, staging and launch on its own threads and never on the GPUI
thread. Before spawning it checks that `verified-manifest.sha256` equals the
digest of the manifest compiled into the application (`fetch-browser.py
--verify-only` re-verifies every file and refreshes that stamp after a
manifest edit), that `libcef.so`,
`icudtl.dat` and `en-US.pak` exist and that `chrome-sandbox`,
`v8_context_snapshot.bin` and `icudtl.dat` match their manifest digests. The
runtime is hard-linked (symlinked across filesystems) with the host binary into
`<data dir>/browser/host-<digest>/bin`, the profile is a 0700 directory under
`<data dir>/browser/profiles/`, and the host runs in its own process group with
`DISPLAY` removed for Wayland. The handshake requires `presentation_ready`, the
`initialized` event with the host's own PID and a `capabilities` reply at
contract version 1 with a non-absent availability; anything else fails once.
A dead or refused host leaves the state `Failed` with its reason; there is no
retry until the caller invokes `retry`, no kill by process name, and shutdown
sends EOF, waits five seconds, then signals the process group until every
descendant is gone. Before activation the application maps no `libcef` and
runs no CEF process; targets without a Linux adapter answer `unavailable`.

`paneflow browser-prototype` is the executable entry that drives this path:

```sh
paneflow browser-prototype --url http://127.0.0.1:PORT/empty --scenario input,resize,scale,host-loss --log /absolute/prototype.jsonl
paneflow browser-prototype --source pattern --scenario resize,scale
bun scripts/browser-qualification/prototype.mjs --output /absolute/new-dir --display wayland
```

`--source pattern` produces frames on the window device through the same
consumer, ledger and release path without CEF. The runner serves the fixture,
launches the prototype with `PANEFLOW_CEF_ROOT` and `PANEFLOW_BROWSER_HOST`,
snapshots the host process tree (seccomp, no_new_privs, namespaces, `ss -xp`
X11 sockets, `DISPLAY` in the environment) and writes `native.json` with a
per-criterion verdict. Processes are classified by thread names
(`VizCompositorTh` marks the GPU process, `Compositor` a renderer) because
zygote-forked children keep the zygote command line; the snapshot is taken
after the first presented frame because CEF paints on demand and a static
fixture yields a single frame. The host's stderr, which also carries the GPU
process log, is archived as `host-stderr.txt`.

### GPU process switches and sandbox enforcement

In presentation mode the host adds `--render-node-override=<node>` when
`PANEFLOW_BROWSER_RENDER_NODE` names the render node of the window device
(the application discovers it through `VK_EXT_physical_device_drm`) and, on
X11 only, `--use-angle=vulkan` with
`--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan`, the only
configuration that produced accelerated paints on the local machine. X11
capture also enables `--enable-native-gpu-memory-buffers` so the experimental
runtime can initialize GBM before sandbox installation. Chromium
refuses the Skia Vulkan backend under `--ozone-platform=wayland`, so Wayland
keeps the default ANGLE GL path. Every presentation-mode host also passes
`--gpu-sandbox-failures-fatal=yes`: Chromium otherwise skips seccomp-bpf
silently when the GPU process is already multi-threaded at sandbox
initialization, and a GPU process without seccomp would present frames while
violating the sandbox requirement. With the switch that GPU process exits,
CEF falls back to software painting, and the host refuses the software path
with `unsupported_format`, so a driver that cannot keep the sandbox makes the
Browser unavailable instead of unsandboxed. `PANEFLOW_BROWSER_LOG_VERBOSE=1`
raises CEF logging to verbose for that diagnosis.

Frames whose visible size differs from the presented geometry (the 1x1 frame
CEF paints before the first `Present`, or a frame at the old scale) are
dropped, reported once as the native event `frame_dropped` with the expected
size, and answered with a delayed `Invalidate` so CEF repaints at the current
size; the refresh stops after 120 attempts. A native pixmap descriptor that is
not a DMA-BUF (`/proc/self/fd` shows a memfd when Chromium renders with
SwiftShader) is refused as `invalid_handle` before any import.

### Local outcome on NVIDIA 610.57.04 (Fedora 44, GNOME 50.4)

| Session | Result | Cause |
|---|---|---|
| Wayland native | FAILED, no accelerated paint | The GPU process cannot create a Skia surface on the GBM pixmaps used by the shared-texture capturer (`Unable to initialize SkSurface`); Skia Vulkan is refused on Ozone Wayland |
| X11 (XWayland client), Vulkan | FAILED, sandbox enforced | The NVIDIA Vulkan driver leaves the GPU process multi-threaded, seccomp cannot start, the fatal switch ends the GPU process and the software fallback is refused |
| X11 (XWayland client), default GL | FAILED, no paint | The sandboxed GPU process cannot load Mesa's GBM backend from `/usr/lib64/gbm/`, so no native pixmap exists |
| Native Xorg session | NOT_EXECUTED | No X11 session on the local machine |

A diagnostic archive taken with the sandbox not enforced
(`bench/browser/evidence/prototype/x11-20260906T110908Z`) shows the full
chain working on this GPU: page presented, keyboard and click observed by the
fixture, resize and scale through new pool generations, host loss and retry,
about 1 ms per host blit-and-fence and under 1 ms from fence to application
intake. Its `sandbox` verdict is FAILED because the GPU process ran without
seccomp, so it proves the transfer path and nothing about qualification. The
`--source pattern` run proves the GPUI composition and release path on the
window device without CEF. Both display servers therefore stay non-qualified
on this driver.

### Local outcome on Mesa RADV (Raphael iGPU, Mesa 26.1.8, headless mutter 50.4)

The same machine carries an AMD Raphael iGPU with no display attached. A
headless `mutter --headless --virtual-monitor 1280x800` started inside a
`bwrap` mount namespace that exposes only `/dev/dri/renderD128` and
`/dev/dri/card1` makes the Mesa device the compositor's primary GPU without
recabling a monitor; GPUI then selects the RADV adapter on its own.

| Run | Result | Cause |
|---|---|---|
| `--source pattern` (`mesa-headless-pattern-20260906T120304Z`) | COMPLETED | GPUI composition, resize and scale through four pool generations on RADV under native Wayland |
| CEF, default sandbox order (`mesa-headless-wayland-20260906T120043Z`) | FAILED, software paint refused | The GPU process is multi-threaded when seccomp starts (`InitializeSandbox() called with multiple threads`), the fatal switch ends it three times, CEF falls back to `OnPaint` |
| CEF, `--gpu-sandbox-start-early` (`mesa-headless-wayland-early-sandbox-20260906T120259Z`) | FAILED, software paint refused | The sandbox starts before GL initialization and ANGLE can no longer `dlopen` the native `libEGL.so.1` (permission denied by the sandbox); the switch is not kept |

The multi-threaded GPU process is therefore not an NVIDIA property. A thread
capture of the host family (`scripts/browser-qualification/gpu-process-threads.py`,
archived in `mesa-headless-wayland-gpu-threads-20260906T121731Z/host-family-threads.jsonl`)
names the threads: within 160 ms of GL initialization the zygote-forked GPU
process carries Mesa radeonsi's `cs0`, `gdrv0`, `gl0`, `disk$0`, `sh0` to
`sh11`, `sh_opt0` and four `traceq0` workers (`libEGL_mesa`, `libdrm_amdgpu`
and `libLLVM` mapped), and dies 1.5 s later on the sandbox FATAL. Chromium
initializes the Linux desktop GPU sandbox after GL initialization and requires
a single-threaded process at that point; every Mesa driver that spawns
compiler and submission threads at context creation defeats it.

Stock Google Chrome on the same headless Mesa compositor
(`mesa-headless-chrome-gpu-process-20260906T140912Z`) shows the identical
thread set in its GPU process, logs
`InitializeSandbox() called with multiple threads in process gpu-process` as a
WARNING and keeps running with `Seccomp: 0` for the life of the process. The
only difference with the host is `--gpu-sandbox-failures-fatal=yes`, which
turns that warning into the FATAL. `--gpu-sandbox-start-early` does not help
on a desktop distribution: the sandbox then starts before the driver is loaded
and ANGLE cannot `dlopen` the native `libEGL.so.1` through the broker.

A host variant without the fatal switch, otherwise identical
(`mesa-headless-wayland-gpu-sandbox-tolerated-20260906T121259Z`), presents
the fixture under native Wayland on RADV: two pool generations, keyboard and
click observed by the page, resize observed, about 0.5 ms per blit-and-fence
on the steady frame, renderers with seccomp 2, `NoNewPrivs` 1 and their own
user and PID namespaces, GPU process with `NoNewPrivs` 1 and no seccomp,
exactly Chrome's Linux model. The `scale` step timed out in that run: CEF
produced no new frame after a device-scale-only change (`notify_screen_info_changed`
plus `was_resized` at an unchanged logical size), which the NVIDIA X11 run
had handled; it is an open item of the same story. A headless compositor has
no real presentation feedback, so M1 stays unmeasured there in any case.

### Source-built CEF follow-up, 2026-09-06

The three source patches in `native/browser/experiments/tsync/` were built and
tested through the real host and GPUI. The series prepares the broker before
driver threads, installs the existing GPU policy with TSYNC, preserves CEF's
offscreen scale on Wayland, and initializes X11 GBM before sandbox. The X11
host switch described above enables that last initialization path.

AMD Raphael/Mesa 26.1.8 under headless native Wayland and NVIDIA 610.57.04
under XWayland pass input, resize, scale 1x/2x, host loss/retry and shutdown.
The two GPU processes observed during each lifecycle have every observed
thread filtered: 32 threads per AMD process and 20 per NVIDIA process.
Both also pass 120 shader programs with fresh native caches and pixel checks.
No observed process remains after shutdown. The final AMD socket inventories
show no X11 connection. Initial scale and GBM failures are retained.

Evidence is indexed by
`bench/browser/evidence/prototype/cef-tsync-integrated-20260906/summary.json`.
NVIDIA native Wayland still fails on Skia surface creation. Native Xorg and
physical M1 A/B/C remain unexecuted. The experimental build has assertions but
disables CFI/LTO; it is not a qualified shipping artifact. The normal manifest
and runtime remain unchanged, and the strict GPU sandbox/fatal switch remain
active. The host still performs the documented GPU copy.

See [TSYNC source experiment](gpu-sandbox-tsync.md) for the exact source
causes, patch series, earlier probe limitations and complete receipts.

## Native presentation boundary

The Linux reference measures Chromium's `Graphics.Pipeline.DrawAndSwap` begin
to its matching `Display::FrameDisplayed` timestamp. The end is accepted only
when it matches an actual `wp_presentation_feedback.presented` event on the
same Wayland surface with Vsync, HwClock and HwCompletion flags. Pairing uses
exact timestamps and frame identities; callback arrival is not the endpoint.
The parser rejects clock mismatches, trace loss, reused live frame IDs,
discarded frames in the measured window, missing feedback and display changes. Chromium's microsecond
trace precision contributes a conservative 1,000 ns uncertainty. The raw
Wayland timestamp retains its nanoseconds.

This follows the pinned Chromium
[display implementation](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/components/viz/service/display/display.cc#1338)
and [process metadata](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/services/tracing/public/cpp/perfetto/track_name_recorder.cc#114).
The parser checks trace-buffer loss rather than treating a truncated trace as
complete. The default upstream buffer is
[200 MiB](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/services/tracing/public/cpp/perfetto/perfetto_config.cc#259).

XWayland Present UST was investigated and excluded from these references:
[XWayland 24.1.13](https://gitlab.freedesktop.org/xorg/xserver/-/blob/xwayland-24.1.13/hw/xwayland/xwayland-present.c#L533)
can produce it from a frame callback or timer without native presentation
feedback. The native Wayland path avoids that ambiguity. It remains compositor
presentation evidence, not a photodiode measurement of physical light output.

The terminal reference independently requests `wp_presentation` feedback after
painting the matching PTY marker through GPUI's real glyph path. Synthetic
scheduled input enters the normal GPUI terminal key handler. Latency starts
there, excluding IPC registration and transport; it ends at the compositor
presentation timestamp. A CLOCK_MONOTONIC calibration bracket, surface ID,
input tick and feedback ID bind the raw events. Logging runs off the render
thread, and missing markers or dropped log records reject the capture.

## Proof scope

The local lifecycle and presentation path has been observed on Fedora 44,
GNOME 50.4 Wayland and NVIDIA 610.57.04 at nominal 60 Hz (59.951 Hz actual).
The [local receipt](../../bench/browser/evidence/m1/README.md) and
[measurement protocol](../../bench/browser/README.md) identify the full
M1 archives and separates them from shorter diagnostics and CPU benchmarks.
Distribution qualification, native aarch64, other GPU drivers, 120 Hz and the
future integrated configuration C are separate evidence requirements.

The first [two](../../bench/browser/evidence/linux-witness-rejected-1.json)
[bootstrap investigations](../../bench/browser/evidence/linux-witness-rejected-2.json)
remain rejected historical records: process role attribution was missing and
the close sequence was not yet proven. Later Views/trace-metadata work fixes
those causes; the old records are not rewritten into successful captures.
