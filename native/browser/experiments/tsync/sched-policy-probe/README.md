# Native Chromium scheduling policy probe

This target uses `SandboxSeccompBPF::PolicyForSandboxType` and the kernel's
`SandboxBPF::SeccompLevel::MULTI_THREADED`. It does not recreate or subclass
the production BPF policy. A negative case also directly constructs the real
`GpuProcessPolicy` with its default opt-in value, which must retain fatal
denial. The GPU broker is prepared through the same
`content::PrepareGpuSandboxBroker` used by patch 0001, before witness threads
exist. It exercises GPU, renderer, on-device model and hardware video encoder
policies. The encoder case is compiled only when its factory is available
with `USE_LINUX_VIDEO_ACCELERATION`. There is no
namespace sandbox, EGL initialization, rendering, display connection or CEF
qualification in this probe.

The first native execution passed 26 cases. The renderer case stopped before
its scheduling call because the probe's `PR_GET_NO_NEW_PRIVS` query is itself
forbidden by that policy. The corrected renderer instrumentation has not been
re-executed here.

After review, apply patch 0008 to the exact Chromium checkout carrying
patches 0001 through 0007. Copy this directory to
`chromium/src/cef/paneflow_sched_policy_probe/`. The exact GN label is
`//cef/paneflow_sched_policy_probe:paneflow_sched_policy_probe`. For a root
graph which does not already reach this label, add that dependency to the
root `group("gn_all")` in the isolated checkout, then regenerate the existing
output directory without changing its GN arguments. The files in this
directory do not modify the checkout or root build graph themselves.

The target depends on Chromium `base`, `sandbox/policy`, `sandbox/linux`
seccomp and services, sandbox mojom, content static switches and media/GPU
build flags. It compiles the existing GPU pre-sandbox hook. It requires the
same source pin and patch 0001's broker preparation entry point. No separately
installed libraries or alternative BPF harness are needed beyond pthread.

Example commands, after the copy and explicit root dependency are in place:

```sh
gn gen out/Release_GN_x64
autoninja -C out/Release_GN_x64 paneflow_sched_policy_probe
python3 /absolute/repository/native/browser/experiments/tsync/sched-policy-probe/run.py \
  --binary /absolute/chromium/src/out/Release_GN_x64/paneflow_sched_policy_probe \
  --output /absolute/new-sched-evidence --run
```

Without `--run`, the Python runner only prints the matrix. The full matrix
contains positive kernel calls for zero/PID/self-TID, callers born before and
after TSYNC, denied foreign workers and the live external runner process,
the pthread error convention, invalid pointers, non-BATCH policies, reset
flags, high scalar bits, other scheduling syscalls and unchanged renderer
and model policies, the default GPU constructor and the video encoder factory.
The matrix has 27 cases. Before filtering, a valid foreign-worker SCHED_BATCH call
must succeed and is restored to SCHED_OTHER. This guards against tests which
would merely observe an unrelated kernel permission error.

Successful own-thread calls verify SCHED_BATCH reached the kernel and restore
SCHED_OTHER. Nonfatal cases require the foreign worker's policy and priority
to remain unchanged. The external target is the runner's live PID, whose
policy and priority are checked before and after every child. Each child has
a private process group; the runner terminates only that group, including its
broker, after completion or timeout. Core files are disabled. Fatal cases
require an `armed` record after filter installation, SIGSEGV from Chromium's
SIGSYS crash handler, and the expected syscall number in its diagnostic.
A setup crash or an ordinary bad-pointer SIGSEGV cannot pass.

The renderer negative control runs on the main thread and skips both main
and operation-level `PR_GET_NO_NEW_PRIVS` queries. Its `sandbox_started`
record states that this observation is forbidden by the renderer policy.
Successful installation through `StartSandbox(MULTI_THREADED)`, the armed
scheduling call and its exact Chromium SIGSYS diagnostic remain mandatory.
The passive witness is never released in this fatal case. If the scheduling
call unexpectedly returns, the probe fails before entering witness cleanup,
so an unrelated cleanup `prctl` cannot replace the expected failure. Other
policies retain every no-new-privileges check. This control does not claim a
post-install renderer NNP observation or a renderer call from a worker thread.

`receipt.json` includes the matrix, raw stdout/stderr hashes, source/patch and
binary hashes, syscall results, target scheduler witnesses and final status.
If the binary was built without the encoder factory, that case reports
`NOT_BUILT` and the aggregate can only report `PASS_AVAILABLE_CASES`, never
full `PASS`. No unavailable factory is reported as tested.
Any failed or incomplete case means the probe has not passed. Shader-cache
stress and the AMD CEF host-loss retry remain separate required validations.
