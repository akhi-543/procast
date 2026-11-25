# Screen Time Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port micro bot's screen-time tracker into procast as a native module, with its history imported and its dashboard served at the same URL.

**Architecture:** `src/wellbeing.rs` holds pure accounting (ported 1:1 from `tracker.py`), SQLite storage, stats, import and a 2 s sampler thread. The browser companion answers a new `{t:"active"}` request for the site in front. `src/wellbeing_http.rs` serves the embedded `dashboard.html` and its JSON API on `127.0.0.1:8737`. Categories come from Focus's list (`focus::categories`); launcher commands show today and this week.

**Tech Stack:** Rust 2024, GPUI (gpui-ce), `rusqlite` (bundled, backup), `toml_edit`, `serde_json`, Win32 via `windows`, Chromium MV3 extension JS.

**Spec:** `docs/superpowers/specs/2026-11-25-screen-time-design.md` (read it with this plan). Source being ported: `D:\Pro\micro bot\tracker.py`, `test_tracker.py`, `dashboard.html`, `categories.json`, `settings.json`.

## Global Constraints

- Only new dependency: `rusqlite = { version = "0.32", features = ["bundled", "backup"] }` (or the current minor). HTTP on `std::net::TcpListener`.
- Before every commit: `cargo test` green, `cargo clippy --all-targets -- -D warnings` clean, zero warnings.
- Database `%APPDATA%\procast\screentime.db`; backups `%APPDATA%\procast\screentime-backups\screentime-YYYY-MM-DD.db`, 7 kept.
- Schema exactly: `usage(day TEXT, hour INTEGER, kind TEXT, name TEXT, seconds REAL, PRIMARY KEY(day, hour, kind, name))`, `daily(day TEXT PRIMARY KEY, switches INTEGER, longest REAL, first TEXT, last TEXT)`, `meta(key TEXT PRIMARY KEY, value TEXT)`.
- Constants from `tracker.py`: poll 2 s, idle 120 s, gap 10 s, flush 30 s. Browsers: `chrome.exe msedge.exe brave.exe opera.exe vivaldi.exe`. Ignored exes: `""`, `lockapp.exe`.
- Port 8737; allowed `Host`: `127.0.0.1:8737`, `localhost:8737`; POST `/api/category` only with `Origin` `http://127.0.0.1:8737` or `http://localhost:8737`; body ≤ 1024 bytes; span 0–400 days.
- Websites: hostname only, lower-case, no `www.`, must match `[a-z0-9.-]{0,253}`.
- `[screen_time] enabled` defaults to `false`; the import turns it on.
- Never block the UI thread with SQLite; never panic the sampler on a DB error.
- No new visual components: reuse list, detail pane, form, toast.
- Dates: reuse `calendar::days_from_civil` / `civil_from_days` (day numbers, `i64`); make `calendar::weekday` `pub(crate)`. No chrono.

## Review Focus

1. **Old companion extension** answers `{t:"active"}` with `{error:"unknown request"}`: must not raise an error toast every 2 s; app time still counts, site stays empty. (Task 4 test `active_error_is_silent`.)
2. **Database locked or unwritable**: flush fails, counters stay in memory and are written on the next flush. (Task 2 test `failed_flush_keeps_pending`.)
3. **Midnight during use**: a sample after 00:00 goes to the new day; the streak carries over but the new day's `first` is `00:00`. (Task 2 test `pending_splits_days`.)
4. **Malformed or hostile HTTP**: missing `Host`, wrong `Origin`, oversized body, bad dates, slow client. 403/400 as specified; a slow client cannot stall the server (per-connection thread, 5 s read timeout). (Task 7 tests.)
5. **Import from a wrong folder** (no `data.db`, not a micro bot DB, run twice): clear error, nothing written; second run warns from `meta.imported_from`. (Task 6 tests.)

---

### Task 1: Accounting core

**Files:**
- Create: `src/wellbeing.rs` (add `mod wellbeing;` in `src/main.rs`)
- Modify: `src/calendar.rs:64` (`weekday` → `pub(crate)`)

