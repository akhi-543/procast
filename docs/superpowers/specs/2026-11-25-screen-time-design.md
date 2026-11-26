# Screen Time (port of micro bot into procast): design

2026-11-25 · source: `D:\Pro\micro bot` (`tracker.py`, `dashboard.html`, `extension/`, `categories.json`, `settings.json`)

## Intent

micro bot is a separate Python screen-time tracker: it samples the foreground app every 2 s,
gets the active website from its own Chromium extension over HTTP, keeps per-hour counters in
SQLite and serves a dashboard on `http://127.0.0.1:8737`. procast already runs all day, already
knows the foreground window (`src/foreground.rs`), already has a browser companion connected
over native messaging (`browser/background.js`, `src/browser.rs`) and already has app/website
categories (Focus, `src/focus.rs`). Moving the tracker in gives one process, no Python, and one
browser extension instead of two.

Success: micro bot's scheduled task `WellbeingTracker` is disabled, procast records the same
numbers micro bot did (same rules, same schema), the old history is imported, the existing
dashboard works unchanged at the same URL, and screen time is readable from the launcher.

## Goals, non-goals, anti-goals

**Goals**
1. Native tracking inside procast, numerically equivalent to `tracker.py`.
2. Active-tab site tracking through the existing browser companion.
3. Same SQLite schema; import of micro bot's `data.db`, `categories.json`, `settings.json`.
4. Launcher commands: **Screen Time** (today) and **Screen Time This Week**, plus
   **Open Screen Time Dashboard** and **Import Screen Time from micro bot**.
5. `dashboard.html` served by procast at `http://127.0.0.1:8737`, unchanged API.
6. One category list shared by Focus and Screen Time.
7. Weekly summary as a procast toast.

**Non-goals (v1)**
- Native charts in GPUI (hourly bars, heatmap). The dashboard has them; the launcher shows lists.
- A Settings-window page. `[screen_time]` lives in `config.toml` (hot-reloads), like layout presets.
- Tray tooltip with today's total (micro bot had one). The Screen Time command shows it.
- Firefox. The companion is Chromium-only, as before.
- Changing micro bot's repo beyond a README note pointing to procast.

**Anti-goals (a failure even if it works)**
- Numbers that drift from micro bot's for the same usage (double counting the browser and its
  sites, counting idle time, counting sleep).
- Anything leaving the machine, or the dashboard reachable from another site (DNS rebinding,
  cross-origin POST). The HTTP guards in `tracker.py` port as they are.
- A database write or a bad category in config stopping tracking or blocking the UI thread.
- Paths, titles or query strings recorded for websites. Hostname only, as today.

## Constraints

- No new dependency except `rusqlite` with `bundled` and `backup` features (the schema and the
  backup copies need SQLite; the import is an `ATTACH`). HTTP server on `std::net::TcpListener`.
