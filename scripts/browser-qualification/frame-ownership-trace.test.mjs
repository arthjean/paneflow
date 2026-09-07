import { expect, test } from "bun:test";
import { analyzeFrameOwnership } from "./frame-ownership-trace.mjs";

const document = generation => ({ owner: { workspace: "work", session: "session" }, browser: "browser", generation });
const ready = (pid = 7, at_ns = 1) => ({ event: "host_ready", pid, at_ns });
const native = fields => ({ event: "native", native: { clock: "CLOCK_MONOTONIC", ...fields }, at_ns: (fields.at_ns ?? fields.ui_dispatch_ns) + 100 });
const ack = (generation = 1, sequence = 1, buffer = 0, pool_generation = 1) => ({ type: "release", document: document(generation), pool_generation, buffer, sequence });

function chain({ generation = 1, sequence = 1, buffer = 0, pool_generation = 1, offset = 0 } = {}) {
  const frame = ack(generation, sequence, buffer, pool_generation);
  const stage = (event, timestamp) => ({ event, clock: "CLOCK_MONOTONIC", stage_ns: offset + timestamp, at_ns: offset + timestamp + 99, acks: [frame] });
  return [
    native({ native: "frame_slot_occupied", ...frame, capture_counter: sequence, capture_timestamp_us: sequence * 1000, at_ns: offset + 100, occupied_slots: 1, total_slots: 3 }),
    stage("releases_dequeued", 200), stage("release_scene_replaced", 400), stage("release_gpu_completed", 450),
    { event: "release_ack_enqueued", clock: "CLOCK_MONOTONIC", enqueue_start_ns: offset + 500, enqueue_end_ns: offset + 510, at_ns: offset + 555, ack: frame },
    native({ native: "frame_ack_received", ...frame, received_ns: offset + 520, ui_dispatch_ns: offset + 600, occupied_slots_at_ui_dispatch: 1, total_slots: 3 }),
    native({ native: "frame_slot_released", ...frame, received_ns: offset + 520, at_ns: offset + 610, occupied_slots: 0, total_slots: 3 }),
  ];
}

const codes = report => report.issues.map(issue => issue.code);

test("joins each exact release stage and retains enqueue transport bounds without a verdict", () => {
  const report = analyzeFrameOwnership([ready(), ...chain()]);
  expect(report.issues).toEqual([]);
  expect(report.qualification).toBe("NOT_EVALUATED");
  expect(report.counts.complete_stage_chains).toBe(1);
  expect(report.frames[0]).toMatchObject({ host_pid: 7, host_epoch: 1, capture_counter: "1", identity: { buffer: "0", sequence: "1" },
    durations_ns: { dequeue_to_completion_scheduled: 200, completion_scheduled_to_gpu_callback: 50, gpu_callback_to_enqueue_start: 50, gpu_callback_to_enqueue_end: 60, enqueue_call: 10, host_received_to_ui: 80, host_ui_to_slot_free: 10, host_received_to_slot_free: 90, occupied_to_slot_free: 510 },
    ack_transport_bounds_ns: { min: 10, max: 20 } });
  expect(report.pools[0].timeline.map(row => row.occupied_slots)).toEqual([1, 1, 0]);
  expect(report.duration_summary.occupied_to_slot_free.p95_ns).toBe(510);
});

test("uses actual stage timestamps despite reversed log delivery of independent records", () => {
  const events = chain();
  const report = analyzeFrameOwnership([ready(), events[6], events[3], events[5], events[1], events[4], events[0], events[2]]);
  expect(report.issues).toEqual([]);
  expect(report.frames[0].durations_ns.occupied_to_slot_free).toBe(510);
  expect(report.pools[0].timeline.map(row => row.operation)).toEqual(["occupied", "ack_received", "released"]);
});

