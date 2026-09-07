# Native EGL with Chromium's GPU seccomp policy

This Linux experiment compiles the patched Chromium GPU hook directly and links
Chromium's `sandbox/policy` target. It loads the host's `libEGL.so.1` and
`libGLESv2.so.2` dynamically. ANGLE supplies headers only; the renderer comes from
the selected native EGL device.

It requires Chromium 151.0.7922.174 with `PrepareGpuSandboxBroker` and
`GpuPreSandboxHookWithPreparedBroker` exposed by the adjacent TSYNC patch.

## Build

Copy `BUILD.gn` and `main.cc` to `paneflow_tsync_probe/` beneath Chromium's `src/`.
Add the target to the existing `root_extra_deps` array in the chosen output's
`args.gn`, preserving other entries:

```gn
root_extra_deps = [ "//paneflow_tsync_probe:paneflow_tsync_probe" ]
```

After any build using that output directory has finished, regenerate with the
same CEF environment used for the existing build and compile only this target:

```sh
gn gen out/Release_GN_x64
ninja -C out/Release_GN_x64 paneflow_tsync_probe
```

It depends on Chromium base, sandbox and transitive dependencies (including net),
without compiling CEF or linking Chromium content as a whole. It reuses existing
objects when added to an already configured Chromium output.

## Run

Choose the actual render node for the intended GPU:

```sh
env __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json MESA_SHADER_CACHE_DISABLE=true timeout 90s out/Release_GN_x64/paneflow_tsync_probe --render-node=/dev/dri/renderD128
env __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_nvidia.json __GL_SHADER_DISK_CACHE=0 timeout 90s out/Release_GN_x64/paneflow_tsync_probe --render-node=/dev/dri/renderD129 --nvidia
```

`--nvidia` enables the existing Chromium NVIDIA preloading option. It does not
change the syscall policy or broker whitelist. Avoid passing unrelated Chromium
switches to this standalone experiment.

The program initializes CommandLine with `type=gpu-process` and FeatureList,
selects an existing readable regular file from the real GPU broker permissions,
starts the real broker while single-threaded. SandboxLinux preinitialization
occurs later inside `InitializeSandbox`, matching the patched GPU startup path.
It then initializes native EGL/GLES, verifies one rendered frame, starts a
witness thread, and calls `InitializeSandbox(kGpu)` with the prepared broker hook
and `allow_threads_during_sandbox_init=true`.

The test requires `/etc/passwd` to be readable before sandbox activation, then
requires EACCES for that same path on both the main and preexisting witness
thread. Both threads must still open the allowed file and report seccomp mode 2.
It verifies 120 further shader compilations, draws and pixel readbacks, then
checks EGL context, surface, display and thread teardown.

A successful result requires exit status 0 and the final `completed` JSON event.
A final event followed by a crash during process teardown is a failure. Events
before termination identify the last completed stage. A timeout is also a
failure, not a partial success.

This is a seccomp and broker experiment only. It does not engage the namespace
layer and cannot certify CEF startup, ANGLE, DMA-BUF transport, GPUI rendering,
Wayland/X11, or the complete browser sandbox. It does not replace the full CEF
host qualification.

The recorded cache-disabled cases pass on both local GPUs. A follow-up with
fresh native shader caches enabled passes on NVIDIA, but Mesa triggers a
scheduling-policy crash in nine of ten additional runs. A passing run does not
establish stability of that configuration. See `docs/browser/gpu-sandbox-tsync.md`
for the separate receipts and the untested relationship to CEF/ANGLE.
