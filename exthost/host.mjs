// akscast extension host: one long-lived Node process, one worker per command run.
// Speaks newline-delimited JSON on stdio with the Rust app:
//   in:  {t:"run", run, entry, mode, arguments, preferences, environment, launchType, memoryMb}
//        {t:"event", run, fn, args} | {t:"pop", run} | {t:"reply", run, id, value|error} | {t:"stop", run}
//   out: whatever a worker posts ({t:"render"|"call"|"request"|"error"|"log"|"exit"}), tagged with run
import { Worker } from "node:worker_threads";
import { createInterface } from "node:readline";
import fs from "node:fs";

const workers = new Map();
const out = (msg) => process.stdout.write(JSON.stringify(msg) + "\n");

function start(msg) {
  fs.mkdirSync(msg.environment.supportPath, { recursive: true });
  const worker = new Worker(new URL("./worker.js", import.meta.url), {
    workerData: msg,
    resourceLimits: { maxOldGenerationSizeMb: msg.memoryMb ?? 256 },
    // Extensions print freely; keep it off our protocol channel.
    stdout: true,
    stderr: true,
  });
  worker.stdout.on("data", (d) => out({ t: "log", run: msg.run, text: String(d) }));
  worker.stderr.on("data", (d) => out({ t: "log", run: msg.run, text: String(d) }));
  worker.on("message", (m) => out({ ...m, run: msg.run }));
  worker.on("error", (e) => out({ t: "error", run: msg.run, message: String(e?.stack ?? e) }));
  worker.on("exit", (code) => {
    workers.delete(msg.run);
    out({ t: "exit", run: msg.run, code });
  });
  workers.set(msg.run, worker);
}

createInterface({ input: process.stdin }).on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.t === "run") return start(msg);
  const worker = workers.get(msg.run);
  if (!worker) return;
  if (msg.t === "stop") worker.terminate();
  else worker.postMessage(msg);
});

// The app closing our stdin means it quit.
process.stdin.on("end", () => process.exit(0));
out({ t: "ready", node: process.version });
