const assert = (condition, message) => { if (!condition) throw new Error(`browser process proof: ${message}`); };
const integer = value => Number.isSafeInteger(value) && value >= 0;

export function verifyBrowserProcessInterval(hostPid, originNs, endNs, traceStopNs, processEvidence, processEvidenceEnd) {
  assert(integer(hostPid) && hostPid > 0 && integer(originNs) && integer(endNs) && endNs > originNs && integer(traceStopNs) && traceStopNs >= endNs, "invalid process observation interval");
  const ticks = value => typeof value === "string" && /^[1-9][0-9]*$/.test(value) && BigInt(value) <= 0xffffffffffffffffn;
  const observations = [processEvidence, processEvidenceEnd];
  const gpus = observations.map(evidence => {
    assert(evidence?.schema_version === 1 && evidence.host_pid === hostPid && evidence.clock === "CLOCK_MONOTONIC"
      && ticks(evidence.host_start_ticks) && integer(evidence.started_ns) && integer(evidence.ended_ns) && evidence.ended_ns >= evidence.started_ns, "process evidence does not bind the observed host and clock");
    assert(Array.isArray(evidence.errors) && evidence.omitted_errors === 0 && evidence.errors.every(error => ["fd_inventory", "fd_type", "smaps_rollup"].includes(error.operation)), "process identity or thread evidence contains errors");
    assert(Array.isArray(evidence.processes), "missing native process evidence");
    const processes = new Map();
    for (const process of evidence.processes) {
      assert(integer(process.pid) && process.pid > 0 && integer(process.parent) && ticks(process.start_ticks)
        && !processes.has(process.pid), "process ancestry contains an invalid or duplicate identity");
      processes.set(process.pid, process);
    }
    const host = processes.get(evidence.host_pid);
    assert(host?.role === "host" && host.start_ticks === evidence.host_start_ticks, "process ancestry has no matching host identity");
    const matches = evidence.processes.filter(process => process.role === "gpu-process");
    assert(matches.length === 1, "GPU process identity is missing or ambiguous");
    const gpu = matches[0];
    assert(integer(gpu.pid) && gpu.pid > 0 && ticks(gpu.start_ticks) && gpu.seccomp === 2 && gpu.no_new_privs === 1
      && integer(gpu.seccomp_filters) && gpu.seccomp_filters >= 1 && Array.isArray(gpu.sandbox_flags) && gpu.sandbox_flags.length === 0, "GPU process identity and sandbox evidence are missing or ambiguous");
    const ancestry = new Set();
    let child = gpu;
    while (child.pid !== evidence.host_pid) {
      assert(!ancestry.has(child.pid), "GPU process ancestry contains a cycle");
      ancestry.add(child.pid);
      const parent = processes.get(child.parent);
      assert(parent, "GPU process ancestry contains an unknown parent");
      assert(BigInt(parent.start_ticks) <= BigInt(child.start_ticks), "GPU process ancestor is younger than its child");
      child = parent;
    }
    assert(gpu.threads_complete === true && Array.isArray(gpu.threads) && gpu.threads.length > 0
      && new Set(gpu.threads.map(thread => thread.tid)).size === gpu.threads.length
      && gpu.threads.every(thread => integer(thread.tid) && thread.tid > 0 && ticks(thread.start_ticks) && thread.seccomp === 2 && thread.no_new_privs === 1 && integer(thread.seccomp_filters) && thread.seccomp_filters >= 1), "GPU thread sandbox coverage is incomplete");
    return gpu;
  });
  assert(processEvidence.ended_ns <= originNs && processEvidenceEnd.started_ns >= endNs
    && processEvidenceEnd.ended_ns <= traceStopNs, "process observations do not bracket the full measurement before trace stop");
  assert(processEvidence.host_start_ticks === processEvidenceEnd.host_start_ticks && gpus[0].pid === gpus[1].pid && gpus[0].start_ticks === gpus[1].start_ticks, "host or GPU process was replaced during observation");
  return { gpu_pid: gpus[0].pid, gpu_start_ticks: gpus[0].start_ticks,
    resource_observation_complete: observations.every(evidence => evidence.complete === true) };
}
