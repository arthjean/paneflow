import { expect, test } from "bun:test";
import { summarize } from "./windows-dock-benchmark.mjs";

function recording() {
  const phases = ["steady", "scroll", "resize"].map((name, i) => ({ name, at_ns: (i + 1) * 1e9, clock: "QPC" }));
  const events = phases.flatMap((phase) => Array.from({ length: 12 }, (_, i) => {
    const at_ns = phase.at_ns + 10e6 + i * 20e6;
    return { event: "frame_received", at_ns, page: "one", fields: { callback_ns: at_ns - 3e6, ready_ns: at_ns - 1e6 } };
  }));
  for (let i = 0; i < 12; i++) {
    events.push({ event: "wheel", at_ns: 2e9 + i * 20e6, fields: {} });
    events.push({ event: "resize_sent", at_ns: 3e9 + i * 20e6, fields: {} });
    events.push({ event: "resize_ready", at_ns: 3e9 + i * 20e6 + 10e6, fields: { duration_ns: 10e6 } });
  }
  return { events: events.sort((a, b) => a.at_ns - b.at_ns), phases };
}

test("reports the same host and transport boundaries as the Linux analyzer", () => {
  const { events, phases } = recording();
  const report = summarize(events, phases);
  expect(report[0].host_prepare_ms.p95).toBe(2);
  expect(report[0].ready_to_receive_ms.p95).toBe(1);
  expect(report[2].resize_ready_ms.p95).toBe(10);
});

test("rejects missing native scroll or resize input", () => {
  const { events, phases } = recording();
  expect(() => summarize(events.filter((event) => event.event !== "wheel"), phases)).toThrow("Wheel");
  expect(() => summarize(events.filter((event) => event.event !== "resize_ready"), phases)).toThrow("Resize");
});

test("rejects mixed clocks and lost recording events", () => {
  const { events, phases } = recording();
  expect(() => summarize(events, phases.map((phase) => ({ ...phase, clock: "CLOCK_MONOTONIC" })))).toThrow("QPC");
  expect(() => summarize([...events, { dropped: 1 }], phases)).toThrow("dropped");
});