test("missing GPU callback remains null instead of using next frame or log receipt time", () => {
  const report = analyzeFrameOwnership([ready(), ...chain().filter(event => event.event !== "release_gpu_completed")]);
  expect(report.frames[0].missing_stages).toEqual(["gpu_completed_ns"]);
  expect(report.frames[0].durations_ns.completion_scheduled_to_gpu_callback).toBeNull();
  expect(report.duration_summary.completion_scheduled_to_gpu_callback).toMatchObject({ samples: 0, unavailable_or_negative: 1, p95_ns: null });
});

test("native ACK arrival during enqueue call is explicit valid concurrency", () => {
  const events = chain();
  events[5].native.received_ns = 505;
  const report = analyzeFrameOwnership([ready(), ...events]);
  expect(report.frames[0].ack_transport_bounds_ns).toEqual({ min: -5, max: 5 });
  expect(report.frames[0].races).toContainEqual({ code: "host_received_during_enqueue_call", ordering: "valid_concurrent_overlap" });
  expect(codes(report)).not.toContain("host_received_before_ack_enqueue");
});

test("reversed causality and duplicate stages are preserved rather than selecting one timestamp", () => {
  const events = chain();
  events[3].stage_ns = 350;
  events[5].native.received_ns = 490;
  events.push(structuredClone(events[1]));
  const report = analyzeFrameOwnership([ready(), ...events]);
  expect(codes(report)).toContain("reversed_stage_order");
  expect(codes(report)).toContain("host_received_before_ack_enqueue");
  expect(codes(report)).toContain("duplicate_stage");
  expect(report.frames[0].stages.dequeued_ns).toBeNull();
  expect(report.frames[0].durations_ns.completion_scheduled_to_gpu_callback).toBe(-50);
});

test("multiple hosts, document generations and pools remain separate identities", () => {
  const report = analyzeFrameOwnership([ready(), ...chain(), { event: "host_stopped", at_ns: 900 }, ready(8, 1001), ...chain({ generation: 2, pool_generation: 2, offset: 1100 })]);
  expect(report.issues).toEqual([]);
  expect(report.frames.map(frame => [frame.host_epoch, frame.host_pid, frame.identity.document.generation, frame.identity.pool_generation])).toEqual([[1, 7, "1", "1"], [2, 8, "2", "2"]]);
  expect(report.pools).toHaveLength(2);
});

test("PID reuse starts a distinct host epoch and cannot merge same frame identity", () => {
  const report = analyzeFrameOwnership([ready(), ...chain(), { event: "host_stopped", at_ns: 900 }, ready(7, 1001), ...chain({ offset: 1100 })]);
  expect(report.frames).toHaveLength(2);
  expect(report.frames.map(frame => frame.host_epoch)).toEqual([1, 2]);
  expect(codes(report)).toContain("ambiguous_or_missing_frame_host");
  expect(report.unresolved).toHaveLength(4);
  expect(report.frames[1].stages.gpu_completed_ns).toBeNull();
});

test("late GPU callback after host loss retains its old unique frame and exposes the race", () => {
  const events = chain();
  const report = analyzeFrameOwnership([ready(), ...events.slice(0, 3), { event: "host_lost", at_ns: 425 }, events[3], events[4]]);
  expect(report.frames[0].stages.gpu_completed_ns).toBe(450);
  expect(report.frames[0].races.some(race => race.code === "callbacks_after_host_end")).toBe(true);
  expect(report.frames[0].stages.slot_free_ns).toBeNull();
});

test("a delayed old native event is not attributed to a newly ready host", () => {
  const events = chain();
  const report = analyzeFrameOwnership([ready(), events[0], { event: "host_lost", at_ns: 700 }, ready(8, 1000), events[6]]);
  expect(codes(report)).toContain("unresolved_native_host");
  expect(report.unresolved).toHaveLength(1);
  expect(report.frames[0].stages.slot_free_ns).toBeNull();
});

