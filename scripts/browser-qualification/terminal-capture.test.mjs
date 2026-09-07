import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { archiveBrowserShutdown, inspectBrowserEvents, inspectVisibility, sourceProvenance, verifyBrowserRuntime } from "./terminal-capture.mjs";

const hash = value => createHash("sha256").update(value).digest("hex");
const origin = 1e9;
const frame = {
  document: { owner: { workspace: "synthetic", session: "synthetic" }, browser: "synthetic", generation: 1 },
  pool_generation: 1, buffer: 0, frame_sequence: 1, callback_ns: origin - 3e6,
  ready_ns: origin - 2e6, intake_ns: origin - 1e6, capture_timestamp_us: (origin - 4e6) / 1000,
};
function browserEvents() {
  return [
    { event: "browser_viewport", at_ns: origin - 10e6, width_px: 1920, height_px: 1080, scale: 1 },
    { event: "browser_intake", at_ns: frame.intake_ns, ...frame },
    { event: "browser_paint", at_ns: origin - 0.9e6, ...frame, feedback_id: 1 },
    { event: "browser_presented", ...frame, feedback_id: 1, present_ns: origin - 0.8e6,
      native_present_ns: origin - 0.8e6, presentation_callback_ns: origin - 0.7e6,
      clock_id: 1, refresh_ns: 16666667, sequence: 900,
      calibration: { source_ns: origin - 0.7e6, mapped_ns: origin - 0.7e6, max_error_ns: 100 } },
  ];
}
const inspect = events => inspectBrowserEvents(events, origin, 3, 60, 1);

test("Browser intake and compositor presentation remain separate measurements", () => {
  const events = browserEvents();
  events.push({ event: "browser_intake", ...frame, frame_sequence: 2, buffer: 1 });
  const result = inspect(events);
  expect(result.errors).toEqual([]);
  expect(result.intake_frames).toBe(2);
  expect(result.presented_frames).toBe(1);
  expect(result.chromium_correlation).toBe("NOT_EVALUATED");
  expect(result.presentation_qualification).toBe("NOT_EVALUATED");
});

test("Browser intakes alone cannot satisfy native readiness", () => {
  expect(inspect(browserEvents().filter(event => event.event !== "browser_presented")).errors).toContain("Browser has no native presentation before terminal replay");
});

test("Browser feedback cannot be correlated across document generations", () => {
  const events = browserEvents();
  events.at(-1).document = { ...frame.document, generation: 2 };
  expect(inspect(events).errors).toContain("Browser feedback does not uniquely identify the painted frame");
});

test("Browser feedback preserves the original Chromium and CEF timestamps", () => {
  const events = browserEvents();
  events.at(-1).callback_ns++;
  expect(inspect(events).errors).toContain("Browser feedback timing differs from its accepted frame");
});

test("Browser calibration and actual viewport are checked", () => {
  const events = browserEvents();
  events[0].width_px = 1280;
  events.at(-1).calibration.mapped_ns++;
  const errors = inspect(events).errors;
  expect(errors).toContain("Browser physical viewport or scale differs from the M1 condition");
  expect(errors).toContain("Browser presentation clock or causal boundary is invalid");
});

function visibility() {
  return { schema_version: 1, observation: "native_compositor_surface_visibility", clock: "CLOCK_MONOTONIC", application_pid: 123,
    artifacts: [{ path: "compositor.jsonl", sha256: "a".repeat(64) }],
    intervals: [{ start_ns: origin, end_ns: origin + 3e9,
      terminal: { visible: true, occluded: false, x_px: 0, y_px: 0, width_px: 1920, height_px: 1080 },
      browser: { visible: true, occluded: false, x_px: 1920, y_px: 0, width_px: 1920, height_px: 1080 } }] };
}

