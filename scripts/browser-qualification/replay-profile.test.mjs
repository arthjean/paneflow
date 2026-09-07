import { expect, test } from "bun:test";
import { replayBundle } from "./replay.mjs";

test("fullscreen has an explicit paired geometry and distinct workload identity", async () => {
  const windowed = await replayBundle();
  const fullscreen = await replayBundle("fullscreen");
  expect(fullscreen.protocol.terminal_geometry).toEqual([70, 71, 70, 71].map(columns => ({ columns, rows: 18 })));
  expect(fullscreen.events).toEqual(windowed.events);
  expect(fullscreen.sha256).not.toBe(windowed.sha256);
  expect((await replayBundle("fullscreen")).sha256).toBe(fullscreen.sha256);
  await expect(replayBundle("automatic")).rejects.toThrow("unknown terminal replay profile");
});
