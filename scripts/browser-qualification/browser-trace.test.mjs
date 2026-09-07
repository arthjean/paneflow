import { expect, test } from "bun:test";
import { extractBrowserPresentation, parseWaylandPresentation } from "./browser-trace.mjs";

const origin = 1_000_000_000_000;
const options = { origin_ns: origin, gpu_pid: 42 };

function nativeFrame(timestamp, id = 62, sequence = 1, flags = 7) {
  const seconds = Math.floor(timestamp / 1e9);
  return [
    `[1.000]  -> wp_presentation#33.feedback(wl_surface#46, new id wp_presentation_feedback#${id})`,
    `[1.001] wp_presentation_feedback#${id}.sync_output(wl_output#8)`,
    `[1.002] wp_presentation_feedback#${id}.presented(0, ${seconds}, ${timestamp % 1e9}, 16666667, 0, ${sequence}, ${flags})`,
  ];
}

function syntheticProof({ idle = false } = {}) {
  const trace = {
    metadata: { "clock-domain": "LINUX_CLOCK_MONOTONIC", trace_processor_stats: { json_parser_failure: 0, traced_buf: [{ bytes_overwritten: 0, trace_writer_packet_loss: 0 }] } },
    traceEvents: [{ ph: "M", name: "process_name", pid: 42, tid: 0, ts: 0, args: { name: "GPU Process" } }],
  };
  const lines = ["[1.000] wp_presentation#33.clock_id(1)"];
  const times = idle ? [1, 2] : [1, 2, 11, 12, 69];
  for (const [index, seconds] of times.entries()) {
    const start = origin + seconds * 1e9;
    const end = start + 20_000_000;
    for (const [ph, time] of [["b", start], ["e", end]]) {
      trace.traceEvents.push({ name: "Graphics.Pipeline.DrawAndSwap", ph, ts: time / 1000, pid: 42, tid: 43, id2: { local: "0x2e" }, args: {} });
    }
    trace.traceEvents.push({ name: "Display::FrameDisplayed", ph: "I", ts: end / 1000, pid: 42, tid: 43, args: {} });
    lines.push(...nativeFrame(end, 62, index + 1));
  }
  return { trace, lines, stderr: () => `${lines.join("\n")}\n` };
}

test("synthetic proof joins exact compositor timestamps and safely reuses completed async IDs", () => {
  const proof = syntheticProof();
  const result = extractBrowserPresentation(proof.trace, proof.stderr(), options);
  expect(result.qualification).toBe("NOT_EVALUATED");
  expect(result.kind).toBe("browser_draw_to_present");
  expect(result.samples.map((sample) => sample.draw_ns)).toEqual([11e9, 12e9, 69e9]);
  expect(result.samples.map((sample) => sample.frame_generation)).toEqual([3, 4, 5]);
  expect(result.samples.map((sample) => sample.feedback_generation)).toEqual([3, 4, 5]);
  expect(result.samples.every((sample) => sample.present_ns - sample.draw_ns === 20_000_000)).toBe(true);
  expect(result.calibration.max_error_ns).toBe(1000);
  expect(result.calibration.points).toHaveLength(5);
});

test("synthetic idle fixture returns no fabricated post-warmup latency samples", () => {
  const proof = syntheticProof({ idle: true });
  const result = extractBrowserPresentation(proof.trace, proof.stderr(), options);
  expect(result.samples).toEqual([]);
  expect(result.diagnostics.idle_window).toBe(true);
  expect(result.calibration.points).toHaveLength(2);
});

test("missing native feedback rejects a completed measured frame", () => {
  const proof = syntheticProof();
  proof.lines.splice(7, 3);
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("missing or ambiguous native feedback");
});

test("one microsecond mismatch cannot become a nearest-event match", () => {
  const proof = syntheticProof();
  proof.lines[9] = proof.lines[9].replace(", 20000000,", ", 20001000,");
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("missing or ambiguous native feedback");
});

test("sub-microsecond endpoint precision is retained with an explicit quantization bound", () => {
  const proof = syntheticProof();
  proof.lines[9] = proof.lines[9].replace(", 20000000,", ", 20000999,");
  const result = extractBrowserPresentation(proof.trace, proof.stderr(), options);
  expect(result.samples[0].present_ns).toBe(11_020_000_999);
  expect(result.calibration.points[2].mapped_ns - result.calibration.points[2].source_ns).toBe(999);
});

test.each([0, 1, 3, 5, 6])("hardware presentation flags %i are insufficient", (flags) => {
  const proof = syntheticProof();
  proof.lines[9] = proof.lines[9].replace(", 7)", `, ${flags})`);
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("lacks VSYNC");
});

test("duplicate outstanding Chromium frame ID rejects instead of guessing a queue", () => {
  const proof = syntheticProof();
  const start = structuredClone(proof.trace.traceEvents.find((event) => event.ph === "b"));
  start.ts += 1;
  proof.trace.traceEvents.push(start);
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("duplicate active frame identity");
});

