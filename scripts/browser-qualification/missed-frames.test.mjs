import { expect, test } from "bun:test";
import { countMissedFrames } from "./missed-frames.mjs";

function fixture(missing = []) {
  const presentations = Array.from({ length: 102 }, (_, index) => ({
    present_ns: index * 10_000_000, refresh_ns: 10_000_000, output_sequence: String(index),
    fresh: !missing.includes(index), content_id: String(index),
  }));
  return { presentations, start_ns: 10_000_000, end_ns: 1_010_000_000, refresh_hz: 100, uncertainty_ns: 0 };
}

test("counts the complete half-open fixed window", () => {
  const result = countMissedFrames(fixture());
  expect(result.status).toBe("SATISFIED");
  expect(result.expected_slots).toBe(100);
  expect(result.presented_fresh_slots).toBe(100);
});

test("exactly one percent is rejected, including a loss at either edge", () => {
  for (const missing of [1, 50, 100]) {
    const result = countMissedFrames(fixture([missing]));
    expect(result.status).toBe("EXCEEDED");
    expect(result.missed_percent).toBe(1);
  }
});

test("warmup and drain do not enter the measured numerator", () => {
  expect(countMissedFrames(fixture([0, 101])).missed_slots).toBe(0);
});

test("silence and resurrected content cannot shrink the denominator", () => {
  const input = fixture();
  input.presentations = [input.presentations[0], input.presentations.at(-1)];
  expect(countMissedFrames(input).missed_slots).toBe(100);
  const repeated = fixture();
  repeated.presentations.forEach(point => { point.content_id = "same"; });
  expect(countMissedFrames(repeated).missed_slots).toBe(100);
});

test("unproven phase, duplicate sequence and unstable refresh are not passes", () => {
  for (const mutate of [
    input => input.presentations.shift(),
    input => input.presentations.pop(),
    input => { input.presentations[20].output_sequence = "19"; },
    input => { input.presentations[20].present_ns += 2_000_000; },
  ]) {
    const input = fixture();
    mutate(input);
    if (input.presentations[0].present_ns === input.start_ns) input.presentations.shift();
    expect(countMissedFrames(input).status).toBe("NOT_EVALUATED");
  }
});

test("uncertain boundary slots are retained conservatively", () => {
  const input = fixture();
  input.uncertainty_ns = 1000;
  const result = countMissedFrames(input);
  expect(result.expected_slots).toBe(101);
  expect(result.missed_slots).toBe(2);
});

test("startup phase changes do not replace the phase bracketing measurement", () => {
  const input = fixture();
  input.presentations.unshift({ present_ns: 0, refresh_ns: 10_000_000, output_sequence: "0", fresh: false });
  input.presentations.slice(1).forEach(point => { point.output_sequence = String(Number(point.output_sequence) + 20); point.present_ns += 100_000_000; });
  input.start_ns += 100_000_000;
  input.end_ns += 100_000_000;
  expect(countMissedFrames(input).status).toBe("SATISFIED");
});

test("bounded native refresh jitter is recorded in the conservative phase margin", () => {
  const input = fixture();
  input.presentations[20].present_ns += 80_000;
  const result = countMissedFrames(input);
  expect(result.max_phase_error_ns).toBe(80_000);
  expect(result.phase_margin_ns).toBe(80_000);
  expect(result.expected_slots).toBeGreaterThanOrEqual(100);
});
