# TSYNC follow-up, 2026-09-06

See `summary.json` for the verdict and `docs/browser/gpu-sandbox-tsync.md`
from the repository root for the method, source audit and limitations.

`mesa/` contains four passing kernel/EGL probe cases. `nvidia/` contains
passing marker controls, a deliberately trapped socket control, and a
bounded-filter run that renders successfully but fails during process exit
on sendmsg. The NVIDIA failure remains a failed case.

These are diagnostic artifacts with qualification NOT_EVALUATED. They do
not run CEF, ANGLE, the Chromium broker, GPUI or an M1 presentation workload.
The prior EP-002 review and gates receipts remain historical snapshots;
this follow-up does not recertify the workspace or turn any story DONE.