test("qualification visibility evidence must bind PID and uninterrupted non-overlapping viewports", () => {
  expect(() => inspectVisibility(visibility(), 123, origin, 3)).not.toThrow();
  expect(() => inspectVisibility(visibility(), 456, origin, 3)).toThrow("application PID");
  const gap = visibility(); gap.intervals[0].start_ns++;
  expect(() => inspectVisibility(gap, 123, origin, 3)).toThrow("gap");
  const overlap = visibility(); overlap.intervals[0].browser.x_px--;
  expect(() => inspectVisibility(overlap, 123, origin, 3)).toThrow("overlap");
});

test("snapshots preserve their declared base and reject changed or uninventoried compiled sources", async () => {
  const directory = await mkdtemp(join(tmpdir(), "pf-snapshot-test-"));
  try {
    await mkdir(join(directory, "src-app"));
    const files = { "Cargo.toml": "synthetic manifest", "Cargo.lock": "synthetic lock", "src-app/main.rs": "fn main() {}" };
    for (const [name, content] of Object.entries(files)) await writeFile(join(directory, name), content);
    const receipt = { head: "b".repeat(40), files_sha256: Object.fromEntries(Object.entries(files).map(([name, content]) => [name, hash(content)])) };
    await writeFile(join(directory, "snapshot-provenance.json"), JSON.stringify(receipt));
    const provenance = await sourceProvenance(directory);
    expect(provenance.commit).toBe(receipt.head);
    expect(provenance.commit_provenance).toBe("snapshot_declared_base_plus_verified_inventory");
    expect(provenance.patch).toBeNull();
    expect(hash(provenance.snapshot)).toBe(hash(await readFile(join(directory, "snapshot-provenance.json"))));
    await writeFile(join(directory, "src-app/main.rs"), "fn different() {}");
    await expect(sourceProvenance(directory)).rejects.toThrow("differs from provenance");
    await writeFile(join(directory, "src-app/main.rs"), files["src-app/main.rs"]);
    await writeFile(join(directory, "src-app/new.rs"), "fn new() {}");
    await expect(sourceProvenance(directory)).rejects.toThrow("missing from snapshot provenance");
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("C refuses an implicit or missing host/runtime", async () => {
  await expect(verifyBrowserRuntime("/", undefined, undefined)).rejects.toThrow("explicit --host and --runtime");
});

test("graceful Browser shutdown archives trace and stderr only after native closure", async () => {
  const directory = await mkdtemp(join(tmpdir(), "pf-browser-stop-test-"));
  try {
    const data = join(directory, "data");
    const profile = join(data, "paneflow/browser/profiles/synthetic");
    const output = join(directory, "output");
    await mkdir(profile, { recursive: true });
    await mkdir(output);
    const trace = join(profile, "chromium-trace.json");
    const stderr = join(profile, "host.stderr");
    const payload = JSON.stringify({ traceEvents: [], synthetic_test_only: true });
    await writeFile(trace, payload);
    await writeFile(stderr, "synthetic host stderr\n");
    const events = [
      { event: "host_config", host_stderr: stderr },
      { event: "native", native: { native: "trace_completed", path: trace } },
      { event: "native", native: { native: "closed" } },
      { event: "host_stopped" },
    ];
    await expect(archiveBrowserShutdown(output, data, events.filter(event => event.event !== "host_stopped"))).rejects.toThrow("trace_completed, closed or host_stopped");
    const result = await archiveBrowserShutdown(output, data, events);
    expect(result.status).toBe("CLOSED_WITH_TRACE_ARCHIVED");
    expect(result.qualification).toBe("NOT_EVALUATED");
    expect(result.artifacts[0].sha256).toBe(hash(payload));
    expect(await readFile(join(output, "host.stderr"), "utf8")).toBe("synthetic host stderr\n");
    const outside = join(directory, "outside-trace.json");
    await writeFile(outside, payload);
    await rm(trace);
    await symlink(outside, trace);
    const otherOutput = join(directory, "outside-output");
    await mkdir(otherOutput);
    await expect(archiveBrowserShutdown(otherOutput, data, events)).rejects.toThrow("outside the repetition's private data directory");
  } finally { await rm(directory, { recursive: true, force: true }); }
});
