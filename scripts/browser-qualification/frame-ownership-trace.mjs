const limit = 64 * 1024 * 1024;
const uint64 = (1n << 64n) - 1n;
const stages = {
  releases_dequeued: "dequeued_ns",
  release_next_frame: "completion_scheduled_ns",
  release_scene_replaced: "completion_scheduled_ns",
  release_gpu_completed: "gpu_completed_ns",
};
const nativeStages = new Set(["frame_slot_occupied", "frame_slot_starved", "frame_ack_received", "frame_slot_released"]);
const scalarStages = ["occupied_ns", "dequeued_ns", "completion_scheduled_ns", "gpu_completed_ns", "enqueue_start_ns", "enqueue_end_ns", "host_received_ns", "host_ui_ns", "slot_free_ns"];
const validTime = value => Number.isSafeInteger(value) && value >= 0;
const nonempty = value => typeof value === "string" && value.length > 0;

function identifier(value, name) {
  if (Number.isSafeInteger(value) && value >= 0) return String(value);
  if (typeof value === "string" && /^(0|[1-9][0-9]*)$/.test(value) && BigInt(value) <= uint64) return value;
  throw new Error(`invalid ${name}`);
}

function documentIdentity(document) {
  if (!document || !nonempty(document.browser) || !nonempty(document.owner?.workspace) || !nonempty(document.owner?.session)) throw new Error("invalid document identity");
  return { owner: { workspace: document.owner.workspace, session: document.owner.session }, browser: document.browser, generation: identifier(document.generation, "document generation") };
}

function identity(value, frame = true) {
  const result = { document: documentIdentity(value.document), pool_generation: identifier(value.pool_generation, "pool generation") };
  if (frame) Object.assign(result, { buffer: identifier(value.buffer, "buffer"), sequence: identifier(value.sequence, "frame sequence") });
  return result;
}

function parse(input) {
  if (Array.isArray(input)) {
    if (input.length > 500_000) throw new Error("ownership event limit exceeded");
    return input;
  }
  if (typeof input !== "string" || Buffer.byteLength(input) > limit) throw new Error("invalid or oversized ownership log");
  if (!input.endsWith("\n")) throw new Error("truncated ownership JSONL");
  return parse(input.split("\n").filter(line => line.length > 0).map((line, index) => {
    try { return JSON.parse(line); } catch { throw new Error(`invalid ownership JSON at line ${index + 1}`); }
  }));
}

function summarize(values, total) {
  const finite = values.filter(value => Number.isSafeInteger(value) && value >= 0).sort((left, right) => left - right);
  const percentile = fraction => finite[Math.ceil(finite.length * fraction) - 1] ?? null;
  return { samples: finite.length, unavailable_or_negative: total - finite.length, min_ns: finite[0] ?? null, p50_ns: percentile(0.5), p95_ns: percentile(0.95), p99_ns: percentile(0.99), max_ns: finite.at(-1) ?? null };
}

