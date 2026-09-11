import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, mkdirSync, readdirSync, existsSync } from "node:fs";
import { resolve, join } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { createInterface } from "node:readline/promises";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "../..");
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
export function now() {
  const sample = spawnSync("python3", ["-c", "import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC))"], {encoding:"utf8", timeout:5000});
  const value = Number(sample.stdout?.trim());
  if(sample.status!==0 || !Number.isSafeInteger(value) || value<=0)throw new Error("Lecture CLOCK_MONOTONIC impossible.");
  return value;
}
export const fixture = `<!doctype html><meta charset="utf-8"><title>PaneFlow benchmark</title><style>body{margin:0;background:#181a20;color:#eee;font:16px sans-serif}header{position:sticky;top:0;background:#282c38;padding:16px;z-index:1}i{display:block;width:30px;height:8px;background:#68e;animation:move 2s linear infinite alternate}@keyframes move{to{transform:translateX(300px)}}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;padding:16px}article{background:#292d36;padding:18px;border-radius:8px}</style><header>PaneFlow : chargement, scroll, resize<i></i></header><main>${Array.from({length:1000},(_,i)=>`<article><h3>Carte ${i}</h3><p>Contenu local fixe pour mesurer la fluidité.</p></article>`).join("")}</main>`;

export function distribution(values) {
  const sorted = values.filter(Number.isFinite).sort((a,b)=>a-b);
  const percentile = (p) => sorted.length ? sorted[Math.ceil(p*sorted.length)-1] : null;
  return {count:sorted.length,p50:percentile(.5),p95:percentile(.95),p99:percentile(.99),max:sorted.at(-1)??null};
}

export function analyze(events, phases) {
  return phases.map((phase,i)=>{
    const rows = events.filter(e=>e.at_ns>=phase.at_ns && (i+1===phases.length || e.at_ns<phases[i+1].at_ns));
    const frames = rows.filter(e=>e.event==="frame_received");
    const gaps=[]; const previous=new Map();
    for(const f of frames){if(previous.has(f.page))gaps.push((f.at_ns-previous.get(f.page))/1e6);previous.set(f.page,f.at_ns);}
    const pending=new Map(); const loads=[];
    for(const row of rows){if(row.event==="navigate")pending.set(row.page,row.at_ns);if(row.event==="loaded"&&pending.has(row.page)){loads.push((row.at_ns-pending.get(row.page))/1e6);pending.delete(row.page);}}
    return {resize_ready_ms:distribution(rows.filter(e=>e.event==="resize_ready").map(e=>e.fields.duration_ns/1e6)),load_event_after_submit_ms:distribution(loads),phase:phase.name,frames:frames.length,host_prepare_ms:distribution(frames.map(e=>(e.fields.ready_ns-e.fields.callback_ns)/1e6)),ready_to_receive_ms:distribution(frames.map(e=>(e.at_ns-e.fields.ready_ns)/1e6)),arrival_gap_ms:distribution(gaps),pool_import_ms:distribution(rows.filter(e=>e.event==="pool_import").map(e=>e.fields.duration_ns/1e6)),resizes:rows.filter(e=>e.event==="resize_sent").length,wheels:rows.filter(e=>e.event==="wheel").length,recording_dropped:Math.max(0,...rows.map(e=>e.dropped??0))};
  });
}

export function validateRecording(events, phases) {
  if(!phases.length || phases.some(p=>p.clock!=="CLOCK_MONOTONIC"))throw new Error("Collecte invalide : horloge des phases non synchronisée. Refaire avec le collecteur corrigé.");
  if(phases.some((p,i)=>!Number.isSafeInteger(p.at_ns)||(i>0&&p.at_ns<=phases[i-1].at_ns)))throw new Error("Horodatages de phases invalides.");
  if(events.some(e=>e.dropped>0))throw new Error("Collecte invalide : événements perdus.");
  const result=analyze(events,phases);
  for(const name of ["steady","scroll","resize"]){
    if(!result.some(p=>p.phase===name&&p.frames>0))throw new Error(`Collecte invalide : aucune frame pendant ${name}.`);
  }
  return result;
}

function report(directory) {
  const lines=readFileSync(join(directory,"events.jsonl"),"utf8").trim().split("\n").filter(Boolean);
  const events=lines.map(line=>JSON.parse(line));
  if(!events.length)throw new Error("Aucun événement : collecte invalide.");
  const phases=JSON.parse(readFileSync(join(directory,"phases.json")));
  const result={schema:1,limitations:["Arrivée dans GPUI, pas présentation physique écran.","Aucun verdict NFR ni causalité entrée-vers-image.","Comparer les phases de même durée, taille, écran et cadence."],phases:validateRecording(events,phases),capture_rate_changes:events.filter(e=>e.event==="capture_frame_rate").map(e=>({page:e.page,...e.fields}))};
  writeFileSync(join(directory,"summary.json"),JSON.stringify(result,null,2));
  console.table(result.phases.map(p=>({phase:p.phase,frames:p.frames,host_p95:p.host_prepare_ms.p95,transport_p95:p.ready_to_receive_ms.p95,interval_p95:p.arrival_gap_ms.p95,import_p95:p.pool_import_ms.p95,resize_ready_p95:p.resize_ready_ms.p95,dropped:p.recording_dropped})));
  return result;
}

