// Where procast Focus sends a blocked site's tab (background.js): what is blocked, and Snooze,
// which lets the site through for `snooze` minutes and goes back to the page.
const q = new URLSearchParams(location.search);
const site = q.get("site") || "This site";
const minutes = Number(q.get("snooze")) || 0;
document.getElementById("title").textContent = `${site} is blocked by procast`;
const snooze = document.getElementById("snooze");
if (minutes > 0) {
  snooze.textContent = `Snooze for ${minutes} minute${minutes === 1 ? "" : "s"}`;
  snooze.hidden = false;
  snooze.onclick = () => chrome.runtime.sendMessage({ type: "snooze", site, url: q.get("url") || "" });
}
