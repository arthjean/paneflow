# Linux GPU sandbox: TSYNC feasibility

The 2026-09-06 source-built CEF experiment passes the complete functional
CEF -> DMA-BUF -> GPUI scenario on AMD Raphael under headless native Wayland
and NVIDIA RTX 4070 Ti SUPER under both native Wayland and XWayland. The
native Wayland shader runs retain Chromium's GPU sandbox and pass 120 shader
programs with fresh caches. Native Xorg and complete physical M1 comparisons
remain open; this is not a release qualification.

## Integrated CEF results

The full CEF build, runtime hash/extraction/GLIBC verification and isolated
PaneFlow app/host build completed. Evidence and source receipts are indexed by
`bench/browser/evidence/prototype/cef-tsync-integrated-20260906/summary.json`.
The initial full source build receipts are in the adjacent
`cef-tsync-source-build-20260906/` directory. The production manifest is unchanged.

| Configuration | Functional lifecycle | Shader compilation | Remaining limit |
|---|---|---|---|
| AMD Raphael, Mesa 26.1.8, headless Mutter 50.4 Wayland | PASS: input, resize, scale 1x/2x, host loss/retry, shutdown | PASS: 120 programs and pixel checks, fresh caches | No physical presentation timing |
| NVIDIA 4070 Ti SUPER, 610.57.04, XWayland | PASS: same lifecycle | PASS: same shader workload, fresh caches | Not a native Xorg session |
| NVIDIA, headless native Wayland | PASS: input, resize, scale, host loss/retry, shutdown | PASS: 120 programs and pixel checks, fresh caches | Physical M1 and hardened candidate remain open |

The lifecycle captures observe two GPU processes per configuration, before and
after host restart: 32 threads each on AMD and 20 each on NVIDIA. Every
observed GPU thread has seccomp mode 2, NoNewPrivs 1 and an installed filter.
Renderer sandbox checks pass. All 26 observed processes per lifecycle run
have exited after shutdown. These snapshots cover observed processes and
threads, not every transient task across the whole execution.

The original three source patches establish sandbox startup and geometry:

1. Create the GPU filesystem broker before driver threads, then install the
   existing GPU filter using TSYNC after driver initialization.
2. Preserve CEF's offscreen scale when `GetNativeView()` is null. Chromium's
   per-window Wayland override otherwise replaces the supplied scale with 1.0.
3. Initialize X11 GBM synchronously during GPU platform initialization, after
   the early broker and before sandbox installation. The previous ThreadPool
   task ran after sandbox and could not load the GBM backend.

The NVIDIA Wayland correction is indexed separately by
`bench/browser/evidence/prototype/cef-tsync-nvidia-wayland-20260906/summary.json`.
CEF originally requests a CPU-mappable capture allocation, which becomes
`DRM_FORMAT_MOD_LINEAR`. The driver reports this modifier as external-only for
BGRA/RGBA, and actual CEF GL attachment fails. The fourth patch selects native
exportable capture images on Linux and completes Chromium's corresponding
shared-image result path. It preserves the existing GPU-completion wait before
CEF receives the handle; no CPU pixel mapping is introduced.