function compare(before, after) {
  const metadata=p=>JSON.parse(readFileSync(join(p,"metadata.json")));
  const a=metadata(before),b=metadata(after);
  for(const key of ["platform","arch","session","display","fixture_sha256","frame_rate"]) {
    if(a[key]!==b[key])throw new Error(`Comparaison refusée : ${key} diffère.`);
  }
  const left=report(before),right=report(after);
  for(const report of [left,right]){
    for(const name of ["steady","scroll","resize"]){
      if(!report.phases.some(p=>p.phase===name&&p.frames>0))throw new Error(`Collecte incomplète : ${name} absent ou sans frames.`);
    }
    if(report.phases.some(p=>p.recording_dropped))throw new Error("Collecte incomplète : événements perdus.");
  }
  const rows=[];
  for(const phase of left.phases.filter(p=>["load","steady","scroll","resize"].includes(p.phase))){
    const other=right.phases.find(p=>p.phase===phase.phase);
    if(!other)throw new Error("Phase manquante");
    if(phase.recording_dropped||other.recording_dropped)throw new Error("Collecte incomplète : événements perdus.");
    for(const metric of ["host_prepare_ms","ready_to_receive_ms","arrival_gap_ms","pool_import_ms","load_event_after_submit_ms","resize_ready_ms"]){
      const old=phase[metric].p95,next=other[metric].p95;
      rows.push({phase:phase.phase,metric,before:old,after:next,delta_ms:old!==null&&next!==null?next-old:null});
    }
  }
  console.table(rows);
  console.log("Écarts descriptifs : répéter trois fois avant de conclure à une amélioration ou régression.");
}