**Interfaces:**
- Produces:
  - `pub enum Kind { App, Site }` with `fn as_str(self) -> &'static str` (`"app"`/`"site"`), `Copy, Eq, Hash`
  - `pub fn account(exe: &str, idle_s: f64, site: &str, audible: bool, dt: f64) -> Vec<(Kind, String, f64)>`
  - `pub struct Streak { pub exe: String, pub seconds: f64 }` (`Default`), `pub fn advance(&mut self, exe: &str, dt: f64) -> u32` (1 if switched app)
  - `pub type Rules = std::collections::HashMap<String, String>`
  - `pub fn classify(kind: Kind, name: &str, rules: &Rules) -> String`
  - `pub fn duration(seconds: f64) -> String`
  - `pub fn summary_text(total: u64, previous: u64, top: &str) -> String`
  - `pub fn iso(day: i64) -> String`, `pub fn parse_iso(s: &str) -> Option<i64>`

- [ ] **Step 1: Write failing tests** in `wellbeing.rs` `mod tests`, ported from `test_tracker.py` lines 21–45, 88–96:

```rust
#[test] fn account_rules() {
    assert_eq!(account("code.exe", 0.0, "", false, 2.0), vec![(Kind::App, "code.exe".into(), 2.0)]);
    assert_eq!(account("chrome.exe", 0.0, "youtube.com", false, 2.0).len(), 2);
    assert_eq!(account("code.exe", 0.0, "youtube.com", false, 2.0).len(), 1); // site only in a browser
    assert!(account("code.exe", 500.0, "", false, 2.0).is_empty());          // idle
    assert!(!account("chrome.exe", 500.0, "youtube.com", true, 2.0).is_empty()); // audible beats idle
    assert!(account("chrome.exe", 500.0, "youtube.com", false, 2.0).is_empty());
    assert!(account("code.exe", 500.0, "youtube.com", true, 2.0).is_empty());
    assert!(!account("brave.exe", 500.0, "youtube.com", true, 2.0).is_empty());
    assert!(account("code.exe", 0.0, "", false, 3600.0).is_empty());          // slept
    assert!(account("code.exe", 0.0, "", false, 0.0).is_empty());
    assert!(account("lockapp.exe", 0.0, "", false, 2.0).is_empty());
    assert!(account("", 0.0, "", false, 2.0).is_empty());
}
#[test] fn classify_rules() { /* RULES: code.exe→Development, youtube.com→Streaming, docs.google.com→Productivity,
    google.com→Browsing, mail.google.com→Browsing, localhost→Development, com→Social (bare TLD must never match) */
    // Code.exe→Development; mystery.exe→Other; m.youtube.com→Streaming; docs.google.com→Productivity;
    // mail.google.com→Browsing; localhost→Development; notyoutube.com→Other; example.com→Other
}
#[test] fn streak_counts_switches() { /* [code, code, chrome] → [0,0,1], seconds 6; "" → 0 and seconds 0; code → 0, seconds 2 */ }
#[test] fn summary_fits_a_toast() {
    assert_eq!(summary_text(129_960, 144_400, "Development"), "Last week 36h 6m, 10% less. Most: Development");
    assert_eq!(summary_text(129_960, 0, "Development"), "Last week 36h 6m. Most: Development");
    assert_eq!(summary_text(3600, 3600, "Gaming"), "Last week 1h 0m, same. Most: Gaming");
    assert!(summary_text(359_940, 1, "Productivity").chars().count() <= 60);
}
#[test] fn iso_round_trip() { assert_eq!(parse_iso("2026-09-30").map(iso).as_deref(), Some("2026-09-30")); assert_eq!(parse_iso("2026-13-01"), None); assert_eq!(parse_iso("x"), None); }
```

- [ ] **Step 2:** `cargo test wellbeing::` → FAIL (not defined).
- [ ] **Step 3:** Implement the signatures above. `account`/`Streak::advance`/`classify`/`duration` follow `tracker.py` `account`, `focus`, `classify`, `duration` exactly (classify: suffixes from most specific, `range(max(1, len(parts) - 1))`). `parse_iso` validates month 1–12 and day against the month by round-tripping through `civil_from_days`.
- [ ] **Step 4:** `cargo test wellbeing::` → PASS; clippy clean.
- [ ] **Step 5:** `git commit -m "feat(screen-time): accounting core ported from micro bot"`

### Task 2: Storage, stats, CSV