The resulting NVIDIA tiled buffer includes padding beyond Chromium's reported
`stride * height` plane size. The Vulkan host now discovers the allocation size
with `lseek(SEEK_END)` and resets with `SEEK_SET`, validates the plane metadata
against that size, and refuses imports whose Vulkan requirements exceed it.
This follows the [Linux DMA-BUF userspace interface](https://docs.kernel.org/driver-api/dma-buf.html#userspace-interface-notes).
Imported descriptors are duplicated atomically with `F_DUPFD_CLOEXEC`.

Patch five adds capture-request lifetime to DrawAndSwap trace correlation for
M1. Patch six bounds Linux linker parallelism without disabling CFI or ThinLTO.
A separate official CEF candidate is being built with CFI indirect-call checks,
ThinLTO and the pinned Chromium/V8 PGO profiles. Its build and native validation
are not yet complete. The current functional runtime contains temporary GL
logging and is not a timing reference.

The host explicitly enables native GPU memory buffers in X11 presentation
mode. CEF's `shared_texture_enabled` alone does not set that GPU preference,
so the third patch's initialization branch would otherwise remain unused.
The switch also enables native mappable-buffer capabilities; it neither
disables sandbox nor forces CPU readback. Presentation retains its GPU blit,
fence and DMA-BUF transfer. The shader page reads one pixel per program as a
test oracle, independently of the presentation path.

Initial failures remain archived: the first TSYNC runtime fixes sandbox but
still times out on Mesa scale; the initial X11 GBM patch without the host
switch still fails. The socket collector also exceeded its default 1 MiB
output limit; a bounded 16 MiB capture fixes that diagnostic, and the final
AMD runs measure no X11 connections. No sandbox policy rule was broadened.

The build uses assertions with `dcheck_always_on=true`, but disables CFI, LTO
and symbols for the local experiment. Shipping artifacts must be qualified
separately. The cache-sensitive native EGL failure below is retained: the
successful CEF tests do not turn it into a general driver-policy guarantee.

## Real Chromium policy and early broker

The patch at `native/browser/experiments/tsync/` creates the desktop Linux GPU
broker before driver initialization, with Chromium's existing command set and
file permissions. It stops an existing Perfetto thread before broker creation,
keeps it stopped during driver initialization, and restores it through the
existing sandbox restart path. Chromium's normal post-driver initialization
then selects the actual GPU policy with TSYNC enabled. The single-thread broker
assertion, open-directory checks and fatal sandbox failures remain active.
ChromeOS, Chromecast and the existing early-sandbox path keep their startup.

The exact source commits, patch digest and GN arguments are recorded in
`native/browser/experiments/tsync/experiment.json`. Both changed translation
units compiled with `dcheck_always_on=true`. The standalone policy probe links
Chromium's sandbox implementation and GPU hook directly; it loads native
system EGL/GLES, with ANGLE used for headers only.

`bench/browser/evidence/prototype/real-policy-tsync-20260906-r2/` records PASS
on AMD Raphael with Mesa 26.1.8 and RTX 4070 Ti SUPER with NVIDIA 610.57.04:

- Broker creation succeeds before driver threads.
- A preexisting witness and the main thread report seccomp mode 2.
- Both receive EACCES opening `/etc/passwd`, which was readable before TSYNC.
- Both retain access to an existing file allowed by Chromium's GPU broker.
- 120 shader compilations and pixel checks pass after filtering.
- EGL teardown and normal process exit succeed, including on NVIDIA.

A cache-enabled follow-up at
`bench/browser/evidence/prototype/real-policy-tsync-cache-20260906/` passes on
NVIDIA but fails on Mesa after TSYNC. In ten additional fresh-cache Mesa runs,
nine terminate in Chromium's SIGSYS crash handler for `sched_setscheduler`
(syscall 144 on x86_64). One completes. A traced run also completes, so the
failure is timing-sensitive. The scheduler policy only permits the main PID,
zero or the calling thread, and traps requests targeting another thread.
The subsequent real CEF/ANGLE shader runs above pass with fresh native caches
on the named configurations. The passing cache-disabled native EGL probe
remains a conditional result, not a general Mesa-policy qualification.

The syscall traces from the passing cache-disabled runs show successful kernel
TSYNC installation and both brokers exiting after their clients. Chromium's
signal broker deliberately uses exit code 1 after its request loop ends on
client EOF; the main probe must still exit with code 0.

The probe does not install namespace isolation and does not exercise CEF,
ANGLE rendering, DMA-BUF or GPUI. Its pixel readback is a test oracle. The
full runtime results are reported separately above. Initial probe setup failures
are retained in the adjacent unsuffixed receipt directory.

## Local evidence

The receipt directory is
`bench/browser/evidence/prototype/tsync-20260906/`. Each GPU directory contains
the exact C source, its digest, compile command and logs, per-case JSONL,
thread snapshots and `result.json`. The probe runs as an ordinary user without
a compositor, browser, root access or changes to the display configuration.

| Condition | AMD Raphael, Mesa 26.1.8 radeonsi | RTX 4070 Ti SUPER, NVIDIA 610.57.04 |
|---|---|---|
| Kernel | Linux 7.1.12-200.fc44.x86_64 | Same |
| Render node | `/dev/dri/renderD128` | `/dev/dri/renderD129` |
| Threads at filter installation | 11: main, 9 driver threads, one witness | 2: main and one witness |
| Without TSYNC, marker denied on main only | PASS | PASS |
| TSYNC, marker denied on main and existing witness | PASS | PASS |
| All observed threads receive Seccomp 2, one filter, NoNewPrivs 1 | PASS | PASS |
| 120 renders after synthetic bounded filter | PASS | Pixels pass; process exit FAIL |
| Deliberately forbidden `socket` | Expected SIGSYS | Expected SIGSYS |

The AMD snapshots include `cs0`, `sh0`, `sh_opt0`, four `traceq0`, `gdrv0`
and `gl0`. This exercises a real multithreaded radeonsi context, but not the
exact ANGLE/CEF context from the earlier captures. The NVIDIA EGL path did
not create the driver-thread topology observed in CEF's Vulkan path.

The NVIDIA bounded-filter process finishes rendering and EGL teardown, then
dies during process exit. The targeted strace identifies `sendmsg` with
`si_code=SYS_SECCOMP`; the synthetic allowlist omits this syscall. The trace
excerpt is `nvidia/bounded-exit.strace-excerpt.txt`. This is a failure of this
probe policy for that lifecycle, not evidence that Chromium denies the same
call. The runner requires a successful process exit and keeps the case FAIL
even though the program printed its completion event.

An initial NVIDIA attempt using automatic EGL vendor enumeration failed
before filter installation with `EGL_NOT_INITIALIZED`. Explicit selection of
the NVIDIA EGL vendor resolved that probe setup issue. The exploratory logs
are retained separately from the reproducible final runs.

## What the filters prove

`scripts/browser-qualification/gpu-tsync.py` compiles and launches
`gpu-tsync-linux.c` in fresh processes with a 45-second deadline per case.
Mesa's disk shader cache is disabled. After an initial warmup draw, the probe
installs the filter with libseccomp's TSYNC attribute, compiles varying shader
sources, draws a triangle 120 times, waits for completion and checks a pixel.
The CPU pixel read is a test oracle in this standalone probe, not the Browser
presentation path or a proposed fallback. These runs measure no M1 budget.

The marker policy permits all syscalls except `getppid`, which returns EPERM.
The control without TSYNC shows that this marker still works on an existing
worker. With TSYNC it is denied on both main and worker, while `/proc` shows
the filter on every observed driver thread.

The second policy defaults to TRAP and permits a fixed syscall list in the C
source. It intentionally includes filesystem opens and broad `ioctl` access
needed by this experiment. It implements neither Chromium's argument checks
nor filesystem broker. It is not a production sandbox. The final control
tries an unlisted `socket` syscall and must exit with SIGSYS before that call
can create a socket. Core dumps are disabled for these controlled failures.

The [kernel documentation](https://docs.kernel.org/userspace-api/seccomp_filter.html)
explicitly distinguishes syscall filtering from a complete sandbox. A process
reporting Seccomp 2 does not establish that its policy is appropriate.

To reproduce on a Linux machine with EGL/GLES and libseccomp development
libraries installed, discover its render nodes and EGL vendor descriptors,
then use a new output directory for each run:

```bash
python3 scripts/browser-qualification/gpu-tsync.py \
  --render-node /dev/dri/renderD128 \
  --egl-vendor /usr/share/glvnd/egl_vendor.d/50_mesa.json \
  --output /tmp/paneflow-tsync-mesa-new
```

Paths above describe the recorded Fedora machine, not a portable device
selection rule. The runner reports NOT_EXECUTED outside Linux. This probe
has no Windows or macOS qualification meaning.

## Chromium integration obstacle

The reviewed upstream tag is **151.0.7922.174**, matching
`native/browser/manifest.toml`. The relevant call sequence is:

1. [`GpuInit`](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/gpu/ipc/service/gpu_init.cc#906)
   requests the sandbox after GL initialization.
2. [`StartSandboxLinux`](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/content/gpu/gpu_main.cc#499)
   stops watchdog and tracing threads, constructs GPU-specific policy options,
   and leaves `allow_threads_during_sandbox_init` false.
3. [`StartSeccompBPF`](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/sandbox/policy/linux/sandbox_linux.cc#321)
   runs the pre-sandbox hook before selecting MULTI_THREADED and installing
   the filter when that option is enabled.
4. [`GpuPreSandboxHook`](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/content/common/gpu_pre_sandbox_hook_linux.cc#698)
   starts the filesystem broker before that filter installation.
5. [`BrokerProcess::Fork`](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/sandbox/linux/syscall_broker/broker_process.cc#99)
   asserts that the process has one thread, then uses `fork` without `exec`.
   Its [child callback](https://chromium.googlesource.com/chromium/src/+/151.0.7922.174/sandbox/policy/linux/sandbox_linux.cc#124)
   performs C++ initialization, including command-line and policy allocation.

With assertions enabled, allowing the existing driver threads reaches a
contradictory broker assertion before TSYNC. Without assertions, this is still
a multithreaded fork with C++ execution in the child. A deadlock has not been
observed in this experiment; the unsupported invariant and the resulting risk
must be addressed, not removed by deleting the assertion.

The reviewed CEF patch inventory at commit
`2384915b7b1f0fe5ad1107e48d80c34e86b698d7` contains no patch of these sandbox
files. A standalone EGL probe never reaches that code.

## Integration gate and decision

The early-broker design is implemented in the isolated source experiment.
It preserves the existing desktop GPU broker permissions and passes the
recorded real CEF lifecycle on AMD Wayland, NVIDIA Wayland and NVIDIA
XWayland. The scale, GBM and native capture buffer follow-up patches are part
of the tested source series. Native NVIDIA Wayland evidence is indexed in
`bench/browser/evidence/prototype/cef-tsync-nvidia-wayland-20260906/summary.json`.

Before this path can qualify the Browser:

- Qualify the source-pinned patch series with the intended shipping hardening
  configuration and a reproducible distribution artifact.
- Finish native Wayland/X11 evidence and applicable M1 measurements. A
  headless functional result cannot stand in for physical presentation timing.
- Verify the consolidated transport fixes: GPU completion must precede buffer
  acknowledgements, an overflowing event channel permanently fails its host,
  and every loaded runtime/staging file must match the pinned manifest.

Physical M1 diagnostics exposed the default GPUI inactive-window throttle.
The Browser window now opts out through `inactive_frame_interval = None` so
terminal focus does not cap visible Browser content at 30 frames per second.
The next diagnostic recorded 172 Browser paints in three seconds at 60 Hz,
against 85 before the change. This is diagnostic evidence only: the capture
was rejected for replay timing and missing effective-display identity, and
does not establish the R0 latency or missed-frame budgets.

A later AMD Wayland recheck using the native-capture runtime passed input,
resize and scale, then failed after the explicit host restart. The GPU sandbox
trapped `sched_setscheduler` targeting another thread; subsequent GPU restarts
failed and the host aborted. Evidence:
`bench/browser/evidence/prototype/security-r5-amd-wayland-20260906/prototype/native.json`.
The earlier AMD success used a different runtime digest. Compatibility of the
current native-capture path on AMD therefore remains open; its sandbox has not
been relaxed to obtain a passing result.

The strict runtime behavior remains in place: GPU sandbox failure makes
Browser unavailable. The production runtime and manifest remain unchanged;
experimental binaries are staged only in the isolated workspace. There is no
prediction that an upstream patch will be accepted, no release qualification,
and no new DONE story.
