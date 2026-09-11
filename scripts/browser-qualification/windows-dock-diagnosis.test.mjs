import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { diagnose } from "./windows-dock-diagnosis.mjs";

test("separates UI-correlated frame delays from independent resize delays", () => {
  const directory = mkdtempSync(join(tmpdir(), "paneflow-dock-diagnosis-"));
  const save = (name, value) => writeFileSync(join(directory, name), JSON.stringify(value));
  const lines = (name, rows) => writeFileSync(join(directory, name), rows.map(JSON.stringify).join("\n") + "\n");
  try {
    save("phases.json", [{ name: "steady", at_ns: 1e9 }, { name: "end_resize", at_ns: 4e9 }]);
    lines("application.jsonl", [
      { event: "renderer_stage", at_ns: 1e9, fields: { stage: "dxgi_present_call" } },
      { event: "agent_binary_scan", fields: { start_ns: 1.01e9, end_ns: 1.13e9, thread: "one", thread_name: "main" } },
      { event: "renderer_stage", at_ns: 1.14e9, fields: { stage: "dxgi_present_call" } },
    ]);
    const probe = (at_ns, expected, counter, outcome) => ({ event: "paint_probe",
      at_ns, fields: { at_ns, expected, capture_counter: counter, outcome, coded: [900, 500] } });
    lines("events.jsonl", [
      { event: "frame_received", at_ns: 1.145e9, fields: { ready_ns: 1.02e9 } },
      { event: "resize_ready", at_ns: 2e9, fields: { duration_ns: 200e6 } },
      probe(2.0e9, [880, 500], 10, "stale_geometry"),
      probe(2.2e9, [880, 500], 21, "stale_geometry"),
      probe(2.21e9, [880, 500], 22, "published"),
      probe(3.0e9, [860, 500], 40, "stale_geometry"),
    ]);
    const result = diagnose(directory);
    expect(result.presentation_gaps_over_50ms).toBe(1);
    expect(result.late_frames_coincident_with_presentation_gap).toBe(1);
    expect(result.agent_binary_scan_ms.p95).toBe(120);
    expect(result.scans_overlapping_presentation_gap).toBe(1);
    expect(result.resizes_over_100ms).toBe(1);
    expect(result.long_resizes_overlapping_presentation_gap).toBe(0);
    expect(result.paint_probe_outcomes).toEqual({ stale_geometry: 3, published: 1 });
    expect(result.capture_delivery_stalls).toBe(1);
    expect(result.capture_delivery_stall_ms.p50).toBe(200);
    expect(result.frames_captured_during_stall.p50).toBe(11);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
