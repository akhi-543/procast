// procast's browser companion. procast asks, this answers, one request at a time over
// native messaging (procast.exe is the host; it relays to the running procast).
//   {t: "tabs"}                      -> {t: "tabs", browser, tabs: [{id, window, title, url, icon}]}
//   {t: "activate", id, window}      -> {t: "activate", ok}
//   {t: "open", url}                 -> {t: "open", ok, existing}: the tab already showing url, else a new one
//   {t: "ytm", req, op, ...}         -> {t: "ytm", req, op, title, items, current, error?}: YouTube Music, see ytmusic.js

importScripts("ytmusic.js");
ytm.setup();

const HOST = "com.procast.browser";
let port = null;

function browserName() {
  const ua = navigator.userAgent;
  if (ua.includes("Edg/")) return "Edge";
  if (navigator.brave) return "Brave";
  if (ua.includes("OPR/")) return "Opera";
  if (ua.includes("Vivaldi")) return "Vivaldi";
  return "Chrome";
}

// The same page: ignore the fragment and a trailing slash.
function samePage(a, b) {
  const norm = (u) => (u || "").split("#")[0].replace(/\/$/, "");
  return norm(a) === norm(b);
}

async function focus(tab) {
  await chrome.tabs.update(tab.id, { active: true });
  await chrome.windows.update(tab.windowId, { focused: true });
}

async function handle(req) {
  switch (req.t) {
    case "tabs": {
      const tabs = await chrome.tabs.query({});
      return {
        t: "tabs",
        browser: browserName(),
        tabs: tabs.map((t) => ({ id: t.id, window: t.windowId, title: t.title || "", url: t.url || "", icon: t.favIconUrl || "" })),
      };
    }
    case "activate": {
      const tab = await chrome.tabs.get(req.id);
      await focus(tab);
      return { t: "activate", ok: true };
    }
    case "open": {
      const same = (await chrome.tabs.query({})).find((t) => samePage(t.url, req.url));
      if (same) {
        await focus(same);
      } else {
        const tab = await chrome.tabs.create({ url: req.url });
        await chrome.windows.update(tab.windowId, { focused: true });
      }
      return { t: "open", ok: true, existing: !!same };
    }
    case "ytm":
      return ytm.handle(req);
    default:
      return { t: req.t, error: "unknown request" };
  }
}

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST);
  } catch (e) {
    port = null;
    return;
  }
  port.onMessage.addListener(async (req) => {
    let reply;
    try {
      reply = await handle(req);
    } catch (e) {
      reply = { t: req.t, error: String(e) };
    }
    port?.postMessage(reply);
  });
  port.onDisconnect.addListener(() => {
    port = null;
  });
}

// An open native port keeps this worker alive; the alarm reconnects after procast restarts.
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.alarms.create("procast", { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
connect();
