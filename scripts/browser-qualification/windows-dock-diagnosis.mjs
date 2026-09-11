import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { distribution } from "./dock-benchmark.mjs";

const read = (path) => JSON.parse(readFileSync(path, "utf8"));
const lines = (path) => readFileSync(path, "utf8").trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);

export function diagnose(directory) {
  const events = lines(join(directory, "events.jsonl"));
  const application = lines(join(directory, "application.jsonl"));
  const phases = read(join(directory, "phases.json"));
  const start = phases.find((phase) => phase.name === "steady").at_ns;
  const end = phases.find((phase) => phase.name === "end_resize").at_ns;
  const presents = application.filter((event) => event.event === "renderer_stage"
    && event.fields.stage === "dxgi_present_call" && event.at_ns >= start && event.at_ns < end);
  const gaps = presents.slice(1).map((event, i) => ({
    start_ns: presents[i].at_ns, end_ns: event.at_ns, duration_ms: (event.at_ns - presents[i].at_ns) / 1e6,
  })).filter((gap) => gap.duration_ms > 50);
  const delayed = events.filter((event) => event.event === "frame_received"
    && event.at_ns >= start && event.at_ns < end && event.at_ns - event.fields.ready_ns > 50e6);
  const longResizes = events.filter((event) => event.event === "resize_ready"
    && event.at_ns >= start && event.at_ns < end && event.fields.duration_ns > 100e6);
  const scans = application.filter((event) => event.event === "agent_binary_scan"
    && event.fields.start_ns >= start && event.fields.end_ns < end);
  const overlaps = (begin, finish) => gaps.some((gap) => gap.start_ns < finish && gap.end_ns > begin);
  const probes = events.filter((event) => event.event === "paint_probe"
    && event.fields.at_ns >= start && event.fields.at_ns < end);
  const sameTransition = (one, other) => JSON.stringify(one.fields.expected) === JSON.stringify(other.fields.expected);
  const stalls = probes.slice(1).map((event, i) => ({
    duration_ms: (event.fields.at_ns - probes[i].fields.at_ns) / 1e6,
    captured_meanwhile: event.fields.capture_counter - probes[i].fields.capture_counter,
    settled: !sameTransition(probes[i], event),
  })).filter((stall) => stall.duration_ms > 60 && !stall.settled);
  const outcomes = {};
  for (const probe of probes) outcomes[probe.fields.outcome] = (outcomes[probe.fields.outcome] ?? 0) + 1;
  const hostResizes = events.filter((event) => event.event === "resize_host");
  const stageDurations = [];
  for (const sent of events.filter((event) => event.event === "resize_sent" && event.at_ns >= start && event.at_ns < end)) {
    const host = hostResizes.find((event) => event.page === sent.page && event.fields.generation === sent.fields.generation);
    const ready = events.find((event) => event.event === "resize_ready" && event.page === sent.page
      && event.fields.generation === sent.fields.generation && event.at_ns >= sent.at_ns);
    if (!host || !ready) continue;
    const capture = events.find((event) => event.event === "resize_capture" && event.page === sent.page
      && event.fields.at_ns >= host.fields.at_ns && event.at_ns <= ready.at_ns + 5e6
      && event.fields.width === Math.ceil(sent.fields.width * sent.fields.scale / 100)
      && event.fields.height === Math.ceil(sent.fields.height * sent.fields.scale / 100));
    if (!capture) continue;
    const pool = events.find((event) => event.event === "resize_pool" && event.page === sent.page
      && event.fields.at_ns >= host.fields.at_ns && event.fields.at_ns <= capture.fields.at_ns
      && event.fields.width === capture.fields.width && event.fields.height === capture.fields.height);
    stageDurations.push({ generation: sent.fields.generation,
      send_to_host_ms: (host.fields.at_ns - sent.at_ns) / 1e6,
      host_to_pool_ms: pool ? (pool.fields.at_ns - host.fields.at_ns) / 1e6 : null,
      pool_to_publication_ms: pool ? (capture.fields.at_ns - pool.fields.at_ns) / 1e6 : null,
      host_to_publication_ms: (capture.fields.at_ns - host.fields.at_ns) / 1e6,
      total_ms: ready.fields.duration_ns / 1e6,
    });
  }
  return { directory, observed_span_seconds: (end - start) / 1e9,
    presentation_gaps_over_50ms: gaps.length,
    presentation_gap_ms: distribution(gaps.map((gap) => gap.duration_ms)),
    gap_recurrence_ms: distribution(gaps.slice(1).map((gap, i) => (gap.start_ns - gaps[i].start_ns) / 1e6)),
    late_frames_over_50ms: delayed.length,
    late_frames_coincident_with_presentation_gap: delayed.filter((event) => gaps.some((gap) =>
      event.fields.ready_ns >= gap.start_ns && event.at_ns <= gap.end_ns + 10e6)).length,
    resizes_over_100ms: longResizes.length,
    long_resizes_overlapping_presentation_gap: longResizes.filter((event) => overlaps(event.at_ns - event.fields.duration_ns, event.at_ns)).length,
    agent_binary_scan_ms: distribution(scans.map((event) => (event.fields.end_ns - event.fields.start_ns) / 1e6)),
    paint_probe_outcomes: outcomes,
    capture_delivery_stalls: stalls.length,
    capture_delivery_stall_ms: distribution(stalls.map((stall) => stall.duration_ms)),
    frames_captured_during_stall: distribution(stalls.map((stall) => stall.captured_meanwhile)),
    scans_overlapping_presentation_gap: scans.filter((event) => overlaps(event.fields.start_ns, event.fields.end_ns)).length,
    scan_threads: [...new Set(scans.map((event) => `${event.fields.thread}:${event.fields.thread_name}`))],
    resize_send_to_host_ms: distribution(stageDurations.map((row) => row.send_to_host_ms)),
    resize_host_to_pool_ms: distribution(stageDurations.map((row) => row.host_to_pool_ms)),
    resize_pool_to_publication_ms: distribution(stageDurations.map((row) => row.pool_to_publication_ms)),
    resize_host_to_publication_ms: distribution(stageDurations.map((row) => row.host_to_publication_ms)),
    resize_stages: stageDurations,
    gaps,
  };
}

if (import.meta.main) {
  const root = resolve(process.argv[2]);
  const runs = readdirSync(root).filter((name) => /^r\d+$/.test(name)).map((name) => diagnose(join(root, name)));
  writeFileSync(join(root, "diagnosis.json"), JSON.stringify({ runs }, null, 2));
  console.log(JSON.stringify(runs.map(({ gaps, resize_stages, ...run }) => run), null, 2));
}
