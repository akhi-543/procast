// Installs a Raycast extension into procast: from the raycast/extensions repo by name, or
// from a local folder (your own extensions). Builds with Raycast's `ray build` (MIT CLI).
// usage: node install.mjs <name | local folder> <extensions dir>
//        node install.mjs --sync <extensions dir>   (just refresh the store index)
// Prints one JSON status line per step: {step, ok, message}.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const [source, extDir] = process.argv.slice(2);
const say = (step, message, ok = true) => console.log(JSON.stringify({ step, ok, message }));
const run = (cmd, args, cwd) =>
  execFileSync(cmd, args, { cwd, stdio: ["ignore", "pipe", "pipe"], shell: process.platform === "win32", encoding: "utf8" });

// One shared sparse clone holding every extension's package.json; installs add their folder.
function syncRepo() {
  const repo = path.join(extDir, ".src", "raycast-extensions");
  if (!fs.existsSync(path.join(repo, ".git"))) {
    say("clone", "cloning raycast/extensions (sparse, first time only)");
    run("git", ["clone", "--filter=blob:none", "--no-checkout", "--depth", "1", "--sparse", "https://github.com/raycast/extensions", repo]);
    run("git", ["sparse-checkout", "set", "--no-cone", "/extensions/*/package.json"], repo);
    run("git", ["checkout", "main"], repo);
  } else {
    run("git", ["pull", "--depth", "1", "--ff-only"], repo);
  }
  return repo;
}

try {
  if (!source || !extDir) throw new Error("usage: install.mjs <name | folder> <extensions dir>");
  if (source === "--sync") {
    syncRepo();
    say("sync", "store index up to date");
    process.exit(0);
  }
  let src;
  if (fs.existsSync(source) && fs.statSync(source).isDirectory()) {
    // Build from a copy so the original folder (maybe your git repo) is never touched.
    src = path.join(extDir, ".src", "local", path.basename(path.resolve(source)));
    fs.rmSync(src, { recursive: true, force: true });
    fs.cpSync(source, src, { recursive: true, filter: (p) => !/[\\/](node_modules|\.git)([\\/]|$)/.test(p) });
    say("copy", `copied ${source}`);
  } else {
    if (!/^[a-z0-9][a-z0-9-]*$/i.test(source)) throw new Error(`not a store extension name: ${source}`);
    const repo = syncRepo();
    run("git", ["sparse-checkout", "add", `/extensions/${source}/`], repo);
    src = path.join(repo, "extensions", source);
    if (!fs.existsSync(path.join(src, "package.json"))) throw new Error(`no extension named "${source}" in raycast/extensions`);
    say("fetch", `fetched ${source}`);
  }

  const manifest = JSON.parse(fs.readFileSync(path.join(src, "package.json"), "utf8"));
  const platforms = (manifest.platforms ?? ["macOS"]).map((p) => p.toLowerCase());
  if (!platforms.includes("windows")) say("platform", `${manifest.name} declares ${platforms.join(", ")} only; it may not work on Windows`, false);

  // --ignore-scripts: no third-party install scripts run on your machine.
  run("npm", [fs.existsSync(path.join(src, "package-lock.json")) ? "ci" : "install", "--ignore-scripts", "--no-audit", "--no-fund", "--loglevel=error"], src);
  say("deps", "dependencies installed");

  const out = path.join(extDir, manifest.name);
  fs.rmSync(out, { recursive: true, force: true });
  run("npx", ["ray", "build", "-e", "dist", "-I", "-o", out], src);
  say("build", `installed ${manifest.title ?? manifest.name} to ${out}`);
} catch (e) {
  say("error", String(e.stderr || e.message || e).trim().split("\n").slice(-3).join(" "), false);
  process.exit(1);
}