**Files:**
- Modify: `Cargo.toml` (rusqlite), `src/wellbeing.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces:
  - `pub fn open(path: &Path) -> rusqlite::Result<rusqlite::Connection>` (creates schema)
  - `pub struct Daily { pub switches: u32, pub longest: f64, pub first: String, pub last: String }`
  - `#[derive(Default)] pub struct Pending { pub usage: HashMap<(i64, u8, Kind, String), f64>, pub daily: HashMap<i64, Daily> }` with `pub fn credit(&mut self, day: i64, hour: u8, minute: &str, credits: &[(Kind, String, f64)], switched: u32, streak: f64)` and `pub fn is_empty(&self) -> bool`
  - `pub fn flush(con: &mut Connection, p: &mut Pending) -> rusqlite::Result<()>` (one transaction; clears `p` only on success; same upserts as `tracker.py` `flush`)
  - `pub fn stats(con: &Connection, from: i64, to: i64, before: (i64, i64), rules: &Rules) -> rusqlite::Result<serde_json::Value>` (shape of `tracker.py` `stats`, without `options`/`kinds`)
  - `pub fn prior(from: i64, to: i64) -> (i64, i64)` = same length just before (the API's rule)
  - `pub fn days(con: &Connection, from: i64, to: i64, rules: &Rules) -> rusqlite::Result<serde_json::Value>` (`{"days": {...}}`)
  - `pub fn export_csv(con: &Connection, from: i64, to: i64, rules: &Rules) -> rusqlite::Result<Vec<u8>>`
  - `pub fn seconds_on(con: &Connection, day: i64) -> f64`

- [ ] **Step 1: Write failing tests**, fixture from `test_tracker.py` lines 46–84 on `open(":memory:")`-equivalent (`Connection::open_in_memory` + schema):

```rust
#[test] fn stats_carve_sites_out_of_browser() {
    // rows: 2026-09-30 h9 app code.exe 600, app chrome.exe 300, site youtube.com 200; 2026-09-29 h9 app code.exe 100
    // RULES: code.exe→Development, chrome.exe→Browsing, youtube.com→Streaming
    let d = parse_iso("2026-09-30").unwrap();
    let day = stats(&con, d, d, prior(d, d), &rules).unwrap();
    assert_eq!(day["total"], 900); assert_eq!(day["previous"], 100);
    assert_eq!(day["buckets"].as_array().unwrap().len(), 24);
    assert_eq!(day["buckets"][9]["categories"], json!({"Development": 600, "Streaming": 200, "Browsing": 100}));
    assert_eq!(day["apps"][0], json!({"name": "code.exe", "seconds": 600, "category": "Development"}));
    assert_eq!(day["sites"], json!([{"name": "youtube.com", "seconds": 200, "category": "Streaming"}]));
    assert_eq!(day["previous_categories"], json!({"Development": 100}));
    assert_eq!(day["focus"]["days"], 0);
    let mon = parse_iso("2026-09-28").unwrap();
    let week = stats(&con, mon, mon + 6, prior(mon, mon + 6), &rules).unwrap();
    assert_eq!(week["buckets"][0]["label"], "2026-09-28"); assert_eq!(week["total"], 1000); assert_eq!(week["previous"], 0);
}
#[test] fn stats_uses_given_previous_span() { /* from=Mon..Wed, before=(prev Mon..prev Wed): previous counts only those 3 days */ }
#[test] fn csv_escapes_formulas() { /* header "day,hour,kind,name,category,seconds"; "2026-09-30,9,site,youtube.com,Streaming,200";
    row app "=evil.exe" 5 at h10 → "2026-09-30,10,app,'=evil.exe,Other,5"; output starts with BOM EF BB BF */ }
#[test] fn flush_upserts_and_clears() { /* flush twice 2 s of code.exe → usage seconds 4; daily switches add, longest max, first min, last max */ }
#[test] fn failed_flush_keeps_pending() { /* con with `PRAGMA query_only = ON` → flush Err, p not empty; turn off → flush Ok, written once */ }
#[test] fn pending_splits_days() { /* credit day D 23:59 then D+1 00:00 → two daily rows, D+1.first == "00:00" */ }
```

- [ ] **Step 2:** `cargo test wellbeing::` → new tests FAIL.
- [ ] **Step 3:** Implement. `stats` follows `tracker.py` `aggregate`+`stats` (hourly when `from == to`, top 12, `_clean` rounding drops ≤ 0, browser category = `classify(App, "chrome.exe")`), except the previous span comes from `before`. CSV via manual quoting (`"` around fields containing `,` `"` or newline, doubled quotes), no csv crate.
- [ ] **Step 4:** `cargo test` → PASS; clippy clean.
- [ ] **Step 5:** `git commit -m "feat(screen-time): SQLite storage, stats and CSV export"`

### Task 3: Config, categories, sampler thread

**Files:**
- Modify: `src/config.rs` (`ScreenTime` section + default text block), `src/focus.rs` (`BUILT_IN`), `src/wellbeing.rs`, `src/main.rs` (start/stop, quit flush), `src/win.rs` (`local_now`)

**Interfaces:**
- Consumes: Tasks 1–2; `apps::exe_path(pid)`; `focus::categories(&cfg.focus_category)`.
- Produces:
  - `config::ScreenTime { enabled: bool (false), productive: Vec<String> (["Development","Productivity"]), distracting: Vec<String> (["Social","Streaming","Gaming"]), weekly_summary: bool (true) }` as `cfg.screen_time`
  - `pub fn rules(cats: &[FocusCategory]) -> Rules` (lower-case names; first category in order wins; `Other` category's entries map to `"Other"`)
  - `pub fn kinds(st: &config::ScreenTime) -> serde_json::Value` (`{cat: "productive"|"distracting"}`)
  - `pub fn db_path() -> PathBuf`
  - `pub fn win::local_now() -> (i64, u8, String)` (day number, hour, `"HH:MM"`)
  - `pub struct Tab { pub host: String, pub audible: bool }`; `pub static TAB: RwLock<Option<Tab>>`
  - `pub struct Tracker` with `pub fn start(tx: UnboundedSender<Msg>, browsers: browser::Browsers, rules: Arc<RwLock<Rules>>, st: Arc<RwLock<config::ScreenTime>>) -> Tracker` and `pub fn stop(self)` (signals, joins, final flush)
  - `pub fn today_seconds() -> f64` (live, from the sampler)
  - `pub fn backup(con: &Connection, folder: &Path, day: i64) -> rusqlite::Result<()>` (one copy per day via the backup API, keeps the newest 7)

- [ ] **Step 1: Failing tests**:

```rust
#[test] fn rules_first_category_wins() { /* cats [Social{sites:[reddit.com]}, Streaming{sites:[reddit.com, youtube.com]}] → reddit.com→Social, youtube.com→Streaming */ }
#[test] fn rules_other_overrides_suffix() { /* Streaming{youtube.com}, Other{m.youtube.com} → classify(Site,"m.youtube.com")=="Other", "www2.youtube.com"→Streaming */ }
#[test] fn built_ins_include_screen_time_categories() { /* focus::categories(&[]) names contain Development, Productivity, Browsing, Other; chrome.exe → Browsing */ }
#[test] fn backup_keeps_seven() { /* temp folder; backup for 9 consecutive days → 7 files, oldest two gone; same day twice → no second copy */ }
#[test] fn screen_time_defaults() { /* config::parse("") → screen_time.enabled == false, distracting == ["Social","Streaming","Gaming"] */ }
```

- [ ] **Step 2:** `cargo test` → FAIL.
- [ ] **Step 3:** Implement.
  - `BUILT_IN` gains `Development` (`g::CODE`), `Productivity` (`g::BRIEFCASE`), `Browsing` (`g::GLOBE`) with micro bot's `categories.json` lists for those three verbatim, and `Other` (`g::CIRCLE_HELP`, empty). Pick the closest existing glyph if a name differs.
  - Sampler thread: loop every 2 s: `dt` from `Instant`; exe via `GetForegroundWindow` → `GetWindowThreadProcessId` → `apps::exe_path` → file name lower-case (`""` no window, `"system"` when unreadable); idle via `GetLastInputInfo`/`GetTickCount` wrapping; host/audible from `TAB` only when exe ∈ browsers, and when it is, `browsers.ask_all(json!({"t":"active"}))` for the next sample. `credit` into `Pending`; every 30 s open DB and `flush` (log nothing on error, retry next time); once per day after a flush: `backup` (Task 6 adds the weekly-summary call right after it).
  - `main.rs`: start when `cfg.screen_time.enabled`; on config reload update `rules`/`st`, start/stop when `enabled` flips; on quit `stop()`.
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: `enabled = true`, use VS Code 1 min, `sqlite3 %APPDATA%\procast\screentime.db "select * from usage"` shows ~60 s of `code.exe` after ≤ 30 s.
- [ ] **Step 5:** `git commit -m "feat(screen-time): sampler thread, [screen_time] config, shared categories"`

### Task 4: Active tab from the browser companion

**Files:**
- Modify: `browser/background.js` (+ `manifest.json` version bump), `src/browser.rs` (`Reply::Active`, `parse_reply`), `src/main.rs` (`Msg::Browser` arm)
- Test: `browser/active.test.mjs`, `src/browser.rs` tests

**Interfaces:**
- Consumes: `wellbeing::TAB`, `wellbeing::Tab`.
- Produces: `browser::Reply::Active { focused: bool, host: String, audible: bool }`; request `{t:"active"}` → `{t:"active", browser, focused, host, audible}`.

- [ ] **Step 1: Failing tests**:

```rust
#[test] fn active_reply_parses_and_validates() {
    // {"t":"active","browser":"Edge","focused":true,"host":"WWW.GitHub.com","audible":false} → Active{focused:true, host:"github.com", audible:false}
    // host "evil host/../" → Active{host:""}
}
#[test] fn active_error_is_silent() {
    // {"t":"active","error":"unknown request"} → Reply::Active{focused:false, host:"", audible:false}, never Reply::Done(Err)
}
```

`browser/active.test.mjs` (pattern of `ytmusic.test.mjs`): stub `chrome.windows.getLastFocused` → `{focused:true, tabs:[{active:true, url:"https://www.youtube.com/watch?v=1", audible:true}]}` → reply `{t:"active", focused:true, host:"youtube.com", audible:true}`; `url:"chrome://newtab"` → `host:""`.

- [ ] **Step 2:** `cargo test browser::` and `node browser/active.test.mjs` → FAIL.
- [ ] **Step 3:** JS: `case "active"` using `chrome.windows.getLastFocused({populate:true})`, the active tab, hostname rule from Global Constraints. Rust: `parse_reply` checks `t == "active"` **before** the generic `error` branch. `main.rs`: on `Reply::Active` with `focused`, write `TAB`; a non-focused answer never overwrites a focused one from the same round (keep it simple: last focused answer wins).
- [ ] **Step 4:** both tests PASS; clippy clean. Manual: reload unpacked extension, YouTube in Edge 1 min → `usage` has `site youtube.com`.
- [ ] **Step 5:** `git commit -m "feat(browser): report the active tab's site for Screen Time"`

### Task 5: Moving a name between categories

**Files:**
- Modify: `src/wellbeing.rs`

**Interfaces:**
- Consumes: `config::edit_file`, `focus::categories`, `toml_edit`.
- Produces: `pub fn move_name(text: &str, cats: &[FocusCategory], name: &str, category: &str) -> Result<String, String>` (returns new config text).

- [ ] **Step 1: Failing tests**:

```rust
#[test] fn move_name_rewrites_categories() {
    // "" + built-ins; move "youtube.com" to "Development" → parsed cfg: Streaming (override written) lacks youtube.com, Development has it
    // move to "Other" → Other has it, no other category does
    // move "x" to "Nope" → Err("unknown category")
    // name "" or > 260 chars or with control chars → Err("bad name")
    // a comment line in the input text survives
}
```

- [ ] **Step 2:** FAIL. **Step 3:** implement as `tracker.py` `recategorize`: remove from every category (built-ins touched become `[[focus_category]]` overrides, existing mechanism), add to target. **Step 4:** PASS, clippy. **Step 5:** `git commit -m "feat(screen-time): move an app or site to another category"`

### Task 6: Import from micro bot, weekly summary

**Files:**
- Modify: `src/wellbeing.rs`, `src/commands.rs` (`Cmd::ImportScreenTime`), `src/main.rs` (form + background run + toast)
- Modify (other repo): `D:\Pro\micro bot\README.md` (top note: moved into procast)

**Interfaces:**
- Consumes: Tasks 2, 3, 5; `focus::add_categories`; `config::write_value`; `Msg::sticky(Kind::Info, ..)`.
- Produces:
  - `pub struct Imported { pub rows: usize, pub days: usize, pub names_added: usize }`
  - `pub fn import_db(con: &mut Connection, from: &Path, force: bool) -> Result<(usize, usize), String>`
  - `pub fn import_categories(text: &str, cats: &[FocusCategory], json: &str) -> Result<(String, usize), String>` (Focus wins)
  - `pub fn import_settings(text: &str, json: &str) -> Result<String, String>` (maps names, sets `enabled = true`)
  - `pub const RENAME: &[(&str, &str)] = &[("Communication","Messaging"),("Entertainment","Streaming"),("Games","Gaming")]`
  - `pub fn weekly_summary(con: &Connection, today: i64, rules: &Rules, on: bool) -> Option<String>`

- [ ] **Step 1: Failing tests**:

```rust
#[test] fn import_db_adds_rows() { /* source file DB with usage+daily rows; target has one overlapping key → seconds added, daily merged; returns (rows, days); meta.imported_from set */ }
#[test] fn import_db_rejects_wrong_file() { /* missing file → Err containing "data.db"; a SQLite file without `usage` → Err("not a micro bot database"); target unchanged */ }
#[test] fn import_db_second_run_warns() { /* meta.imported_from present → Err starting "Already imported" unless force flag */ }
#[test] fn import_categories_focus_wins() {
    // json {"Entertainment":["reddit.com","crunchyroll.com"],"Communication":["slack.com","zoom.us"]}
    // → reddit.com stays Social only; crunchyroll.com added to Streaming; slack.com stays Messaging; zoom.us added to Messaging
}
#[test] fn import_settings_maps_names() { /* {"productive":["Development"],"distracting":["Entertainment","Games"],"weekly_summary":false} → distracting ["Streaming","Gaming"], weekly_summary false, enabled true */ }
#[test] fn weekly_summary_once_per_week() { /* Monday with last week 3600 s → Some(text); second call same week → None; meta.summary_week set; total < 60 → None but week recorded; on=false → None */ }
```

- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Implement. `import_db`: `ATTACH ?1 AS mb`, check `mb.usage` exists, upsert in one transaction, `DETACH`. Command form: one folder field (default `D:\Pro\micro bot`), note "Running it twice adds the hours twice". On submit, in background: `schtasks /End /TN WellbeingTracker`, `schtasks /Change /TN WellbeingTracker /Disable` (a missing task is fine), import DB, categories, settings via `config::edit_file`; toast `Imported N days of screen time`; errors as error toast. Weekly summary: sampler calls it after the daily backup and sends `Msg::sticky(toast::Kind::Info, text)`.
- [ ] **Step 4:** PASS, clippy. Manual: run import on the real folder; `schtasks /Query /TN WellbeingTracker` shows Disabled; counts match `sqlite3 "D:\Pro\micro bot\data.db" "select count(*) from usage"`.
- [ ] **Step 5:** `git commit -m "feat(screen-time): import micro bot history and categories; weekly summary toast"`; in micro bot: `git commit -m "docs: point to procast"` (only `README.md`).

### Task 7: Dashboard server

**Files:**
- Create: `src/wellbeing_http.rs`, `assets/screen-time/dashboard.html` (copy of micro bot's, two edits)
- Modify: `src/main.rs` (start with the tracker)

**Interfaces:**
- Consumes: Tasks 2, 3, 5.
- Produces:
  - `pub struct Request { pub method: String, pub path: String, pub query: String, pub host: String, pub origin: String, pub body: Vec<u8> }`
  - `pub struct Response { pub code: u16, pub ctype: &'static str, pub body: Vec<u8>, pub filename: Option<String> }`
  - `pub fn route(req: &Request, db: &Path, cfg_path: &Path) -> Response` (pure enough to test with a temp DB and config)
  - `pub fn parse_request(r: &mut impl BufRead) -> Option<Request>` (≤ 8 KiB headers, `Content-Length` ≤ 1024 else 400)
  - `pub fn serve(tx: UnboundedSender<Msg>, flush: impl Fn() + Send + Sync + 'static)` (bind failure → one `Msg::error("Port 8737 is in use: is micro bot still running?")`)

- [ ] **Step 1: Failing tests**:

```rust
#[test] fn host_guard() { /* Host "evil.com:8737" → 403 on GET / and /api/stats */ }
#[test] fn origin_guard() { /* POST /api/category Origin "https://evil.com" → 403; Origin "http://127.0.0.1:8737" with {"name":"x.com","category":"Development"} → 204 and config changed */ }
#[test] fn span_and_dates() { /* from>to, span 401, bad date, missing param → 400; 0 and 400 → 200 */ }
#[test] fn stats_body_has_options_and_kinds() { /* /api/stats → has "options" (category names in order) and "kinds" */ }
#[test] fn csv_download() { /* /api/export.csv → text/csv, filename "screen-time-2026-09-30-to-2026-09-30.csv" */ }
#[test] fn parse_rejects_big_body() { /* Content-Length 5000 → parse_request yields a request that route answers 400 */ }
#[test] fn unknown_path_404() { /* GET /tab → 404 */ }
```

- [ ] **Step 2:** FAIL.
- [ ] **Step 3:** Implement. One thread per connection, `set_read_timeout(5 s)`, `Connection: close`, `Cache-Control: no-store`, `Content-Disposition` when `filename`. Every API GET calls `flush` first (as `tracker.py`). Dashboard edits: `SLOTS = ["Development","Productivity","Messaging","Social","Streaming","Gaming","Browsing"]`; "categories.json isn't read-only" → "config.toml isn't read-only". Embed with `include_str!`.
- [ ] **Step 4:** PASS, clippy. Manual: open `http://127.0.0.1:8737`, day/week/month views render, recategorize from the dashboard updates `config.toml`.
- [ ] **Step 5:** `git commit -m "feat(screen-time): serve the dashboard on 127.0.0.1:8737"`

### Task 8: Launcher commands and README

**Files:**
- Modify: `src/commands.rs` (`Cmd::ScreenTime`, `Cmd::ScreenTimeWeek`, `Cmd::OpenScreenTimeDashboard`; names/keywords/glyphs in the table at ~line 215, group "Screen Time"), `src/main.rs` (mode, rows, actions), `README.md` (Built-in tools bullet + feature table row)

**Interfaces:**
- Consumes: `stats` (Task 2), `move_name` (Task 5), `export_csv` (Task 2), `win::open`.
- Produces: `Mode::ScreenTime(Range)` where `enum Range { Today, Week }` (or the builder's equivalent, spec leaves wiring open).

- [ ] **Step 1: Failing test** for the pure part: `pub fn rows(stats: &Value) -> Vec<Row>` with `Row { section: &'static str, name: String, seconds: u64, category: String }`:

```rust
#[test] fn rows_from_stats() { /* fixture stats → first section "Categories" sorted by seconds desc, then "Apps", then "Sites"; header text "15m today · 5m more than yesterday" via pub fn header(total, previous, Range) */ }
#[test] fn week_compares_same_weekdays() { /* Range::Week on Wed 2026-09-30 → span (Mon 09-28, Wed 09-30), before (09-21, 09-23) via pub fn span(range, today) -> ((i64,i64),(i64,i64)) */ }
```

- [ ] **Step 2:** FAIL. **Step 3:** implement; stats load on `background_executor`; list + detail pane (row's per-hour numbers for today's view, totals for week); Ctrl+K: Move to Category… (picker of `focus::categories` names, then `move_name` via `config::edit_file`), Open Dashboard, Export CSV (to `%USERPROFILE%\Downloads\screen-time-<from>-to-<to>.csv`). Disabled Screen Time: rows replaced by one row "Screen Time is off" with action Import from micro bot / Turn on.
- [ ] **Step 4:** PASS, clippy. Manual: Alt+Space, "screen time" → today's list; Ctrl+K → Move to Category works and the list re-sorts.
- [ ] **Step 5:** `git commit -m "feat(screen-time): launcher views and README"`

### Task 9: Cutover check

- [ ] **Step 1:** Release build, run for a day with micro bot disabled.
- [ ] **Step 2:** Compare a day against micro bot's last day of habit: totals within a few minutes, no browser double counting (category sum == total).
- [ ] **Step 3:** User removes micro bot's own browser extension; README's companion section says to reload procast's.
