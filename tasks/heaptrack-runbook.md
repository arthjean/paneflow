# Heaptrack Runbook

Historical companion for `tasks/devto-v0.3.4-hardening.md`.

The original v0.3.4 draft referenced a heaptrack procedure for reproducing RAM
claims. The current codebase has since moved the agent UI paths that draft cited,
so treat this file as a placeholder for rerunning the measurement, not as a
validated current benchmark.

## Procedure

1. Build a release binary with debug symbols available.
2. Launch Paneflow under `heaptrack` with a clean config and a demo workspace.
3. Drive five agent panes through the same long-response scenario.
4. Compare retained allocations before and after the target change.
5. Record the exact commit, OS, GPU backend, scenario prompt, and heaptrack
   summary before quoting any RAM number publicly.
