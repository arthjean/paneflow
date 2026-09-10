import test from "node:test";
import assert from "node:assert/strict";
import { compare, summarize } from "./windows-m1-compare.mjs";

const capture = (configuration, scenario = "animation") => ({
  configuration,
  scenario,
  binary_sha256: "binary",
  host_sha256: "host",
  client_sha256: "client",
  runtime_sha256: "runtime",
  manifest_sha256: "manifest",
  gpui_manifest_sha256: "gpui",
  paneflow_commit: "commit",
});

const analysis = (browser, terminal) => ({
  protocol_duration_matches: true,
  matched_presents: 600,
  unmatched_presents: 0,
  dropped_presents: 0,
  browser_callback_to_present_ns: { count: 100, p50: browser, p95: browser, p99: browser, max: browser },
  terminal_input_to_present_ns: { count: 100, p50: terminal, p95: terminal, p99: terminal, max: terminal },
  private_working_set_bytes: { count: 60, p50: 100, p95: 100, p99: 100, max: 100 },
  handles: { count: 60, p50: 10, p95: 10, p99: 10, max: 10 },
});

const runs = (values, configuration, scenario) =>
  values.map((value, index) => ({
    directory: `${configuration}-r${index + 1}`,
    capture: capture(configuration, scenario),
    analysis: analysis(value.browser ?? null, value.terminal ?? null),
  }));

test("a configuration reports the median repetition next to its worst repetition", () => {
  const summary = summarize(runs([{ browser: 10 }, { browser: 30 }, { browser: 20 }], "B"));
  assert.equal(summary.repetitions, 3);
  assert.equal(summary.browser_callback_to_present_ns.p95.median, 20);
  assert.equal(summary.browser_callback_to_present_ns.p95.worst, 30);
  assert.equal(summary.browser_callback_to_present_ns.samples, 300);
});

test("the campaign stays incomplete until the three configurations carry five repetitions", () => {
  const partial = compare([...runs([{ browser: 10 }], "B"), ...runs([{ terminal: 10 }], "A")]);
  assert.equal(partial.status, "INCOMPLETE_M1_CAMPAIGN");
  assert.equal(partial.protocol.complete, false);
  const five = [1, 2, 3, 4, 5];
  const complete = compare([
    ...runs(five.map(() => ({ terminal: 10 })), "A"),
    ...runs(five.map(() => ({ browser: 4 })), "B"),
    ...runs(five.map(() => ({ browser: 6, terminal: 11 })), "C"),
  ]);
  assert.equal(complete.status, "OBSERVED_NOT_BUDGET_CERTIFIED");
  assert.equal(complete.protocol.complete, true);
  assert.equal(complete.comparison.integrated_browser_overhead_ns.p95, 2);
  assert.equal(complete.comparison.terminal_input_overhead_ns.p95, 1);
});

test("captures from different artifacts are refused instead of averaged", () => {
  const mixed = [...runs([{ browser: 10 }], "B"), ...runs([{ browser: 10 }], "C")];
  mixed[1].capture.binary_sha256 = "other";
  assert.throws(() => compare(mixed), /different application or runtime artifacts/);
});
