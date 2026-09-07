# Mesa disk-cache scheduling denial

Status: patch and native probe prepared for review. No compilation, native
test, new capture or cache-policy change has been performed for this patch.
The source pin is CEF `151.3.24+g2384915` with Chromium `151.0.7922.174`.

## Evidence and cause

The AMD Wayland r5 prototype first delivered five frames and completed input,
resize and scale. After the deliberate host kill, the replacement GPU process
crashed three times in `sched_setscheduler` (x86-64 syscall 0x90), targeting
another TID with policy 3 (`SCHED_BATCH`). Chromium subsequently disabled GPU
GL and failed to initialize the shared capture context. The later `alarm`
SIGSYS is part of fatal reporting and is not the original failure.

The installed `/usr/lib64/libgallium-26.1.8.so` has GNU build ID
`d570aa3cd678abf987a89cc3ceda1a11ac75b737`. Its matching
[Fedora symbol table](https://debuginfod.fedoraproject.org/buildid/d570aa3cd678abf987a89cc3ceda1a11ac75b737/section/.symtab)
and string table identify the crash offsets:

| ELF return address | Symbol |
|---|---|
| `0xb5e6ab` | `si_init_shader_selector_async+0x31b` |
| `0xb5d909` | `si_shader_cache_insert_shader+0x109` |
| `0x59f630` | `disk_cache_put`, tail call into queue submission |
| `0x5b4874` | `util_queue_add_job+0x64` |
| `0x5b4378` | `util_queue_add_job_locked.part.0+0x2b8` |
| `0x5b4022` | `util_queue_adjust_num_threads+0x62` |
| `0x5b3b60` | `util_queue_create_thread+0xa0` |

The installed ELF's call at `0x5b3b5b` targets `pthread_setschedparam`; its
return address matches `0x7f22ddbb3b60` with load base `0x7f22dd600000`.
In [Mesa's official 26.1.8 source archive](https://archive.mesa3d.org/mesa-26.1.8.tar.xz),
`src/util/disk_cache.c:90` creates a disk cache queue with up to four threads
and `UTIL_QUEUE_INIT_USE_MINIMUM_PRIORITY`. `src/util/u_queue.c:569` grows it
when a job already waits. Line 344 calls `pthread_setschedparam` from the
creator against the new thread and ignores the result. This optional
latency-insensitive scheduling hint occurs after TSYNC when cache writes
cause the queue to grow.

## Scope of patch 0008

Only the desktop Linux `Sandbox::kGpu` factory opts into the new branch.
`GpuProcessPolicy` and the internal `GetGpuProcessSandbox` helper accept an
explicit `deny_foreign_batch_scheduler` parameter defaulting to false. Only
the `kGpu` factory passes true. ChromeOS, Cast, renderer, on-device model,
hardware video encoder, subclasses and other existing constructor callers
retain their behavior. `MremapPolicy` cannot define this scope: both `kGpu`
and `kHardwareVideoEncoding` use `MremapPolicy::kBlock`.

For exactly `sched_setscheduler` and the full 64-bit policy argument equal
to `SCHED_BATCH`, zero and the policy PID retain their existing `Allow`.
The original SIGSYS handler still rewrites a caller's own TID to zero. A
different target returns kernel-style `-EPERM`, without invoking a syscall
against that target or dereferencing its `sched_param` pointer. Other policy
values, reset flags and high scalar bits take the original restriction path.
Other scheduling syscalls and the shared restriction helpers are unchanged.

No new kernel operation becomes permitted. The change deliberately converts
one previously fatal refusal into a nonfatal refusal. Mesa continues with
the created thread's existing scheduling policy. Its worker still performs
its existing independent nice-priority request. The probe must establish
the return convention (`syscall`: -1/errno=EPERM; pthread: return EPERM) and
verify that target scheduling state did not change.

## Capture mode and remaining checks

Patch 0004 changes ARGB allocation from CPU-mappable scanout usage to native
scanout usage. Both capture modes use `CopyOutputRequest::kSharedImage`, GPU
blits, GPU completion and native DMA-BUF delivery. The texture completion
class's `Readback` name does not mean it reads pixels into CPU memory.
Different layouts may require different shaders, and warm caches can hide
queue growth. Neither the earlier AMD mappable-buffer PASS nor the first
successful r5 frames proves that changing capture mode removes this race.

The scoped denial avoids a global Mesa cache setting, driver-dependent
capture selection or sandbox disable switch. Before accepting it, execute
shader-cache stress and the exact AMD host-loss retry against the freshly
hashed CEF artifact. The [real-policy probe](sched-policy-probe/README.md)
passed all 27 native cases on 2026-09-06 with this exact patch. Preserve the
existing NVIDIA evidence and all failing evidence. Runtime qualification
remains pending.
