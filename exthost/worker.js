// Runs one extension command. Redirects @raycast/api and react to akscast's copies.
const Module = require("node:module");
const path = require("node:path");
const { parentPort, workerData } = require("node:worker_threads");

const ours = {
  "@raycast/api": path.join(__dirname, "runtime.js"),
  react: require.resolve("react"),
  "react/jsx-runtime": require.resolve("react/jsx-runtime"),
};
const resolve = Module._resolveFilename;
Module._resolveFilename = function (request, ...rest) {
  return ours[request] ?? resolve.call(this, request, ...rest);
};

const api = require("./runtime.js");
process.on("uncaughtException", api.__fail);
process.on("unhandledRejection", api.__fail);

const mod = require(workerData.entry);
const command = mod.default ?? mod;
const props = { arguments: workerData.arguments, launchType: workerData.launchType, launchContext: workerData.context };

if (workerData.mode === "view") {
  api.__mount(command, props);
} else {
  // no-view (and background interval) commands are plain async functions.
  Promise.resolve()
    .then(() => command(props))
    .then(
      () => parentPort.postMessage({ t: "exit" }),
      (e) => parentPort.postMessage({ t: "exit", error: String(e?.stack ?? e) }),
    );
}