- Build stays clean: zero warnings, zero clippy, tests green (repo rule).
- Data lives with procast's other data: `%APPDATA%\procast\screentime.db`, backups in
  `%APPDATA%\procast\screentime-backups\` (7 daily copies, as micro bot).
- Port 8737 is kept so old bookmarks and the weekly-summary link work. micro bot must not run
  at the same time (it would hold the port and double-record); the import stops it.
- Browser companion protocol is request/answer, procast asks. The extension cannot push.

## Approaches

| | What | Trade-off |
|---|---|---|
| **A (picked)** | Native Rust module in procast; reuse companion and Focus categories; serve the existing dashboard | Most work; one process, no Python, one extension |
| B | Raycast-format extension in `exthost` reading micro bot's `/api/stats` | Cheapest, but keeps Python and two extensions; charts only as markdown images |
| C | Python backend kept, native procast view over its DB | Costs about A, keeps Python; dropped |

The user picked A.

## Design

### Files

- `src/wellbeing.rs`: sampler, accounting, storage, stats, import, weekly summary, tests.
- `src/wellbeing_http.rs`: the localhost server (kept apart: it is the security boundary).
- `assets/screen-time/dashboard.html`: copied from micro bot, embedded with `include_str!`.
  Two edits, else unchanged: its `SLOTS` colour list takes the merged category names (it
  colours only listed names; the rest go grey), and the "categories.json isn't read-only"
  error text says `config.toml`. The JSON API it reads is unchanged. *My call.*
- Edits: `main.rs` (start, `Msg` arms, modes), `commands.rs` (4 commands), `config.rs`
  (`[screen_time]`, category helpers), `focus.rs` (built-in categories), `browser.rs` +
  `browser/background.js` (active tab).

### Sampling (port of `run()` / `account()` / `focus()`)

A background thread, started from `main.rs` when `[screen_time] enabled`. It keeps micro bot's
2 s poll rather than switching to the foreground hook: polling is what makes idle time, the
sleep gap and site changes inside one browser window come out right, and it ports `tracker.py`
1:1. Each sample:

- `dt` from a monotonic clock; drop the sample if `dt <= 0` or `dt > 10 s` (machine slept).
- Foreground exe: `GetForegroundWindow` → pid → `apps::exe_path`, file name lower-cased; no
  window = `""`; a process that can't be opened = `"system"`. Ignore `""` and `lockapp.exe`.
- Idle: `GetLastInputInfo` vs `GetTickCount`, wrapping u32. Idle > 120 s = not credited,
  unless a browser is in front, a site is known and its tab is audible.
- Credit `("app", exe, dt)`, plus `("site", host, dt)` when a browser is in front and a host is
  known. Browsers: `chrome.exe msedge.exe brave.exe opera.exe vivaldi.exe`.
- Focus streak and switches per `focus()`; daily row: switches, longest streak, first and last
  `HH:MM`.
- Counters accumulate in memory (`Mutex`), flushed every 30 s and on quit with the same upserts
  as `flush()`. A failed flush keeps the counters and retries; it never panics the thread.
- Once per day after a flush: backup, then weekly-summary check.

### Active tab

New companion request `{t: "active"}` → `{t: "active", browser, focused, host, audible}`:
the active tab of `chrome.windows.getLastFocused()`, `focused` = that window has OS focus,
`host` = hostname lower-cased without `www.` for http(s) pages else `""`. procast validates the
host (`[a-z0-9.-]{0,253}`) as `tracker.py` did.

The sampler, when the foreground exe is a browser, calls `Browsers::ask_all({t:"active"})`
each sample. Answers arrive as `Msg::Browser(conn, Reply::Active{..})` on the UI thread, which
stores the newest answer with `focused == true` in a shared `RwLock` the sampler reads. Not a
browser in front = host cleared. This fixes micro bot's "last browser to report wins" corner
(two browsers open). The stored answer is at most one sample (2 s) old; that lag is accepted.

`browser/background.js` gets the new case; the extension's version is bumped and the README's
browser companion section says to reload it. micro bot's own extension is then removed by the
user.

### Storage

Schema unchanged from `tracker.py` `create()`: `usage(day, hour, kind, name, seconds)`,
`daily(day, switches, longest, first, last)`, `meta(key, value)`. One connection per flush or
query, opened on the calling thread (sampler, HTTP worker, or a background task for launcher
views; never the UI thread).

### Categories: one list

Focus's list becomes the only one. `focus::BUILT_IN` gains micro bot's categories that Focus
lacks (Development, Productivity, Browsing, Entertainment, Games, …: whatever
`categories.json` holds), and an `Other` category (entries there override a broader suffix
rule, as in `classify()`). Where both have a near-duplicate (micro bot `Games` vs Focus
`Gaming`, `Entertainment` vs `Streaming`), the Focus name is kept and micro bot's entries are
merged into it.

Classification ports `classify()`: apps by exact exe; sites by most specific suffix, never
the bare TLD; nothing found = `Other`. A name in two categories resolves to the first in list
order (Focus allows overlap; Screen Time needs one answer).

Moving a name (dashboard POST `/api/category`, or the launcher action) ports
`recategorize()`: remove it from every category, add it to the target, written to
`config.toml` through `toml_edit` (comments kept). Editing a built-in category writes a
`[[focus_category]]` of the same name, the existing override mechanism. Consequence, accepted:
moving `youtube.com` out of Streaming also stops Focus blocking it under Streaming.

`[screen_time]` in `config.toml`:

```toml
[screen_time]
enabled = false               # the import turns it on; a fresh install turns it on by hand
productive = ["Development", "Productivity"]
distracting = ["Social", "Streaming", "Gaming"]
weekly_summary = true
```

### Stats

Port `aggregate()`, `stats()` and `export()` exactly, including: site seconds subtracted from
the browser's category (the browser's category = `chrome.exe`'s, as before), top 12 apps and
sites, previous-period comparison, `daily` focus stats, CSV with a BOM and `'` before a
leading `= + - @`. The JSON shape stays byte-compatible with what `dashboard.html` reads
(`options` = category names, `kinds` from `[screen_time]`).

### Dashboard server

