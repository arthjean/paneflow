import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { fixtureBundle, serveFixtures } from "./browser-qualification/fixtures.mjs";
import { compareCaptures, inspectCapture } from "./browser-qualification/measurements.mjs";
import { replayBundle } from "./browser-qualification/replay.mjs";
import { runWitness } from "./browser-qualification/witness.mjs";

async function readCapture(path) {
  if (!path) throw new Error("capture file is required");
  const location = pathToFileURL(resolve(path));
  return { run: JSON.parse(await readFile(location, "utf8")), location };
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  switch (command) {
    case "witness": {
      if (args.length < 2 || args.length > 4) throw new Error("witness requires binary and new output directory, optionally fixture and seconds");
      process.stdout.write(`${JSON.stringify(await runWitness(args[0], args[1], args[2], Number(args[3] ?? 2)), null, 2)}\n`);
      break;
    }
    case "serve": {
      if (args.length > 1 || (args.length === 1 && !/^\d+$/.test(args[0]))) throw new Error("serve accepts an optional port");
      const port = Number(args[0] ?? 0);
      if (!Number.isSafeInteger(port) || port < 0 || port > 65535) throw new Error("port must be between 0 and 65535");
      const { server, url, manifest } = await serveFixtures(port);
      process.stdout.write(`${JSON.stringify({ url, ...manifest })}\n`);
      const stop = () => { server.close(); server.closeAllConnections(); };
      process.once("SIGINT", stop);
      process.once("SIGTERM", stop);
      break;
    }
    case "manifest": {
      if (args.length) throw new Error("manifest takes no arguments");
      process.stdout.write(`${JSON.stringify((await fixtureBundle()).manifest, null, 2)}\n`);
      break;
    }
    case "replay": {
      if (args.length) throw new Error("replay takes no arguments");
      process.stdout.write(`${JSON.stringify(await replayBundle())}\n`);
      break;
    }
    case "inspect": {
      if (args.length !== 1) throw new Error("inspect requires one capture file");
      const { run, location } = await readCapture(args[0]);
      process.stdout.write(`${JSON.stringify(await inspectCapture(run, location), null, 2)}\n`);
      break;
    }
    case "compare": {
      if (args.length !== 2) throw new Error("compare requires reference and candidate capture files");
      const left = await readCapture(args[0]);
      const right = await readCapture(args[1]);
      process.stdout.write(`${JSON.stringify(await compareCaptures(left.run, right.run, left.location, right.location), null, 2)}\n`);
      break;
    }
    default:
      throw new Error("usage: bun scripts/browser-qualification.mjs serve [port] | manifest | replay | inspect capture.json | compare reference.json candidate.json");
  }
}

try { await main(); }
catch (error) {
  process.stderr.write(`${JSON.stringify({ status: "REJECTED", error: error instanceof Error ? error.message : String(error) })}\n`);
  process.exitCode = 1;
}