export function analyzeFrameOwnership(input) {
  const events = parse(input);
  const issues = [];
  const issue = (code, details = {}) => { const result = { code, ...details }; issues.push(result); return result; };
  const hosts = [];
  const records = [];
  let currentHost = null;
  let relevant = 0;
  for (const [offset, event] of events.entries()) {
    const line = offset + 1;
    if (!event || typeof event !== "object") { issue("invalid_event", { line }); continue; }
    if (event.event === "host_ready") {
      if (!Number.isSafeInteger(event.pid) || event.pid <= 0 || !validTime(event.at_ns)) { issue("invalid_host_ready", { line }); continue; }
      if (currentHost) issue("overlapping_host_lifecycles", { line, previous_host_epoch: currentHost.epoch });
      currentHost = { epoch: hosts.length + 1, pid: event.pid, ready_ns: event.at_ns, end_ns: null, ready_line: line };
      hosts.push(currentHost);
      continue;
    }
    if (["host_lost", "host_stopped"].includes(event.event)) {
      if (currentHost) {
        if (!validTime(event.at_ns)) issue("invalid_host_end", { line });
        else currentHost.end_ns = event.at_ns;
        currentHost = null;
      }
      continue;
    }
    const native = event.event === "native" && event.native && typeof event.native === "object" ? event.native : null;
    const kind = native?.native ?? event.event;
    if (!Object.hasOwn(stages, kind) && !nativeStages.has(kind) && !["release_ack_enqueued", "ack_failed"].includes(kind)) continue;
    relevant++;
    const data = native ?? event;
    if (kind === "ack_failed" && !data.ack) { issue("ack_failure_without_identity", { line, error: data.error }); continue; }
    const values = Object.hasOwn(stages, kind) ? data.acks : [kind === "release_ack_enqueued" || kind === "ack_failed" ? data.ack : data];
    if (!Array.isArray(values) || values.length === 0 || values.length > 256) { issue("invalid_ack_batch", { line, kind }); continue; }
    for (const value of values) {
      try {
        if (!native && value?.type !== "release") throw new Error("invalid ACK type");
        const key = identity(value, kind !== "frame_slot_starved");
        const record = { line, kind, data, identity: key, base_key: JSON.stringify(key), native: native !== null, host: null, clock_valid: kind === "ack_failed" || data.clock === "CLOCK_MONOTONIC" };
        if (!record.clock_valid) issue("invalid_clock", { line, kind });
        if (native) {
          const at = kind === "frame_ack_received" ? data.ui_dispatch_ns : data.at_ns;
          const declaredPid = data.host_pid ?? data.pid;
          const initialized = currentHost && events[currentHost.ready_line - 1].initialized?.trace_us;
          const lower = validTime(initialized * 1000) ? initialized * 1000 : currentHost?.ready_ns;
          if (!currentHost || (declaredPid !== undefined && declaredPid !== currentHost.pid) || (validTime(at) && at < lower)) {
            issue("unresolved_native_host", { line, kind, declared_pid: declaredPid ?? null });
          } else record.host = currentHost;
        }
        records.push(record);
      } catch (error) { issue("invalid_identity", { line, kind, reason: error.message }); }
    }
  }
  const frames = new Map();
  const byIdentity = new Map();
  const getFrame = (record, host) => {
    const key = JSON.stringify([host?.epoch ?? null, record.identity]);
    let frame = frames.get(key);
    if (!frame) {
      frame = { frame_key: key, host_epoch: host?.epoch ?? null, host_pid: host?.pid ?? null, identity: record.identity, capture_counter: null, capture_timestamp_us: null, stage_events: {}, stages: {}, durations_ns: {}, missing_stages: [], races: [], lines: [] };
      frames.set(key, frame);
      const candidates = byIdentity.get(record.base_key) ?? [];
      candidates.push(frame);
      byIdentity.set(record.base_key, candidates);
    }
    return frame;
  };
  const allocations = new Map();
  for (const record of records.filter(record => record.native && record.kind !== "frame_slot_starved" && record.host)) {
    getFrame(record, record.host);
    if (record.kind === "frame_slot_occupied" && record.clock_valid && validTime(record.data.at_ns)) {
      const key = JSON.stringify([record.host.epoch, record.base_key]);
      const timestamps = allocations.get(key) ?? [];
      timestamps.push(record.data.at_ns);
      allocations.set(key, timestamps);
    }
  }
  const unresolved = [];
  const starved = [];
  const poolEvents = [];
  const recordStage = (frame, name, timestamp, record) => {
    const list = frame.stage_events[name] ?? [];
    list.push({ ns: record.clock_valid && validTime(timestamp) ? timestamp : null, line: record.line });
    frame.stage_events[name] = list;
    if (!validTime(timestamp)) issue("missing_or_invalid_timestamp", { line: record.line, kind: record.kind, stage: name, frame_key: frame.frame_key });
  };
  for (const record of records) {
    const { data, kind, line } = record;
    if (kind === "frame_slot_starved") {
      let counter = null;
      let timestamp = null;
      try { counter = identifier(data.capture_counter, "capture counter"); timestamp = identifier(data.capture_timestamp_us, "capture timestamp"); }
      catch (error) { issue("missing_starved_capture_identity", { line, reason: error.message }); }
      const row = { host_epoch: record.host?.epoch ?? null, host_pid: record.host?.pid ?? null, ...record.identity, capture_counter: counter, capture_timestamp_us: timestamp,
        at_ns: record.clock_valid && validTime(data.at_ns) ? data.at_ns : null, callback_ns: validTime(data.callback_ns) ? data.callback_ns : null,
        occupied_slots: data.occupied_slots ?? null, total_slots: data.total_slots ?? null, line };
      if (row.at_ns === null) issue("missing_or_invalid_timestamp", { line, kind });
      starved.push(row);
      poolEvents.push({ record, row, operation: "starved", at_ns: row.at_ns });
      continue;
    }
    let frame;
    if (record.native) {
      if (record.host) frame = getFrame(record, record.host);
    } else {
      let candidates = byIdentity.get(record.base_key) ?? [];
      const at = Object.hasOwn(stages, kind) ? data.stage_ns : data.enqueue_start_ns;
      candidates = candidates.filter(candidate => {
        const timestamps = allocations.get(JSON.stringify([candidate.host_epoch, record.base_key])) ?? [];
        return timestamps.length === 0 || !validTime(at) || timestamps.some(timestamp => timestamp <= at);
      });
      if (candidates.length === 1) frame = candidates[0];
      else if (candidates.length === 0 && hosts.length === 1) frame = getFrame(record, hosts[0]);
      else issue("ambiguous_or_missing_frame_host", { line, kind, candidate_host_epochs: candidates.map(candidate => candidate.host_epoch) });
    }
    if (!frame) { unresolved.push({ line, kind, identity: record.identity }); continue; }
    frame.lines.push(line);
    if (Object.hasOwn(stages, kind)) recordStage(frame, stages[kind], data.stage_ns, record);
    else if (kind === "release_ack_enqueued") {
      recordStage(frame, "enqueue_start_ns", data.enqueue_start_ns, record);
      recordStage(frame, "enqueue_end_ns", data.enqueue_end_ns, record);
    } else if (kind === "frame_ack_received") {
      recordStage(frame, "host_received_ns", data.received_ns, record);
      recordStage(frame, "host_ui_ns", data.ui_dispatch_ns, record);
      poolEvents.push({ record, frame, operation: "ack_received", at_ns: record.clock_valid && validTime(data.ui_dispatch_ns) ? data.ui_dispatch_ns : null });
    } else if (kind === "frame_slot_occupied") {
      recordStage(frame, "occupied_ns", data.at_ns, record);
      try {
        const counter = identifier(data.capture_counter, "capture counter");
        const timestamp = identifier(data.capture_timestamp_us, "capture timestamp");
        if (frame.capture_counter !== null && (frame.capture_counter !== counter || frame.capture_timestamp_us !== timestamp)) {
          issue("conflicting_capture_identity", { line, frame_key: frame.frame_key, previous_counter: frame.capture_counter, counter, previous_timestamp_us: frame.capture_timestamp_us, timestamp_us: timestamp });
        } else { frame.capture_counter = counter; frame.capture_timestamp_us = timestamp; }
      }
      catch (error) { issue("missing_capture_identity", { line, frame_key: frame.frame_key, reason: error.message }); }
      poolEvents.push({ record, frame, operation: "occupied", at_ns: record.clock_valid && validTime(data.at_ns) ? data.at_ns : null });
    } else if (kind === "frame_slot_released") {
      recordStage(frame, "slot_free_ns", data.at_ns, record);
      poolEvents.push({ record, frame, operation: "released", at_ns: record.clock_valid && validTime(data.at_ns) ? data.at_ns : null });
    } else frame.races.push(issue("ack_enqueue_failed", { line, frame_key: frame.frame_key, reason: data.error }));
  }
  for (const frame of frames.values()) {
    for (const name of scalarStages) {
      const values = frame.stage_events[name] ?? [];
      frame.stages[name] = values.length === 1 ? values[0].ns : null;
      if (values.length !== 1 || values[0].ns === null) frame.missing_stages.push(name);
      if (values.length > 1) issue("duplicate_stage", { frame_key: frame.frame_key, stage: name, lines: values.map(value => value.line) });
    }
    const times = frame.stages;
    const difference = (name, from, to) => {
      const value = times[from] === null || times[to] === null ? null : times[to] - times[from];
      frame.durations_ns[name] = value;
      if (value !== null && value < 0) frame.races.push(issue("reversed_stage_order", { frame_key: frame.frame_key, from, to, duration_ns: value }));
    };
    for (const [name, from, to] of [
      ["dequeue_to_completion_scheduled", "dequeued_ns", "completion_scheduled_ns"], ["completion_scheduled_to_gpu_callback", "completion_scheduled_ns", "gpu_completed_ns"],
      ["gpu_callback_to_enqueue_start", "gpu_completed_ns", "enqueue_start_ns"], ["gpu_callback_to_enqueue_end", "gpu_completed_ns", "enqueue_end_ns"],
      ["enqueue_call", "enqueue_start_ns", "enqueue_end_ns"], ["host_received_to_ui", "host_received_ns", "host_ui_ns"],
      ["host_ui_to_slot_free", "host_ui_ns", "slot_free_ns"], ["host_received_to_slot_free", "host_received_ns", "slot_free_ns"],
      ["occupied_to_slot_free", "occupied_ns", "slot_free_ns"], ["dequeued_to_slot_free", "dequeued_ns", "slot_free_ns"],
    ]) difference(name, from, to);
    const enqueueKnown = [times.enqueue_start_ns, times.enqueue_end_ns, times.host_received_ns].every(value => value !== null);
    frame.ack_transport_bounds_ns = enqueueKnown ? { min: times.host_received_ns - times.enqueue_end_ns, max: times.host_received_ns - times.enqueue_start_ns } : null;
    if (enqueueKnown && times.host_received_ns < times.enqueue_start_ns) frame.races.push(issue("host_received_before_ack_enqueue", { frame_key: frame.frame_key }));
    else if (enqueueKnown && times.host_received_ns < times.enqueue_end_ns) frame.races.push({ code: "host_received_during_enqueue_call", ordering: "valid_concurrent_overlap" });
    if (times.occupied_ns !== null && times.dequeued_ns !== null && times.dequeued_ns < times.occupied_ns) frame.races.push(issue("dequeued_before_occupied", { frame_key: frame.frame_key }));
    const host = hosts.find(host => host.epoch === frame.host_epoch);
    if (host?.end_ns !== null && host?.end_ns !== undefined) {
      const late = ["dequeued_ns", "completion_scheduled_ns", "gpu_completed_ns", "enqueue_end_ns"].filter(name => times[name] !== null && times[name] > host.end_ns);
      if (late.length) frame.races.push({ code: "callbacks_after_host_end", stages: late, host_end_ns: host.end_ns });
    }
    if (frame.missing_stages.length) issue("incomplete_frame_stages", { frame_key: frame.frame_key, stages: frame.missing_stages });
  }
  const pools = new Map();
  for (const event of poolEvents) {
    const { record } = event;
    const key = JSON.stringify([record.host?.epoch ?? null, record.identity.document, record.identity.pool_generation]);
    const pool = pools.get(key) ?? { pool_key: key, host_epoch: record.host?.epoch ?? null, document: record.identity.document, pool_generation: record.identity.pool_generation, events: [] };
    pool.events.push(event);
    pools.set(key, pool);
  }
  for (const pool of pools.values()) {
    pool.events.sort((left, right) => (left.at_ns ?? Infinity) - (right.at_ns ?? Infinity) || left.record.line - right.record.line);
    const slots = new Map();
    let previousCount = null;
    let capacity = null;
    pool.timeline = [];
    for (const [index, entry] of pool.events.entries()) {
      const { data, identity: frame, line } = entry.record;
      const count = entry.operation === "ack_received" ? data.occupied_slots_at_ui_dispatch : data.occupied_slots;
      const total = data.total_slots;
      const problems = [];
      const fail = code => problems.push(issue(code, { pool_key: pool.pool_key, line }));
      const countsValid = Number.isSafeInteger(total) && total > 0 && total <= 256 && Number.isSafeInteger(count) && count >= 0 && count <= total;
      if (!countsValid) fail("invalid_slot_counts");
      if (capacity !== null && capacity !== total) fail("pool_capacity_changed");
      if (countsValid) capacity = total;
      const tied = entry.at_ns !== null && (pool.events[index - 1]?.at_ns === entry.at_ns || pool.events[index + 1]?.at_ns === entry.at_ns);
      if (tied) fail("equal_timestamp_occupancy_events");
      if (entry.at_ns === null || tied || !countsValid) {
        slots.clear();
        previousCount = null;
      } else {
        if (entry.operation === "occupied" || entry.operation === "released") {
          const id = JSON.stringify(frame);
          if (BigInt(frame.buffer) >= BigInt(total)) fail("buffer_exceeds_capacity");
          if (entry.operation === "occupied") {
            if (slots.has(frame.buffer)) fail("buffer_occupied_without_release");
            slots.set(frame.buffer, id);
          } else {
            if (!slots.has(frame.buffer)) fail("release_without_observed_occupation");
            else if (slots.get(frame.buffer) !== id) fail("release_identity_differs_from_occupied_frame");
            slots.delete(frame.buffer);
          }
          const delta = entry.operation === "occupied" ? 1 : -1;
          if (previousCount !== null && previousCount + delta !== count) fail("occupancy_count_gap");
        } else if (previousCount !== null && previousCount !== count) fail("occupancy_count_gap");
        if (entry.operation === "starved" && count !== total) fail("starvation_with_free_capacity");
        previousCount = count;
      }
      if (countsValid && slots.size > count) fail("known_occupancy_exceeds_reported_count");
      pool.timeline.push({ operation: entry.operation, at_ns: entry.at_ns, line, occupied_slots: countsValid ? count : null, total_slots: countsValid ? total : null,
        known_occupied_slots: [...slots.keys()], unaccounted_occupied_slots: countsValid && count >= slots.size ? count - slots.size : null,
        identity: entry.operation === "starved" ? null : frame, capture_counter: entry.row?.capture_counter ?? entry.frame?.capture_counter ?? null, issues: problems });
    }
    delete pool.events;
  }
  const resultFrames = [...frames.values()];
  const names = new Set(resultFrames.flatMap(frame => Object.keys(frame.durations_ns)));
  return {
    schema_version: 1, kind: "frame_ownership_diagnostic", clock: "CLOCK_MONOTONIC", qualification: "NOT_EVALUATED", diagnostic_only: true,
    hosts, frames: resultFrames, starved, pools: [...pools.values()], unresolved, issues,
    counts: { input_events: events.length, relevant_events: relevant, frames: resultFrames.length, complete_stage_chains: resultFrames.filter(frame => frame.missing_stages.length === 0 && frame.races.length === 0).length,
      incomplete_stage_chains: resultFrames.filter(frame => frame.missing_stages.length > 0).length, frames_with_races: resultFrames.filter(frame => frame.races.length > 0).length, starved_captures: starved.length, unresolved_records: unresolved.length },
    duration_summary: Object.fromEntries([...names].map(name => [name, summarize(resultFrames.map(frame => frame.durations_ns[name]), resultFrames.length)])),
  };
}
