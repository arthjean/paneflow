import {test, expect} from "bun:test";
import {analyze, distribution} from "./dock-benchmark.mjs";
test("phase boundaries and pages do not mix frame intervals",()=>{
 const e=(page,t)=>({page,at_ns:t*1e6,event:"frame_received",fields:{callback_ns:(t-3)*1e6,ready_ns:(t-1)*1e6},dropped:0});
 const result=analyze([e("a",10),e("b",11),e("a",26),e("a",40)],[{name:"scroll",at_ns:0},{name:"resize",at_ns:40e6}]);
 expect(result[0].arrival_gap_ms.p95).toBe(16);
 expect(result[0].host_prepare_ms.p95).toBe(2);
 expect(result[1].arrival_gap_ms.count).toBe(0);
});
test("empty samples are unavailable, not zero",()=>{expect(distribution([]).p95).toBeNull();});
test("navigation duration uses its page and does not cross phases",()=>{
 const events=[{page:"a",event:"navigate",at_ns:1e6},{page:"b",event:"loaded",at_ns:3e6},{page:"a",event:"loaded",at_ns:5e6}];
 expect(analyze(events,[{name:"load",at_ns:0}])[0].load_event_after_submit_ms.p95).toBe(4);
 expect(analyze(events,[{name:"load",at_ns:0},{name:"steady",at_ns:4e6}])[1].load_event_after_submit_ms.count).toBe(0);
});
test("legacy mixed-clock recording is rejected",async()=>{
 const {validateRecording}=await import("./dock-benchmark.mjs");
 expect(()=>validateRecording([{event:"frame_received",at_ns:25e12,fields:{}}],[{name:"steady",at_ns:600e9}])).toThrow("horloge");
});
test("a synchronized recording with empty measured phases is rejected",async()=>{
 const {validateRecording}=await import("./dock-benchmark.mjs");
 expect(()=>validateRecording([],[{name:"steady",at_ns:600e9,clock:"CLOCK_MONOTONIC"}])).toThrow("aucune frame");
});
test("phase clock shares the system monotonic epoch across processes",async()=>{
 const {spawnSync}=await import("node:child_process");
 const {now}=await import("./dock-benchmark.mjs");
 const reference=()=>Number(spawnSync("python3",["-c","import time; print(time.monotonic_ns())"],{encoding:"utf8"}).stdout.trim());
 const before=reference();const measured=now();const after=reference();
 expect(measured).toBeGreaterThanOrEqual(before);
 expect(measured).toBeLessThanOrEqual(after);
});
test("resize completion timing is separate from frame arrival gaps",()=>{
 const rows=[{page:"a",event:"resize_ready",at_ns:100e6,fields:{duration_ns:80e6}}];
 const result=analyze(rows,[{name:"resize",at_ns:0}])[0];
 expect(result.resize_ready_ms.p95).toBe(80);
 expect(result.arrival_gap_ms.p95).toBeNull();
});
