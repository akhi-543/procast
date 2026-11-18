# Built-in Tools (Roadmap O1–O8) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship roadmap rows O1–O8 (Raycast's built-in tools) in procast, matching how Raycast for Windows 2.12.1 behaves where Raycast has the feature.

**Architecture:** Each tool is one or more `Cmd`s in `src/commands.rs` plus, where it needs state, one new module. Reuse first: Windows OCR (`clipboard::recognise`), the browser companion (`src/browser.rs`), `toast::show`, the clipboard split view, the Node sidecar (`exthost/`). One shared foreground watcher (`src/foreground.rs`) serves Focus (O6) and Auto HDR (O8). Translate uses DeepL (user key); Define runs locally (WordNet).

**Tech Stack:** Rust 2024, gpui-ce 0.3, `windows` 0.62, Node sidecar. New only where a task names it.

**Spec:** roadmap §O in [docs/roadmap.md](../../roadmap.md), plus the Raycast reference below (reverse-engineered 2026-11-15 from `C:\Program Files\WindowsApps\Raycast.Raycast_2.12.1.0_x64__qypenmj9wpt2a\Raycast`: `backend/index.mjs`, `frontend/*-settings-view-*.js`, `frontend/startup-Bt9nf1IW.js`, `Raycast.dll` string heap).

## Raycast reference (observed)

