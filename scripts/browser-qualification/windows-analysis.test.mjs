import test from "node:test";
import assert from "node:assert/strict";
import { correlate, foregroundOwnership } from "./windows-analysis.mjs";

const metadata = { pid: 42, origin_ns: 0, warmup_seconds: 0, duration_seconds: 1, qpc_frequency: 10000000 };
const stage = { event: "renderer_stage", fields: { stage: "dxgi_present_call", start_ns: 9900, end_ns: 11000, width: 1920, height: 1080, outcome: "ok" } };
const csv = "ProcessID,TimeInQPC,MsUntilDisplayed,MsGPUBusy,SwapChainAddress\n42,100,2,0.2,0x42\n";

test("presentation is ETW display time, never callback time", () => {
  const events = [{ event: "browser_scene", at_ns: 8000, fields: { page: "page", pool_generation: 1, sequence: 1, callback_ns: 5000 } }, stage];
  const result = correlate(events, csv, metadata);
  assert.equal(result.unmatched, 0);
  assert.equal(result.samples.find((sample) => sample.kind === "browser").latency_ns, 2005000);
});

test("an unmatched or dropped present cannot prove display", () => {
  assert.equal(correlate([], csv, metadata).unmatched, 1);
  const result = correlate([stage], csv.replace(",2,", ",NA,"), metadata);
  assert.equal(result.dropped, 1);
  assert.equal(result.samples.length, 0);
});

test("a reused browser frame is counted only once", () => {
  const frame = { event: "browser_scene", at_ns: 8000, fields: { page: "page", pool_generation: 1, sequence: 1, callback_ns: 5000 } };
  const events = [frame, stage, { ...frame, at_ns: 15000 }, { ...stage, fields: { ...stage.fields, start_ns: 19000, end_ns: 21000 } }];
  const result = correlate(events, csv + "42,200,2,0.2,0x42\n", metadata);
  assert.equal(result.samples.filter((sample) => sample.kind === "browser").length, 1);
});

test("a capture that lost the foreground cannot prove latency", () => {
  const owned = [{ foreground_owned: true }, { foreground_owned: true }];
  assert.deepEqual(foregroundOwnership(owned), { samples: 2, owned: 2, ratio: 1 });
  const lost = [{ foreground_owned: true }, { foreground_owned: false }, { foreground_owned: false }];
  assert.deepEqual(foregroundOwnership(lost), { samples: 3, owned: 1, ratio: 1 / 3 });
});

test("a capture recorded before foreground tracking claims nothing", () => {
  assert.deepEqual(foregroundOwnership([{}, {}]), { samples: 2, owned: null, ratio: null });
  assert.deepEqual(foregroundOwnership([{ foreground_owned: true }, {}]), { samples: 2, owned: null, ratio: null });
  assert.deepEqual(foregroundOwnership([]), { samples: 0, owned: null, ratio: null });
});
