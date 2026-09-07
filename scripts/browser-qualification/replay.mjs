import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

export async function replayBundle(profile = "windowed") {
  if (!["windowed", "fullscreen"].includes(profile)) throw new Error("unknown terminal replay profile");
  const file = profile === "fullscreen" ? "replay-fullscreen.json" : "replay.json";
  const bytes = await readFile(new URL(`../../bench/browser/${file}`, import.meta.url));
  const protocol = JSON.parse(bytes);
  const events = [];
  const ticks = (protocol.warmup_seconds + protocol.duration_seconds) * 1e9 / protocol.tick_ns;
  for (let tick = 0; tick < ticks; tick++) {
    for (let terminal = 0; terminal < protocol.terminal_count; terminal++) {
      const marker = `pf:${terminal}:${tick}`;
      const output = [
        `${marker}:0123456789abcdef\r\n`,
        `${marker}:café 中文 हिन्दी\r\n`,
        `\x1b[38;5;${16 + tick % 216}m${marker}\x1b[0m\r\n`,
        `\x1b[2;2H${marker}\x1b[K`,
        `${marker}:scroll\r\n`.repeat(4),
      ][tick % protocol.families.length];
      events.push({ sequence: events.length, at_ns: tick * protocol.tick_ns, terminal, output_base64: Buffer.from(output).toString("base64") });
    }
    if (tick % protocol.input_every_ticks === 0) {
      events.push({ sequence: events.length, at_ns: tick * protocol.tick_ns, terminal: Math.floor(tick / protocol.focus_every_ticks) % protocol.terminal_count, input: `i${tick}\n` });
    }
  }
  const canonical = JSON.stringify({ protocol, events });
  return { schema_version: 1, sha256: createHash("sha256").update(canonical).digest("hex"), protocol, events };
}

export function validateCalibration(run) {
  const calibration = run.calibration;
  if (!calibration || calibration.clock !== run.clock || !Array.isArray(calibration.points) || calibration.points.length < 2) throw new Error("clock calibration with at least two points is required");
  if (!Number.isSafeInteger(calibration.max_error_ns) || calibration.max_error_ns < 0 || calibration.max_error_ns > run.uncertainty_ns) throw new Error("calibration error exceeds reported uncertainty");
  let previous = -1;
  for (const point of calibration.points) {
    if (!Number.isSafeInteger(point.source_ns) || !Number.isSafeInteger(point.mapped_ns) || point.source_ns <= previous || point.mapped_ns < 0) throw new Error("invalid calibration point");
    previous = point.source_ns;
  }
  if (run.configuration !== "B") {
    const replay = run.replay;
    if (!replay || replay.sha256 !== run.workload_sha256 || replay.expected_events !== replay.observed_events || !Number.isSafeInteger(replay.expected_events) || replay.expected_events < 1 || replay.divergent_events !== 0 || !Number.isSafeInteger(replay.max_delivery_error_ns) || replay.max_delivery_error_ns < 0 || replay.max_delivery_error_ns > 2_000_000) throw new Error("terminal replay is missing, divergent or outside delivery tolerance");
  }
}