> Superseded by 2026-11-26-screen-time-native-dashboard-design.md: the dashboard is native and the web server is gone.

A thread with `TcpListener` on `127.0.0.1:8737`; a failed bind is reported once through
`Msg::error` ("port 8737 is in use: is micro bot still running?") and the rest of Screen Time
keeps working. Minimal HTTP/1.1, one request per connection, body ≤ 1 KiB. Routes and guards
exactly as `Handler`:

- `GET /` → embedded dashboard; `GET /api/stats|days|export.csv?from&to`, span 0–400 days.
- `POST /api/category` only with `Origin` = the dashboard's own origin.
- Every request: `Host` must be `127.0.0.1:8737` or `localhost:8737` (DNS rebinding).
- `/tab` is gone (the companion replaces it).
- Responses `Cache-Control: no-store`; errors as `{"error": …}` with 500, as before.

### Launcher

- **Screen Time** (`Cmd::ScreenTime`): a list mode. Header row: today's total and change vs
  yesterday. Sections: Categories, Apps, Sites, each row with time and category; detail pane
  with today's per-hour numbers for that row and the focus stats. Actions (Ctrl+K): Move to
  Category… (category picker, same write as above), Open Dashboard, Export CSV (to Downloads).
- **Screen Time This Week**: the same view over Monday–today, compared with last Monday to the
  same weekday (a fair partial-week comparison; `stats()` gets the previous span as an
  argument instead of deriving it; the HTTP API keeps deriving it as before). *My call.*
- **Open Screen Time Dashboard**: opens `http://127.0.0.1:8737` with `win::open`.
- **Import Screen Time from micro bot**: a form with the micro bot folder (default
  `D:\Pro\micro bot`, the user's install). It:
  1. ends and disables the `WellbeingTracker` task (`schtasks /End`, `/Change /Disable`); its
     last ≤ 30 s of unflushed counters are lost, accepted;
  2. `ATTACH`es `data.db` and upserts `usage` (add seconds) and `daily` (same merge as flush);
  3. merges `categories.json` into the category list and `settings.json` into `[screen_time]`.
     **Focus wins** (user's decision): a name Focus already files somewhere stays there
     (`reddit.com`, `instagram.com` stay Social; micro bot's Communication sites already in
     Messaging stay there); only names Focus lacks are added, to the mapped category. Names
     Focus already files keep their category; new names do join it, so a Focus block of that
     category (e.g. Messaging gaining Outlook, Teams, Zoom) covers them too, and the import
     form says so (*controller ruling R14*). History re-labels with it, since categories are
     resolved at read time;
  4. sets `[screen_time] enabled = true` (tracking starts only now, so micro bot and procast
     never record the same hour; *builder's call, from the check*);
  5. reports counts in a toast. Running it twice double-adds; the form says so, and `meta`
     records `imported_from` + time to warn on a second run.

Views reuse existing list, detail pane and form components. No new visual components.

### Weekly summary

Ported from `weekly_summary()`: on the first flush of a new week, if `meta.summary_week` is
older, compute last week's stats; if total ≥ 60 s and `weekly_summary = true`, show an Info
toast with `Stay::Sticky` (it fires ~30 s after sign-in; an auto-closing one is missed); record
the week either way. Toasts cut at 60 characters and carry no link, so the text is shortened
to fit: `Last week 12h 34m, 15% more. Most: Development` (`summary_text()` and its test change
accordingly). No link: Screen Time This Week is one search away. *My call.*

### Tests

Port `test_tracker.py` to `#[cfg(test)]` in `wellbeing.rs`: account (gap, ignored, idle,
audible exemption), streak/switches, classify (suffix, no bare TLD, Other override),
aggregate subtraction, stats shape against a fixture DB, CSV escaping, summary text, import
merge. `wellbeing_http.rs`: Host and Origin guards, span limits, body limit.
`browser/`: a node test for the `active` case, like `ytmusic.test.mjs`.

### Build order

1. Sampler + storage + stats + tests (off by default).
2. Active tab in the companion.
3. Import command; disable micro bot; tracking turns on.
4. Dashboard server.
5. Launcher views.
6. Category unification + weekly toast.

## Left to the builder

- Exact list-mode wiring (new `Mode::ScreenTime` vs reuse of an existing list mode).
- How the detail pane lays out the per-hour numbers.
- Merging micro bot's other categories into Focus names beyond Games/Entertainment.
- Whether the sampler shares `foreground.rs`'s thread or has its own (own thread is simpler).
