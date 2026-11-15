// Where procast Focus sends a blocked site's tab: plain text, nothing to click.
const site = new URLSearchParams(location.search).get("site") || "This site";
document.title = "Blocked";
document.body.textContent = `${site} is blocked by procast Focus`;