test("duplicate native presentation timestamp rejects ambiguous frame ownership", () => {
  const proof = syntheticProof();
  proof.lines.splice(10, 0, ...nativeFrame(origin + 11_020_000_000, 70, 99));
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("ambiguous native feedback");
});

test("reusing a live feedback object rejects a corrupt protocol log", () => {
  const proof = syntheticProof();
  proof.lines.splice(2, 0, proof.lines[1]);
  expect(() => parseWaylandPresentation(proof.stderr())).toThrow("ambiguous live feedback object");
});

test("discarded native frames retain their bounded interval without inventing a rate", () => {
  const proof = syntheticProof();
  proof.lines.splice(10, 0,
    "[1.003]  -> wp_presentation#33.feedback(wl_surface#46, new id wp_presentation_feedback#70)",
    "[1.004] wp_presentation_feedback#70.discarded()");
  const result = extractBrowserPresentation(proof.trace, proof.stderr(), options);
  expect(result.discarded_bounds).toHaveLength(1);
  expect(result.diagnostics.discarded_native_frames).toBe(1);
  expect(result.missed_frames.status).toBe("NOT_EVALUATED");
});

test("unmatched native presentations inside the window expose missing Chromium events", () => {
  const proof = syntheticProof();
  proof.lines.splice(10, 0, ...nativeFrame(origin + 11_500_000_000, 70, 99));
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("no Chromium frame association");
});

test("FrameDisplayed is required in addition to an async end and native feedback", () => {
  const proof = syntheticProof();
  proof.trace.traceEvents = proof.trace.traceEvents.filter((event) => !(event.name === "Display::FrameDisplayed" && event.ts === (origin + 11_020_000_000) / 1000));
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("FrameDisplayed");
});

test("unfinished draws overlapping the observation window are not called idle", () => {
  const proof = syntheticProof({ idle: true });
  proof.trace.traceEvents.push({ name: "Graphics.Pipeline.DrawAndSwap", ph: "b", ts: (origin + 3e9) / 1000, pid: 42, tid: 43, id2: { local: "0x42" } });
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("unfinished frame overlaps capture");
});

test("GPU PID must match both trace metadata and the observed native process", () => {
  const proof = syntheticProof();
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), { ...options, gpu_pid: 999 })).toThrow("differs from observed sandbox process");
  proof.trace.traceEvents.push({ ph: "M", name: "process_name", pid: 999, args: { name: "GPU Process" } });
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("absent or ambiguous");
});

test("trace overwrite counters invalidate an apparently complete frame subset", () => {
  const proof = syntheticProof();
  proof.trace.metadata.trace_processor_stats.traced_buf[0].bytes_overwritten = 64;
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("trace loss or corruption");
});

test("truncation and malformed presentation records reject instead of disappearing", () => {
  const proof = syntheticProof();
  expect(() => parseWaylandPresentation(proof.stderr().trimEnd())).toThrow("truncated Wayland log");
  proof.lines[9] = proof.lines[9].replace("presented(", "presented(broken,");
  expect(() => parseWaylandPresentation(proof.stderr())).toThrow("corrupt native timestamp");
});

test("both native and Chromium clocks must declare monotonic timestamps", () => {
  const proof = syntheticProof();
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr().replace("clock_id(1)", "clock_id(0)"), options)).toThrow("CLOCK_MONOTONIC");
  proof.trace.metadata["clock-domain"] = "unknown";
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("LINUX_CLOCK_MONOTONIC");
});

test("output sequence reuse or monitor changes invalidate paired display conditions", () => {
  const proof = syntheticProof();
  proof.lines[12] = proof.lines[12].replace(", 0, 4, 7)", ", 0, 3, 7)");
  expect(() => extractBrowserPresentation(proof.trace, proof.stderr(), options)).toThrow("output sequence is duplicated or reversed");
  const moved = syntheticProof();
  moved.lines[11] = moved.lines[11].replace("wl_output#8", "wl_output#9");
  expect(() => extractBrowserPresentation(moved.trace, moved.stderr(), options)).toThrow("output changed during capture");
});

function connectionLog(log, pid = 99, connection = "1") {
  return log.replace(/^(\[\s*[\d.]+\]) /gm, `$1 [pid=${pid} connection=${connection}] `);
}

function outputMetadata({ id = 8, name = "DP-4", width = 1920, refresh = 60000 } = {}) {
  return [
    `[0.001] -> wl_registry#2.bind(6, "wl_output", 4, new id [unknown]#${id})`,
    `[0.002] wl_output#${id}.name("${name}")`,
    `[0.003] wl_output#${id}.mode(1, ${width}, 1080, ${refresh})`,
    `[0.004] wl_output#${id}.scale(1)`,
    `[0.005] wl_output#${id}.done()`,
  ];
}
const fullscreenOptions = { ...options, fullscreen: true, expected_output: "DP-4", refresh_actual_hz: 60, host_pid: 99 };

test("fullscreen proof resolves SyncOutput to completed output metadata and object generation", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  const result = extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions);
  expect(result.display_evidence.output).toMatchObject({ wl_output_id: 8, generation: 1, name: "DP-4", width_px: 1920, height_px: 1080, scale: 1, refresh_millihz: 60000, registry_global_name: 6 });
  expect(result.samples.every(sample => sample.output_metadata[0].generation === 1)).toBe(true);
});