async function record(label, diagnostic = false) {
  if(!/^[a-zA-Z0-9_-]+$/.test(label))throw new Error("Label attendu : lettres, chiffres, tiret.");
  now();
  const directory=join(root,"bench/browser/dock",`${new Date().toISOString().replaceAll(":","-")}-${label}`);
  mkdirSync(directory,{recursive:true});
  const traceDirectory=diagnostic?join(directory,"chromium"):undefined;
  if(traceDirectory)mkdirSync(traceDirectory);
  const binary=join(root,"target/release/paneflow");
  const host=join(root,"target/release/paneflow-browser-host");
  const manifest=hash(readFileSync(join(root,"native/browser/manifest.toml")));
  const base=join(root,"native/browser/prebuilt",`${process.arch==="arm64"?"aarch64":"x86_64"}-unknown-linux-gnu`);
  const runtime=process.env.PANEFLOW_CEF_ROOT || readdirSync(base).map(n=>join(base,n)).find(p=>existsSync(join(p,"verified-manifest.sha256"))&&readFileSync(join(p,"verified-manifest.sha256"),"utf8").trim()===manifest);
  if(!runtime)throw new Error("Runtime vérifié introuvable.");
  if(!existsSync(binary)||!existsSync(host))throw new Error("Binaires release introuvables.");
  const server=createServer((req,res)=>{if(req.url!=="/"){res.writeHead(404).end();return;}res.setHeader("Cache-Control","no-store");res.setHeader("Content-Type","text/html; charset=utf-8");res.end(fixture);});
  await new Promise(r=>server.listen(0,"127.0.0.1",r));
  const url=`http://127.0.0.1:${server.address().port}/`;
  const terminal=createInterface({input:process.stdin,output:process.stdout});
  const display=await terminal.question("Écran : résolution, Hz, échelle ; taille initiale du dock (à reproduire) : ");
  writeFileSync(join(directory,"metadata.json"),JSON.stringify({schema:2,clock:"CLOCK_MONOTONIC",diagnostic,label,date:new Date().toISOString(),platform:process.platform,arch:process.arch,session:process.env.XDG_SESSION_TYPE,display,fixture_sha256:hash(fixture),manifest_sha256:manifest,app_sha256:hash(readFileSync(binary)),host_sha256:hash(readFileSync(host)),git_head:spawnSync("git",["rev-parse","HEAD"],{cwd:root,encoding:"utf8"}).stdout.trim(),tracked_diff_sha256:hash(spawnSync("git",["diff","HEAD"],{cwd:root}).stdout),frame_rate:process.env.PANEFLOW_BROWSER_FRAME_RATE??"60",resize_refresh:process.env.PANEFLOW_BROWSER_RESIZE_REFRESH??"1",url},null,2));
  const phases=[];
  const mark=name=>{phases.push({name,clock:"CLOCK_MONOTONIC",at_ns:now()});writeFileSync(join(directory,"phases.json"),JSON.stringify(phases,null,2));};
  let child;
  try {
    await terminal.question("Ferme les autres instances PaneFlow. Entrée pour lancer la collecte. ");
    child=spawn(binary,[],{cwd:root,env:{...process.env,PANEFLOW_CEF_ROOT:runtime,PANEFLOW_BROWSER_HOST:host,PANEFLOW_BROWSER_BENCH:join(directory,"events.jsonl"),PANEFLOW_BROWSER_TRACE_DIR:traceDirectory},stdio:["ignore","ignore","inherit"]});
    const exited=new Promise((res,rej)=>{child.once("error",rej);child.once("exit",res);});
    mark("load");
    console.log(`Dans un seul onglet Browser, ouvre ${url}`);
    while(true){
      await terminal.question("Une fois la page locale visible, reviens ici et appuie sur Entrée. ");
      const recording=join(directory,"events.jsonl");
      const snapshot=existsSync(recording)?readFileSync(recording,"utf8"):"";
      const rows=snapshot.slice(0,snapshot.lastIndexOf("\n")).split("\n").filter(Boolean).map(line=>JSON.parse(line));
      if(rows.some(e=>e.event==="frame_received"&&e.at_ns>=phases[0].at_ns))break;
      console.log(`Aucune image reçue : ouvre ${url} dans l'onglet Browser avant de continuer.`);
    }
    for(const [name,instruction] of (diagnostic ? [["resize","Agrandis puis réduis continuellement le dock entre les mêmes deux largeurs."]] : [["steady","Ne touche plus à la page."],["scroll","Scrolle continuellement vers le bas puis vers le haut."],["resize","Agrandis puis réduis continuellement le dock entre les mêmes deux largeurs."]])){
      await terminal.question(`${instruction} Entrée, puis retourne dans PaneFlow : 5 s de préparation et 20 s de mesure. `);
      mark("prepare_"+name);await new Promise(r=>setTimeout(r,5000));mark(name);await new Promise(r=>setTimeout(r,20000));mark("end_"+name);
      const snapshot=readFileSync(join(directory,"events.jsonl"),"utf8");
      const complete=snapshot.slice(0,snapshot.lastIndexOf("\n")).split("\n").filter(Boolean).map(line=>JSON.parse(line));
      if(!analyze(complete,phases).some(p=>p.phase===name&&p.frames>0))throw new Error(`Aucune frame pendant ${name}, collecte arrêtée. Vérifie que la page locale est visible.`);
      console.log(`${name} terminé.`);
    }
    if(diagnostic){
      console.log("Ferme seulement l'onglet Browser de la page locale. Laisse PaneFlow ouvert pendant l'export.");
      const recorded=readFileSync(join(directory,"events.jsonl"),"utf8");
      const rows=recorded.slice(0,recorded.lastIndexOf("\n")).split("\n").filter(Boolean).map(line=>JSON.parse(line));
      const resizeStart=phases.find(p=>p.name==="resize").at_ns;
      const resizeEnd=phases.find(p=>p.name==="end_resize").at_ns;
      const participants=new Set(rows.filter(e=>e.event==="resize_sent"&&e.at_ns>=resizeStart&&e.at_ns<resizeEnd).map(e=>e.page));
      const expected=[...participants].map(page=>rows.filter(e=>e.page===page&&e.event==="host_ready"&&e.at_ns<resizeEnd).at(-1)?.fields.pid).filter(Number.isSafeInteger).map(pid=>`cef-${pid}.json`);
      if(!expected.length)throw new Error("Aucun host de resize identifié dans la collecte.");
      let complete=[];
      const deadline=Date.now()+90000;
      while(Date.now()<deadline){
        complete=expected.filter(n=>{try{const trace=JSON.parse(readFileSync(join(traceDirectory,n),"utf8"));return Array.isArray(trace.traceEvents)&&trace.traceEvents.length>0;}catch{return false;}});
        if(complete.length===expected.length)break;
        await new Promise(r=>setTimeout(r,500));
      }
      if(complete.length!==expected.length)throw new Error("Export Chromium absent ou incomplet. Les événements bruts restent conservés dans "+directory);
      writeFileSync(join(directory,"trace-receipt.json"),JSON.stringify({files:complete,complete:true},null,2));
      console.log(`Trace Chromium exportée (${complete.length} fichier).`);
    }
    console.log("Ferme PaneFlow normalement pour terminer la collecte.");
    await exited;
    if(!diagnostic)report(directory);
    console.log(`Résultats : ${directory}`);
  } finally {terminal.close();server.close();if(child&&child.exitCode===null)child.kill("SIGTERM");}
}

if(import.meta.url===pathToFileURL(process.argv[1]).href){
  const [mode,arg,other]=process.argv.slice(2);
  try{if(mode==="record")await record(arg??"baseline");else if(mode==="trace-resize")await record(arg??"resize-trace",true);else if(mode==="report")report(resolve(arg));else if(mode==="compare")compare(resolve(arg),resolve(other));else throw new Error("Usage: bun scripts/browser-qualification/dock-benchmark.mjs record baseline | report <dossier>");}
  catch(error){console.error(error.message);process.exitCode=1;}
}