test("starvation records keep exact capture IDs and actual occupied capacity", () => {
  const occupied = [0, 1, 2].map(buffer => native({ native: "frame_slot_occupied", ...ack(1, buffer + 1, buffer), capture_counter: String(buffer + 1), capture_timestamp_us: 1000 + buffer, at_ns: 100 + buffer * 10, occupied_slots: buffer + 1, total_slots: 3 }));
  const report = analyzeFrameOwnership([ready(), ...occupied, native({ native: "frame_slot_starved", document: document(1), pool_generation: 1, capture_counter: "18446744073709551615", capture_timestamp_us: 4999, at_ns: 150, callback_ns: 149, occupied_slots: 3, total_slots: 3 })]);
  expect(report.starved[0]).toMatchObject({ capture_counter: "18446744073709551615", capture_timestamp_us: "4999", host_pid: 7, occupied_slots: 3 });
  expect(report.pools[0].timeline.at(-1).known_occupied_slots).toEqual(["0", "1", "2"]);
  expect(report.counts.incomplete_stage_chains).toBe(3);
});

test("missing slot mutations expose unknown occupancy instead of fabricating frame owners", () => {
  const report = analyzeFrameOwnership([ready(), native({ native: "frame_slot_starved", document: document(1), pool_generation: 1, capture_counter: 10, capture_timestamp_us: 2000, callback_ns: 199, at_ns: 200, occupied_slots: 3, total_slots: 3 })]);
  expect(report.pools[0].timeline[0]).toMatchObject({ occupied_slots: 3, known_occupied_slots: [], unaccounted_occupied_slots: 3 });
  expect(report.frames).toHaveLength(0);
});

test("reusing an occupied buffer and changing pool counts expose corruption", () => {
  const events = chain();
  const duplicate = structuredClone(events[0]);
  duplicate.native.sequence = 2;
  duplicate.native.at_ns = 150;
  duplicate.native.occupied_slots = 1;
  const report = analyzeFrameOwnership([ready(), events[0], duplicate, ...events.slice(1)]);
  expect(codes(report)).toContain("buffer_occupied_without_release");
  expect(codes(report)).toContain("occupancy_count_gap");
  expect(codes(report)).toContain("release_identity_differs_from_occupied_frame");
});

test("equal-time slot mutations are explicitly unordered", () => {
  const events = chain();
  events[6].native.at_ns = 100;
  const report = analyzeFrameOwnership([ready(), ...events]);
  expect(codes(report)).toContain("equal_timestamp_occupancy_events");
  expect(report.pools[0].timeline[0].known_occupied_slots).toEqual([]);
});

test("bad clocks, unsafe numeric IDs and missing host provenance cannot create exact durations", () => {
  const events = chain();
  events[3].clock = "CLOCK_REALTIME";
  events[4].ack = { ...events[4].ack, sequence: Number.MAX_SAFE_INTEGER + 1 };
  const report = analyzeFrameOwnership([ready(), ...events]);
  expect(codes(report)).toContain("invalid_clock");
  expect(codes(report)).toContain("invalid_identity");
  expect(report.frames[0].stages.gpu_completed_ns).toBeNull();
  expect(codes(analyzeFrameOwnership(chain()))).toContain("unresolved_native_host");
});

test("pure JSONL parser requires a complete final line and rejects corrupt JSON", () => {
  const text = [ready(), ...chain()].map(event => JSON.stringify(event)).join("\n") + "\n";
  expect(analyzeFrameOwnership(text).counts.complete_stage_chains).toBe(1);
  expect(() => analyzeFrameOwnership(text.trimEnd())).toThrow("truncated");
  expect(() => analyzeFrameOwnership("broken\n")).toThrow("invalid ownership JSON");
});


test("an invalid ACK type and impossible occupied count do not become clean evidence", () => {
  const events = chain();
  events[1].acks = [{ ...events[1].acks[0], type: "unknown" }];
  events[0].native.occupied_slots = 0;
  const report = analyzeFrameOwnership([ready(), ...events]);
  expect(codes(report)).toContain("invalid_identity");
  expect(codes(report)).toContain("known_occupancy_exceeds_reported_count");
  expect(report.frames[0].stages.dequeued_ns).toBeNull();
});