test.each([
  ["no output binding", lines => lines.splice(0, 5), "metadata is missing or incomplete"],
  ["no completed metadata", lines => lines.splice(4, 1), "metadata is missing or incomplete"],
  ["wrong output", lines => { lines[1] = lines[1].replace("DP-4", "DP-3"); }, "expected output"],
  ["wrong physical size", lines => { lines[2] = lines[2].replace("1920", "2560"); }, "mode or scale"],
  ["wrong actual frequency", lines => { lines[2] = lines[2].replace("60000", "59950"); }, "mode refresh"],
  ["not current mode", lines => { lines[2] = lines[2].replace("mode(1,", "mode(2,"); }, "metadata is missing or incomplete"],
  ["unreleased output ID reused", lines => lines.splice(5, 0, ...outputMetadata()), "ambiguous live wl_output"],
  ["another queue is not another proven connection", lines => lines.splice(5, 0, ...outputMetadata({ name: "DP-3" }).map(line => line.replace("] ", "] {Default Queue} "))), "ambiguous live wl_output"],
])("fullscreen fails closed: %s", (_name, corrupt, message) => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  corrupt(fixture.lines);
  expect(() => extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions)).toThrow(message);
});

test("released wl_output ID can be reused with a new generation before observation", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata({ name: "DP-3" }), "[0.006] -> wl_output#8.release()", "[0.007] {Display Queue} wl_display#1.delete_id(8)", ...outputMetadata());
  expect(extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions).display_evidence.output.generation).toBe(2);
});

test("wl_output ID reuse during observation cannot preserve the old output identity", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  fixture.lines.splice(15, 0, "[0.006] -> wl_output#8.release()", "[0.007] {Display Queue} wl_display#1.delete_id(8)", ...outputMetadata());
  expect(() => extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions)).toThrow("output identity changed");
});

test("idle fullscreen proof uses actual warmup native output and invents no samples", () => {
  const fixture = syntheticProof({ idle: true });
  fixture.lines.unshift(...outputMetadata());
  const result = extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions);
  expect(result.samples).toHaveLength(0);
  expect(result.display_evidence.output.name).toBe("DP-4");
});


test("distinct PID and connection namespaces safely reuse simultaneous Wayland object IDs", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  const other = outputMetadata({ name: "DP-3" }).join("\n") + "\n";
  const log = connectionLog(other, 42, "1") + connectionLog(other, 99, "2") + connectionLog(fixture.stderr());
  const result = extractBrowserPresentation(fixture.trace, log, fullscreenOptions);
  expect(result.display_evidence).toMatchObject({ wayland_pid: 99, connection_id: "1", output: { name: "DP-4", wayland_pid: 99, connection_id: "1" } });
});

test("output metadata from another connection never repairs a missing SyncOutput join", () => {
  const fixture = syntheticProof();
  const log = connectionLog(outputMetadata().join("\n") + "\n", 99, "2") + connectionLog(fixture.stderr());
  expect(() => extractBrowserPresentation(fixture.trace, log, fullscreenOptions)).toThrow("metadata is missing or incomplete");
});

test("native feedback can be carried by either proven host or proven GPU", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  expect(extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr(), 42), fullscreenOptions).display_evidence.wayland_pid).toBe(42);
  expect(() => extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr(), 123), fullscreenOptions)).toThrow("not an observed host or GPU");
});

test("fullscreen refuses untagged and mixed client logs", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  expect(() => extractBrowserPresentation(fixture.trace, fixture.stderr(), fullscreenOptions)).toThrow("missing Wayland PID and connection");
  const mixed = connectionLog(fixture.stderr()) + outputMetadata({ id: 9 }).join("\n") + "\n";
  expect(() => extractBrowserPresentation(fixture.trace, mixed, fullscreenOptions)).toThrow("untagged or corrupt");
});

test("two feedback connections cannot become one surface by reusing the same numeric ID", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  const log = connectionLog(fixture.stderr(), 99, "1") + connectionLog(fixture.stderr(), 99, "2");
  expect(() => extractBrowserPresentation(fixture.trace, log, fullscreenOptions)).toThrow("feedback connection is absent or ambiguous");
});

test("connection identity preserves full uint64 decimal strings", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata());
  const result = extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr(), 99, "18446744073709551615"), fullscreenOptions);
  expect(result.display_evidence.connection_id).toBe("18446744073709551615");
  expect(() => extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr(), 99, "18446744073709551616"), fullscreenOptions)).toThrow("exceeds uint64");
});


test("output generation reuse requires compositor delete_id after release", () => {
  const fixture = syntheticProof();
  fixture.lines.unshift(...outputMetadata(), "[0.006] -> wl_output#8.release()", ...outputMetadata());
  expect(() => extractBrowserPresentation(fixture.trace, connectionLog(fixture.stderr()), fullscreenOptions)).toThrow("before delete_id confirmation");
});
