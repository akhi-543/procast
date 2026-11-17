// procast's browser companion. procast asks, this answers, one request at a time over
// native messaging (procast.exe is the host; it relays to the running procast).
//   {t: "tabs"}                      -> {t: "tabs", browser, tabs: [{id, window, title, url, icon}]}
//   {t: "activate", id, window}      -> {t: "activate", ok}
//   {t: "open", url}                 -> {t: "open", ok, existing}: the tab already showing url, else a new one
//   {t: "focusSites", mode, sites, snooze} -> {t: "focusSites", ok}: Focus. mode "block" sends tabs on those domains
//                                       (and their subdomains) to blocked.html, "allow" sends every other tab there, "off"
//                                       stops. The page's Snooze lets its site through for `snooze` minutes.

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

// Focus: the host a tab is on if the filter keeps it out, else null. Only web pages count.
let focusRule = { mode: "off", sites: [], snooze: 0 };
// Hosts the blocked page's Snooze let through, and until when (ms). procast repeats the rule
// every 15 s, which blocks them again once that has passed. A session's snoozes end with it:
// cleared when Focus goes off or a rule with other sites arrives.
// ponytail: a new session with the very same mode and sites keeps the old one's snoozes (procast
// sends no session id); send one with the rule if that matters.
const snoozed = new Map();

function blockedHost(url) {
  if (focusRule.mode === "off") return null;
  let host;
  try {
    const u = new URL(url);
    if (u.protocol !== "http:" && u.protocol !== "https:") return null;
    host = u.hostname.toLowerCase().replace(/\.$/, "").replace(/^www\./, "");
  } catch (e) {
    return null;
  }
  if ((snoozed.get(host) || 0) > Date.now()) return null;
  const listed = focusRule.sites.some((s) => host === s || host.endsWith("." + s));
  return (focusRule.mode === "block") === listed ? host : null;
}

function redirect(tab, url) {
  const host = blockedHost(url);
  if (!host) return;
  const page = new URLSearchParams({ site: host, snooze: focusRule.snooze, url });
  chrome.tabs.update(tab, { url: chrome.runtime.getURL("blocked.html") + "?" + page });
}

// The blocked page's Snooze: its site is let through for a while, and the tab goes back to it.
chrome.runtime.onMessage.addListener((msg, sender) => {
  const fromPage = typeof sender.url === "string" && sender.url.startsWith(chrome.runtime.getURL("blocked.html"));
  if (!msg || msg.type !== "snooze" || typeof msg.site !== "string" || !msg.site || !fromPage || !sender.tab || !(focusRule.snooze > 0)) return;
  snoozed.set(msg.site, Date.now() + focusRule.snooze * 60000);
  const back = /^https?:\/\//.test(msg.url) ? msg.url : "https://" + msg.site;
  chrome.tabs.update(sender.tab.id, { url: back });
});

// A tab that navigates while Focus runs.
chrome.tabs.onUpdated.addListener((id, change) => {
  if (change.url) redirect(id, change.url);
});

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
    case "focusSites": {
      const rule = { mode: req.mode, sites: req.sites || [], snooze: req.snooze || 0 };
      if (rule.mode === "off" || rule.mode !== focusRule.mode || rule.sites.join() !== focusRule.sites.join()) snoozed.clear();
      focusRule = rule;
      for (const t of await chrome.tabs.query({})) redirect(t.id, t.url);
      return { t: "focusSites", ok: true };
    }
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
