import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

// background.js with just enough of `chrome` to load; `handle` is its request dispatcher.
function load(window) {
  const hook = { addListener() {} };
  const chrome = {
    runtime: { onMessage: hook, onStartup: hook, onInstalled: hook, getURL: (p) => p, connectNative() { throw new Error("no host"); } },
    tabs: { onUpdated: hook },
    alarms: { create() {}, onAlarm: hook },
    windows: { getLastFocused: async () => window },
  };
  const sandbox = { chrome, URL, URLSearchParams, navigator: { userAgent: "Edg/1" }, importScripts() {}, ytm: { setup() {} } };
  vm.runInNewContext(readFileSync(new URL("background.js", import.meta.url), "utf8"), sandbox);
  return sandbox.handle;
}

const active = (url, extra = {}) => ({ focused: true, tabs: [{ active: false, url: "https://other/" }, { active: true, url, audible: false, ...extra }] });

test("active tab: host without www, audible, focused", async () => {
  const r = await load(active("https://www.youtube.com/watch?v=1", { audible: true }))({ t: "active" });
  assert.deepEqual(JSON.parse(JSON.stringify(r)), { t: "active", browser: "Edge", focused: true, host: "youtube.com", audible: true });
});

test("active tab: not a web page has no host", async () => {
  for (const url of ["chrome://newtab", "edge://settings", undefined]) {
    assert.equal((await load(active(url))({ t: "active" })).host, "");
  }
});

test("active tab: an unfocused window says so", async () => {
  const r = await load({ ...active("https://github.com/"), focused: false })({ t: "active" });
  assert.deepEqual([r.focused, r.host], [false, "github.com"]);
});