| Feature | What Raycast 2.12.1 does | Source |
|---|---|---|
| Notes | Commands: *Raycast Notes* (open window), *Search Notes*, *Create Note* (fallback), pinned notes as root rows (max 10). Editor: TipTap/ProseMirror rich text. Formats: Bold `Mod-b`, Italic `Mod-i`, Underline `Mod-u`, Strike `Mod-Shift-s`, Code `Mod-e`, Highlight `Mod-Shift-m`, Quote `Mod-Shift-b`, Code block `Mod-Alt-c`, Paragraph `Mod-Alt-0`, H1–H3, bullet / numbered / task lists, links; paste plain `Mod-Shift-v`; undo/redo. Window: top bar with title, browse-notes menu, *Toggle Format Bar*, *Zoom In/Out*, *New Note* ("You already have an empty note"), *Pin/Unpin*, *Save as Duplicate*, *Copy* (Markdown), *Show History*, *Delete* → *Recently Deleted* → *Restore* / *Permanently Delete*. Settings: show pinned notes in root search (off), Esc *closes window* (default) or *loses focus*, hide while screen sharing (off), move checked tasks down (off), zoom (1). | backend `key:'notes'`, notes-settings-view, startup bundle |
| Translator | Cloud only: POST `{raycast}/api/v1/translations` `{format:'text', q, source, target}` → `data.translations[].translatedText, detectedSourceLanguage` (Google Cloud Translation v2 shape — inferred Google behind Raycast's proxy). Pro-only, sign-in. Settings: primary action (copy / paste), prefill from selected text, default source (auto) and target. Custom "Translate to X" commands: name, source, target, output = *Open in Raycast* or *Replace Selection*; own hotkeys. | backend `translations`, translator-settings-view |
| Dictionary | **macOS only** (`availability:{macos}`), "Define a word using macOS Dictionary". Not on Windows. | backend `defineWord` |
| Screenshots | *Search Screenshots* (grid, 3 columns, categories filter, primary action copy / paste / open), *Paste Latest Screenshot*. Settings: search directories, include all media (off), text recognition (on), OCR level, storage duration (unlimited). | backend `key:'screenshots'` |
| Focus | Session: title, duration (0 = no limit), mode **block** (blocked apps + websites) or **allow** (allowed apps + websites), categories (create, search, import JSON). Actions: start, toggle (last settings), pause, resume, complete, cancel, restart, edit, snooze; shown as a root row and in the tray. Settings: snooze 1 / **3** / 5 / 10 min / none, play sound (on), restore apps when session ends (off), allow all sites when allowlist empty (off). Enforcement: graceful quit of blocked app ("Requesting graceful quit for blocked app") + overlay window; websites: polls browser state, reads Chromium address bar via UI Automation (`OmniboxViewViews`), navigates away, page says "<site> has been blocked by Raycast". | backend focus ext, `Raycast.dll` literals |
| Calendar | macOS extension uses EventKit; Windows reads Outlook via Microsoft Graph `Calendars.Read` (MSAL sign-in). My Schedule, Join Meeting, confirm before joining (on), event display tolerance 600 s, disabled calendars. | backend calendar ext, Graph scopes |
| Auto HDR | *Games* extension (Windows only): game launcher (Steam, Epic, Xbox, Riot…), settings *Game Mode* and *Auto HDR* (both off). `AutoHDRService`: `IsGameWindowInForeground` → `DisplayConfigSetAdvancedColorState`; logs "Auto HDR: HDR off". Also `system.toggleHdr` command, deep links `ms-settings:display-hdr`, `ms-settings:gaming-gamemode`. | backend `key:'games'`, `Raycast.dll` |
| Timer | No built-in (store extension only). Our own design. | backend search |

## Global Constraints

- Zero warnings, zero clippy (`cargo clippy --all-targets -- -D warnings`), `cargo test` green before every commit. Stop `procast.exe` before building, restart after.
- Design system first: each new visual (notes window + format bar, focus overlay + block page, timer row, screenshot grid, calendar row) is mocked in the Design System artifact (https://claude.ai/artifact/41jrDsMRQpkkm5fnMMRWPc) and approved before code. No purple; neutral grey selection; Sticker icon tiles.
- New icons: Lucide 0.460.0 only, listed in `assets/icons/README.md`.
- Every network or OS call fails soft: toast, never a panic or UI-thread block.
- Copy Raycast's behaviour, names and defaults; never its code, art or strings beyond command titles.
- New config keys get a commented line in the `config.toml` template in `src/config.rs`.
- One commit per task, `feat(<area>): … (O<n>)`.

## Order

| # | ID | Size |
|---|---|---|
| 1 | O7 Timer / stopwatch | S |
| 2 | O3 Translator (DeepL) | S |
| 3 | O4 Define word (local) | S |
| 4 | O2 Screenshot search | M |
| 5 | Foreground watcher (shared) | S |
| 6 | O8 Auto HDR + Toggle HDR | S |
| 7 | O6 Focus | M |
| 8 | O5 Calendar | M |
| 9–11 | O1 Notes (store, editor, window) | L |

## Decisions (user confirms before the task starts)

- **O3 Translator — Decided (2026-11-15): DeepL API.** Key in `%APPDATA%\procast\config.toml` `[translator] deepl_key`; keys ending `:fx` use `api-free.deepl.com`, others `api.deepl.com`.
- **O4 Define — Default (user did not object): WordNet.** Windows has **no built-in dictionary API** (only `ISpellChecker` spelling suggestions). **(a, recommended)** WordNet 3.1 (Princeton licence), English, offline, ~12 MB, downloaded on first use to `%APPDATA%\procast\wordnet\`, gives senses, examples, synonyms. (b) Wiktionary extract (kaikki.org, CC BY-SA), many languages, hundreds of MB. (c) dictionaryapi.dev, online, no key, English. Plan: (a); spelling suggestions from `ISpellChecker` when the word isn't found.
- **O2 folders — Default:** default `%USERPROFILE%\Pictures\Screenshots`; retention unlimited.
- **O5 account — Default ICS:** (a) secret iCal (ICS) URL, no sign-in; or (b) Microsoft Graph like Raycast (needs an Azure app registration of yours, public client, `Calendars.Read`). Plan: (a).
- **O1 editor:** Raycast is WYSIWYG rich text. gpui-ce ships no editor, so procast builds one: markdown stored, rendered live (headings, bold, italic, strike, code, quote, lists, task checkboxes) with Raycast's shortcuts. Underline and highlight have no markdown form: stored as `<u>…</u>` / `==…==`.

## Review Focus

1. Timer that ends while the PC slept: toast fires once on wake (Task 1 test on `Timers::due`).
2. Translator with no key, a wrong key (403), quota used up (456) or offline: a toast naming the fix ("Add deepl_key to config.toml", "DeepL quota used up"), launcher usable (Task 2 test on `translate::error_text`).
3. Screenshot folder with thousands of images or a corrupt PNG: indexing off the UI thread, bad files skipped and not retried (Task 4 test on `ShotIndex::update`).
4. HDR left on after procast crashes mid-game: restored on next start from the "we turned it on" flag; HDR the user turned on is never turned off (Task 6 test on `hdr::next`).
5. Focus blocking procast, Explorer or the shell, or allow mode with an empty app list: shell is never blocked; empty allowlist blocks nothing but procast's own prompt (Task 7 test on `focus::blocked`).

---

### Task 1: O7 — Timer and stopwatch

**Files:** Create `src/timers.rs`; modify `src/commands.rs` (`Cmd::StartTimer`, `Cmd::StartStopwatch`), `src/main.rs` (root rows, 1 s tick), `src/search.rs` (`timer 5m tea`).

**Interfaces:**
- Produces: `pub fn parse_duration(s: &str) -> Option<Duration>` (`90`, `90s`, `5m`, `1h30m`, `1:30`); `pub struct Timers` with `start(&mut self, label: String, d: Duration) -> u64`, `stopwatch(&mut self) -> u64`, `cancel(&mut self, id: u64)`, `due(&mut self, now: SystemTime) -> Vec<(u64, String)>` (each id once), `rows(&self, now: SystemTime) -> Vec<(u64, String, String)>`. Wall clock (`SystemTime`) so sleep counts. Persist to `%APPDATA%\procast\timers.json` so a restart keeps them.

- [ ] **Step 1:** Tests: `parse_duration("5m") == Some(300s)`, `("1h30m") == 5400s`, `("1:30") == 90s`, `("0")`/`("abc") == None`; `due` returns an id once; a timer already past at `due` fires once; save → load round-trips.
- [ ] **Step 2:** `cargo test timers` — FAIL.
- [ ] **Step 3:** Implement. Root rows for running timers (Enter cancels / stops and copies elapsed). Tick only while one runs; done → `toast::show(…, "Timer done: tea", …)` + system sound.
- [ ] **Step 4:** `cargo test` PASS, clippy clean; manual 5 s timer.
- [ ] **Step 5:** Commit `feat(timers): timer and stopwatch (O7)`.

### Task 2: O3 — Translator (DeepL)

**Files:** Create `src/translate.rs`; modify `src/commands.rs` (`Cmd::Translate`, custom `[[translate]]` commands), `src/search.rs` (`translate hola`, `tr hola`), `src/main.rs` (result view, off-UI-thread request), `src/config.rs` (`[translator] deepl_key = ""`, `from = "auto"`, `to = "EN-US"`, `primary = "copy"`, `prefill_selection = false`; `[[translate]] name, from, to, output = "open" | "replace"`).

**Interfaces:**
- `pub fn endpoint(key: &str) -> &'static str`: `https://api-free.deepl.com/v2/translate` when key ends `:fx`, else `https://api.deepl.com/v2/translate`.
- `pub fn translate(key: &str, text: &str, from: &str, to: &str) -> Result<Translation, String>`: `ureq` POST, header `Authorization: DeepL-Auth-Key <key>`, JSON `{"text":[text],"target_lang":to}` plus `"source_lang":from` unless `auto`. Reply `{"translations":[{"detected_source_language","text"}]}`. `pub struct Translation { text: String, detected: String }`.
- `pub fn error_text(status: u16) -> String`: 403 → "DeepL key rejected: check deepl_key in config.toml"; 456 → "DeepL quota used up this month"; 429 → "DeepL busy, try again"; empty key → "Add deepl_key to config.toml ([translator])".
- `pub const TARGETS: &[(&str, &str)]` (DeepL target codes + names, from `GET /v2/languages?type=target` as of 2026-11; includes `EN-US`, `EN-GB`, `PT-BR`, `PT-PT`, `ZH-HANS`, `ZH-HANT`).
- Query forms: `translate hola`, `tr hola`, `tr de hola` (first word a known code → target), empty → selected text when `prefill_selection`. Custom commands appear as "Translate to German"; `output = "replace"` pastes over the selection (reuse snippet paste). Result: Enter = `primary` (copy or paste).

- [ ] **Step 1:** Tests: `endpoint("abc:fx")` is free host, `endpoint("abc")` pro host; request JSON omits `source_lang` for `auto`; reply parse of a fixture string; `error_text(403/456/429)`; query parse `tr de hola` → (`DE`, `hola`); custom command expansion.
- [ ] **Step 2:** `cargo test translate` — FAIL.
- [ ] **Step 3:** Implement. No network in tests: split `build_request`/`parse_reply` from the HTTP call.
- [ ] **Step 4:** `cargo test` PASS, clippy clean; manual with a real key (user adds it).
- [ ] **Step 5:** Commit `feat(translate): DeepL translator with custom commands (O3)`.

### Task 3: O4 — Define word (local)

**Files:** Create `src/dict.rs`; modify `src/commands.rs` (`Cmd::Define`), `src/search.rs` (`define <word>` and fallback row), `src/config.rs` (`[dictionary] enabled = true`).

**Interfaces:**
- `pub struct WordNet` loaded from WordNet 3.1 `dict/` files (`index.*`, `data.*`, `*.exc` for inflections), binary-search the index by lemma. `pub fn lookup(&self, word: &str) -> Vec<Sense>`; `pub struct Sense { pos: Pos, gloss: String, examples: Vec<String>, synonyms: Vec<String> }`. Morphology: try `*.exc` then suffix rules (`-s`, `-es`, `-ed`, `-ing`, `-ies→y`).
- Download on first use (one zip) to `%APPDATA%\procast\wordnet\`; show progress row.
- Not found → spelling suggestions via `ISpellChecker` (`Win32_Globalization` feature): "Did you mean …".
- View reuses the Ai/Output detail view: senses grouped by part of speech, Enter copies the first gloss.

- [ ] **Step 1:** Tests on a fixture slice of WordNet files: `lookup("dog")` has a noun sense containing "domesticated"; `lookup("ran")` resolves to `run`; `lookup("cats")` → `cat`; unknown → empty.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit `feat(dict): offline define word with WordNet (O4)`.

### Task 4: O2 — Screenshot search

**Files:** Create `src/shots.rs`; modify `src/clipboard.rs` (`recognise` → `pub(crate)`), `src/config.rs` (`[screenshots] folders`, `keep = "unlimited"`, `ocr = true`, `all_media = false`, `primary = "copy"`), `src/commands.rs` (`Cmd::SearchScreenshots`, `Cmd::PasteLatestScreenshot`), `src/main.rs` (`Mode::Shots`).

**Interfaces:**
- `pub struct ShotIndex { entries: Vec<Shot> }`, `pub struct Shot { path: PathBuf, mtime: u64, text: String, failed: bool }`; `load() -> Self` from `%APPDATA%\procast\screenshots.json`; `update(&mut self, folders: &[PathBuf], ocr: impl Fn(&[u8]) -> Option<String>) -> usize` (new/changed only; OCR failure → `failed`); `search(&self, q: &str) -> Vec<&Shot>` (nucleo over OCR text + name; newest first when empty); `prune(&mut self, keep: Option<Duration>)`.
- UI: Raycast shows a 3-column grid; procast reuses the clipboard split view (list + image preview) unless the design system adds a grid. Primary action copy / paste / open per config.
- Rescan when the mode opens (ponytail: no folder watcher; add `ReadDirectoryChangesW` if it lags).

- [ ] **Step 1:** Tests with temp dir + fake OCR closure: new file indexed; unchanged mtime not re-OCR'd (call count); `None` → `failed`, not retried; deleted file dropped; `search("invoice")` hits OCR text; `prune` drops old entries.
- [ ] **Step 2–4:** Fail, implement, pass. Manual: Paste Latest Screenshot into Paint.
- [ ] **Step 5:** Commit `feat(shots): screenshot search and paste latest (O2)`.

### Task 5: Shared foreground watcher

**Files:** Create `src/foreground.rs`; modify `src/main.rs` (start once).

**Interfaces:**
- `pub fn watch(tx: UnboundedSender<Msg>)`: `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` on its own message-loop thread (pattern from `keys.rs`); sends `Msg::Foreground { hwnd: isize, pid: u32, exe: String, game: bool }`.
- `pub fn exe_of(hwnd: HWND) -> Option<String>` (lowercase file name). `pub fn is_game(exe: &str, d3d_full: bool, covers_monitor: bool, games: &[String]) -> bool`: listed exe, or `SHQueryUserNotificationState == QUNS_RUNNING_D3D_FULL_SCREEN`, or borderless window covering its monitor whose exe lives under a Steam / Epic / Xbox / Riot library folder.

- [ ] **Step 1:** Test `is_game`: listed (case-insensitive) → true; D3D full-screen → true; borderless full-monitor non-game (e.g. `chrome.exe` F11) → false; neither → false.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit `feat: foreground watcher for focus and HDR`.

### Task 6: O8 — Auto HDR and Toggle HDR

**Files:** Create `src/hdr.rs`; modify `Cargo.toml` (`Win32_Devices_Display`), `src/config.rs` (`[games] auto_hdr = false`, `list = []`), `src/commands.rs` (`Cmd::ToggleHdr`, `Cmd::HdrSettings` → `ms-settings:display-hdr`, `Cmd::GameModeSettings` → `ms-settings:gaming-gamemode`), `src/main.rs`.

**Interfaces:**
- `pub fn displays() -> Vec<HdrDisplay>` (`DisplayConfigGetDeviceInfo` + `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`: supported, enabled); `pub fn set(target: &HdrDisplay, on: bool) -> Result<(), String>` (`DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE`); monitor of the game window chosen by `MonitorFromWindow`.
- `pub fn next(auto: bool, game: bool, ours: bool, hdr_on: bool) -> Option<bool>`. Flag file `%APPDATA%\procast\hdr-on` while procast owns the on-state; on start, flag + no game → off.
- Toggle HDR flips every HDR-capable display; none capable → toast "No HDR display".

- [ ] **Step 1:** Tests on `next`: game + off → `Some(true)`; game gone + ours → `Some(false)`; user-enabled HDR never turned off; `auto == false` → `None`.
- [ ] **Step 2–4:** Fail, implement, pass. Manual on an HDR monitor.
- [ ] **Step 5:** Commit `feat(hdr): auto HDR for games and Toggle HDR (O8)`.

### Task 7: O6 — Focus sessions

**Files:** Create `src/focus.rs`; modify `src/commands.rs` (`Cmd::Focus(FocusCmd)`: Start, Toggle, Pause, Resume, Complete, Cancel, Restart, Edit, Snooze, SearchCategories, CreateCategory, ImportCategories), `src/config.rs` (`[focus] snooze_minutes = 3`, `sound = true`, `restore_apps = false`, `allow_all_on_empty_sites = false`; `[[focus_category]] name, apps, sites`), `src/main.rs`, `browser/` extension (`{"t":"focusSites", "mode", "sites"}` → redirect matching tabs to the extension's `blocked.html`, "<site> is blocked by procast Focus").

**Interfaces:**
- Consumes: `Msg::Foreground` (Task 5), `Timers` (Task 1), `Browsers::ask_all`.
- `pub enum Mode { Block, Allow }`; `pub struct Session { title: String, mode: Mode, apps: Vec<String>, sites: Vec<String>, until: Option<SystemTime>, paused_left: Option<Duration>, snoozed: HashMap<String, SystemTime> }`; `pub fn blocked_app(&self, exe: &str, now: SystemTime) -> bool` (never `procast.exe`, `explorer.exe`, `shellexperiencehost.exe`, `searchhost.exe`, `startmenuexperiencehost.exe`, `lockapp.exe`); `pub fn blocked_site(&self, host: &str, now: SystemTime) -> bool` (suffix match on domain); `pause`, `resume`, `snooze(&mut self, key: &str, mins: u8)` (1, 3, 5, 10).
- Enforcement like Raycast: blocked app in front → `WM_CLOSE` (graceful quit), remember it if `restore_apps`, show the focus overlay (Snooze / Close). Sites through the browser companion (procast already has it), not UI Automation. Session row in root (title, time left) with Raycast's actions; tray menu item while running. Session persists across restart (`focus.json`).
- Categories import: Raycast's JSON export `{name, apps, websites}` array.

- [ ] **Step 1:** Tests: block mode blocks listed exe during session, not after or while paused; allow mode blocks unlisted, never shell exes; snooze 3 min unblocks then re-blocks; site `www.reddit.com` blocked by `reddit.com`, `notreddit.com` not; import parses a sample JSON.
- [ ] **Step 2–4:** Fail, implement, pass. Manual: block notepad.exe and one site.
- [ ] **Step 5:** Commit `feat(focus): focus sessions with block and allow modes (O6)`.

### Task 8: O5 — Calendar (My Schedule)

**Files:** Create `src/calendar.rs`; modify `src/config.rs` (`[calendar] ics = []`, `tolerance_secs = 600`, `confirm_join = true`, `remind_minutes = 5`, `ignore = []`), `src/commands.rs` (`Cmd::MySchedule`), `src/main.rs`.

**Interfaces:**
- `pub struct Event { start: i64, end: i64, title: String, join: Option<String>, all_day: bool }`; `pub fn parse_ics(s: &str, from: i64, to: i64) -> Vec<Event>` (line unfolding, `DTSTART`/`DTEND` with `TZID`/`Z`/`VALUE=DATE`, `RRULE` DAILY/WEEKLY + `COUNT`/`UNTIL`/`BYDAY`, `EXDATE`); `pub fn join_link(text: &str) -> Option<String>` (Teams, Zoom, Meet, Webex). Fetch with `ureq` every 10 min off the UI thread.
- Rows: events within `tolerance_secs` of now as root rows when the bar is empty (Raycast: `rootSearch when:'empty'`); My Schedule lists today + tomorrow. Join asks "Join <title>?" when `confirm_join`. Reminder toast `remind_minutes` before; titles matching `ignore` skipped.
- Ponytail: other RRULE frequencies ignored; add when an event shows missing.

- [ ] **Step 1:** Tests with fixture ICS: UTC event; folded line; weekly RRULE with EXDATE; all-day event; Teams link from DESCRIPTION; ignore keyword.
- [ ] **Step 2–4:** Fail, implement, pass. Manual with your ICS URL.
- [ ] **Step 5:** Commit `feat(calendar): my schedule from ICS feeds (O5)`.

### Task 9: O1a — Notes store

**Files:** Create `src/notes/store.rs`.

**Interfaces:**
- Notes as `%APPDATA%\procast\notes\<id>.md`; metadata `notes.json`: `{id, pinned, created, modified, deleted_at}`; history snapshots in `history/<id>/<unix>.md` (one per 5 min of editing, keep 50).
- `pub struct Store`: `list(&self) -> Vec<NoteMeta>` (pinned first, then modified desc, excludes deleted), `create(&mut self) -> Result<Id, Exists(Id)>` (returns the existing empty note, like Raycast), `save(&mut self, id, text)`, `pin(id, bool)`, `duplicate(id) -> Id`, `delete(id)` (soft), `deleted() -> Vec<NoteMeta>`, `restore(id)`, `purge(id)`, `history(id) -> Vec<(i64, String)>`, `search(q) -> Vec<NoteMeta>`. Title = first line stripped of markdown.

- [ ] **Step 1:** Tests: create twice → second returns existing empty; delete → in `deleted()` not `list()`; restore; purge removes file; title from `# Hello **world**` = `Hello world`; history snapshot throttle.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit `feat(notes): note store with pin, history, recently deleted (O1)`.

### Task 10: O1b — Markdown editor core

**Files:** Create `src/notes/editor.rs`.

**Interfaces:**
- `pub struct Buffer { text: String, cursor: usize, anchor: usize }` (byte offsets on char boundaries); edits: `insert`, `backspace`, `delete`, `newline`, moves `left/right/up/down/home/end/word_left/word_right` with shift-select, `undo`, `redo` (coalesced by word).
- Raycast shortcuts → markdown: `toggle_mark(Mark)` wraps/unwraps selection (`**`, `*`, `<u>`, `~~`, `` ` ``, `==`); `set_block(Block)` rewrites line prefix (`# `, `## `, `### `, `> `, `- `, `1. `, `- [ ] `, fenced code); `toggle_task()` flips `[ ]`/`[x]` and, when `move_checked_down`, moves the line to the end of its list.
- Input rules like TipTap: typing `# ` at line start, `- `, `1. `, `[] ` converts to the block.
- Enter in a list continues it; Enter on an empty item ends it.
- `pub fn spans(line: &str) -> Vec<Span>` for live styling (heading level, bold, italic, code, strike, underline, highlight, link, task state).

- [ ] **Step 1:** Tests: backspace over emoji keeps UTF-8 valid; `toggle_mark(Bold)` on/off round-trips; `[] ` → `- [ ] `; Enter on `- a` → `\n- `; Enter on empty `- ` ends list; checked task moves down; undo restores the previous word.
- [ ] **Step 2–4:** Fail, implement, pass.
- [ ] **Step 5:** Commit `feat(notes): markdown editor core (O1)`.

### Task 11: O1c — Notes window and commands

**Precondition:** notes window, top bar, browse menu and format bar mocked and approved in the Design System artifact.

**Files:** Create `src/notes/mod.rs` (gpui view); modify `src/commands.rs` (`Cmd::Notes`, `Cmd::SearchNotes`, `Cmd::CreateNote`), `src/search.rs` (pinned notes as root rows when `show_pinned_in_root`), `src/config.rs` (`[notes] show_pinned_in_root = false`, `escape = "close"` | `"blur"`, `hide_when_sharing = false`, `move_checked_down = false`, `zoom = 1.0`, `hotkey = ""`).

**Interfaces:**
- Consumes `Store` (Task 9), `Buffer` + `spans` (Task 10).
- Floating window (always on top via `SetWindowPos(HWND_TOPMOST)`, as `toast.rs`), remembers size/position. Top bar: title, browse menu (`Ctrl+P`, search + pinned + recent), new note (`Ctrl+N`), actions menu (`Ctrl+K`: Pin, Duplicate, Copy Markdown, Show History, Delete, Recently Deleted, Toggle Format Bar, Zoom In/Out `Ctrl+=`/`Ctrl+-`). Autosave 500 ms after last keystroke.
- `hide_when_sharing` → `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
- Esc: `close` hides the window; `blur` returns focus to the previous app.

- [ ] **Step 1:** Tests: pinned notes appear as root rows only when enabled; Search Notes query matches body text; escape config parse.
- [ ] **Step 2–4:** Fail, implement, pass. Manual: note survives restart; window on top; capture excludes it when sharing.
- [ ] **Step 5:** Commit `feat(notes): floating notes window (O1)`.

### Task 12: O3b — Translator rework: Google (keyless), replaces DeepL

**Why:** user decision 2026-11-16: drop DeepL; build in the behaviour of the Raycast "Google Translate" store extension (installed at `%USERPROFILE%\.configaycast\extensions e43920f-…`), which needs no API key.

**Observed in that extension (reference, do not copy code):** GET `https://translate.google.com/translate_a/single` with query `client=dict-chrome-ex`, `sl=<from|auto>`, `tl=<to>`, `hl=<to>`, `dt=at,bd,ex,ld,md,qca,rw,rm,ss,t` (repeated `dt` params), `ie=UTF-8`, `oe=UTF-8`, `otf=1`, `ssel=0`, `tsel=0`, `kc=7`, `q=<text>`; when the URL exceeds 2048 chars, POST the same URL without `q` and send `q` as `application/x-www-form-urlencoded;charset=UTF-8` body. Reply is a nested JSON array: translation = concatenation of `r[0][i][0]` (non-null); detected source = `r[2]` (if it differs from `r[8][0][0]`, use `r[8][0][0]`); pronunciation = `r[0][1][2]` or `r[0][1][3]` when a string; spelling suggestion = `r[7][0]` with `<b><i>…</i></b>` → `[…]`, autocorrected when `r[7][5] == true`. Two-target rule: targets are a list; if the (detected) source matches the first target by base language (`en` = `en-GB`), translate to the second target instead. Commands there: Translate, Translate Form, Quick Translate (several targets at once), Instant Translate Copy / Paste / View (selected text). Preferences: from (auto), targets, auto input from selection, default action, proxy.

**Files:** modify `src/translate.rs` (replace the DeepL client), `src/config.rs` (`[translator]`: remove `deepl_key`; `from = "auto"`, `to = ["en"]` list, `primary`, `prefill_selection`; keep `[[translate]]` commands), `src/search.rs`/`src/commands.rs`/`src/main.rs` only where the DeepL types leak.

**Interfaces:**
- `pub fn build_url(text: &str, from: &str, to: &str) -> (String /*url*/, Option<String> /*form body when POST*/)`.
- `pub fn parse_reply(json: &str) -> Option<Translation>`; `Translation { text, detected, pronunciation: Option<String>, suggestion: Option<String> }`.
- `pub fn pick_target(source: &str, targets: &[String]) -> &str` (two-target rule).
- `pub fn translate(text, from, targets) -> Result<Translation, String>` off the UI thread; errors: offline → "Could not reach Google Translate"; HTTP 429 → "Google Translate is rate limiting, try again later"; unparsable → "Google Translate changed its reply".
- Language list: Google's codes (`auto`, `en`, `de`, `fr`, `es`, `zh-CN`, `zh-TW`, `pt`, `ja`, …) replace DeepL's `TARGETS`; `tr de hello` keeps working.
- New commands like the extension: "Quick Translate" (all configured targets at once, one row each) and "Translate Selection" (instant: selected text → `primary` copy/paste). Raycast titles must not parse as their own queries.

- [ ] **Step 1:** Tests: `build_url` GET for short text has every param above and URL-encodes `q`; text making URL > 2048 → POST with body `q=…`; `parse_reply` on a recorded fixture (multi-segment text, detected `de`, pronunciation, suggestion with `[…]`); `pick_target("en", ["en-GB","de"]) == "de"`; error mapping; config migration: an old config with `deepl_key` still loads.
- [ ] **Step 2–4:** Fail, implement, pass. Manual: `tr de good morning`.
- [ ] **Step 5:** Commit `feat(translate): keyless Google Translate replaces DeepL (O3)`.

### Wrap-up

- [ ] Tick O1–O8 in `docs/roadmap.md` and `TODO.md`, update README. Commit `docs: mark O1–O8 done`.
