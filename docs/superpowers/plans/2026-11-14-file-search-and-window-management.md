# File Search and Window Management (Roadmap J, K) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship roadmap rows J1–J4 (file search) and K1–K6 (window management) in procast.

**Architecture:** Window work stays in `src/commands.rs` (`Snap`, `layout`, `snap`, `run`) with a new `[window]` config section; virtual desktops get one new module `src/desktops.rs` over the `winvd` crate. File work extends both indexes (`src/files.rs` folder crawl, `src/ntfs.rs` whole-drive) with one shared exclusion check, and the launcher (`src/main.rs`) gains a File Search detail pane reusing clipboard's split view.

**Tech Stack:** Rust 2024, gpui-ce 0.3, `windows` 0.62, new: `winvd` (git, rev `ef74c24`), `ignore` (crates.io, gitignore matcher only).

**Spec:** [docs/superpowers/specs/2026-11-14-file-search-and-window-management-design.md](../specs/2026-11-14-file-search-and-window-management-design.md)

## Global Constraints

- Zero warnings, zero clippy (`cargo clippy --all-targets -- -D warnings`), `cargo test` green before every commit.
- No new visual components: J3/J4 reuse clipboard's split view, the Information rows and Settings rows. Anything new → stop, mock it in the Design System artifact first.
- Every virtual-desktop call fails soft: an error becomes a notice, never a panic or a hang.
- Virtual desktop commands only on build ≥ 22631 (Windows 11 23H2); below that they are not registered.
- Hidden-file and glob checks run at index time, never per keystroke.
- Config defaults: `files.gitignore = true`, `files.hidden = true`, `files.removable = false`, `window.gap = 0`, `window.screen_gap = 0`, `window.cycle = true`, `window.auto_close_empty_desktops = false`.
- New config keys get a commented line in the `config.toml` template in `src/config.rs` (the block around line 462).

## Review Focus

1. A lone wildcard in "Skip names" (`*`, `?`, `**`) would empty the index: ignore patterns made only of wildcards (Task 5 test).
2. Two monitors with different DPI: gaps are logical px scaled by the target monitor's DPI, not the primary's (Task 2 test on `Gaps::for_dpi`).
3. Cycling a maximized or minimized window: it is restored first, then gets the first size of the cycle, not the second (Task 3 test).
4. winvd failing on an unknown Windows build: the command shows a notice and procast keeps running (Task 11 test on `desktops::call`).
5. Detail pane on a locked, huge or deleted file: preview falls back to the icon, rows say "Not found" for a deleted file, nothing blocks the UI thread (Task 9 test).

---

### Task 1: K1 — more layouts

**Files:**
- Modify: `src/commands.rs` (`Snap` enum, `ALL`, `layout`)
- Test: `src/search.rs` (next to the existing layout test, ~line 757)

**Interfaces:**
- Produces: `Snap::{TopLeftQuarter, TopRightQuarter, BottomLeftQuarter, BottomRightQuarter, TopLeftSixth, TopCenterSixth, TopRightSixth, BottomLeftSixth, BottomCenterSixth, BottomRightSixth, TopThird, MiddleThird, BottomThird, ReasonableSize}`; `layout(s: Snap, wa: RECT, cur: RECT) -> RECT` unchanged signature.

- [ ] **Step 1: Write the failing test** `more_layouts_tile_the_work_area` with `wa = RECT{0,0,1200,900}`:
  - `TopLeftQuarter` = `(0,0,600,450)`, `BottomRightQuarter` = `(600,450,1200,900)`.
  - `TopCenterSixth` = `(400,0,800,450)`, `BottomRightSixth` = `(800,450,1200,900)`.
  - `TopThird` = `(0,0,1200,300)`, `MiddleThird` = `(0,300,1200,600)`, `BottomThird` = `(0,600,1200,900)`.
  - `ReasonableSize` = 720 × 630 centred: `(240,135,960,765)`.
  - With an odd width (1201), the right/bottom pieces end exactly on `wa.right`/`wa.bottom` (same rule as `RightHalf`).
- [ ] **Step 2:** `cargo test more_layouts` → FAIL (variants don't exist).
- [ ] **Step 3:** Add the variants, their `layout` arms, and `ALL` rows. Titles: "Top Left Quarter", "Top Right Quarter", "Bottom Left Quarter", "Bottom Right Quarter", "Top Left Sixth", "Top Center Sixth", "Top Right Sixth", "Bottom Left Sixth", "Bottom Center Sixth", "Bottom Right Sixth", "Top Third", "Middle Third", "Bottom Third", "Reasonable Size"; search words "window"; icons from existing `g::` glyphs.
- [ ] **Step 4:** `cargo test more_layouts` → PASS; `cargo clippy --all-targets -- -D warnings` → clean.
- [ ] **Step 5:** Commit `feat(window): quarters, sixths, horizontal thirds, reasonable size (K1)`.

### Task 2: K2 — gaps, `[window]` config, Settings → Window

**Files:**
- Modify: `src/config.rs` (new `Window` struct, `Config.window`, template), `src/commands.rs` (`run`, `snap`), `src/main.rs:1684` (pass config), `src/settings.rs` (`SECTIONS`, new "Window" arm)
- Test: `src/commands.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `config::Window { gap: u32, screen_gap: u32, cycle: bool, auto_close_empty_desktops: bool, layouts: Vec<Layout> }` (`Layout` added in Task 10; start with the four scalars).
  - `commands::Gaps { gap: i32, screen: i32 }` (physical px) with `Gaps::for_dpi(w: &config::Window, dpi: u32) -> Gaps`.
  - `commands::with_gaps(target: RECT, wa: RECT, g: Gaps) -> RECT`: shrink `wa` by `g.screen` first (callers lay out inside the shrunk area), then inset each edge of `target` not lying on the shrunk area's edge by `g.gap / 2`.
  - `commands::run(cmd: Cmd, target: HWND, w: &config::Window) -> Result<(), String>` (the `Result` is used from Task 4 on; return `Ok(())` for now).

- [ ] **Step 1: Write failing tests:**
  - `gaps_scale_with_dpi`: `Gaps::for_dpi(&Window{gap:10, screen_gap:4, ..}, 144)` = `Gaps{gap:15, screen:6}`.
  - `gaps_separate_neighbours`: `wa = (0,0,1200,900)`, `g = Gaps{gap:10, screen:0}`; `LeftHalf` → `(0,0,595,900)`, `RightHalf` → `(605,0,1200,900)`.
  - `screen_gap_insets_the_work_area`: `g = Gaps{gap:0, screen:8}`, `Maximize` is not passed through gaps; `AlmostMaximize` and `LeftHalf` stay inside `(8,8,1192,892)`.
- [ ] **Step 2:** `cargo test gaps` → FAIL.
- [ ] **Step 3:** Implement the struct, `for_dpi` (`v * dpi / 96`), `with_gaps`; `snap` gets the monitor DPI with `GetDpiForMonitor(here, MDT_EFFECTIVE_DPI)` and applies gaps to every `Snap` except `Maximize`, `Restore`, `NextDisplay`. Settings: add `"Window"` to `SECTIONS` (after `"Files"`) with rows `window.gap` (Number, "Space between snapped windows, in pixels."), `window.screen_gap` (Number, "Space between windows and the screen edges, in pixels.").
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: set gap 16, Left Half + Right Half on two windows, see a 16 px gap.
- [ ] **Step 5:** Commit `feat(window): window and screen gaps (K2)`.

### Task 3: K3 — cycling

**Files:**
- Modify: `src/commands.rs` (`snap`)
- Test: `src/commands.rs` tests

**Interfaces:**
- Consumes: `layout`, `with_gaps`, `Gaps` (Task 2).
- Produces: `commands::cycle_target(s: Snap, wa: RECT, vis: RECT, g: Gaps, cycle: bool) -> RECT` — the rect `snap` moves to.

Cycle order per command, fraction of the work area's width (Left/Right) or height (Top/Bottom), kept on the same edge: ½ → ⅔ → ⅓ → ½. "Already there" = every edge within 2 px of the step's rect (after gaps). Commands other than the four halves return `with_gaps(layout(..))`.

- [ ] **Step 1: Write failing tests** `left_half_cycles` (`wa=(0,0,1200,900)`, no gaps): from an unrelated `vis` → `(0,0,600,900)`; from `(0,0,600,900)` → `(0,0,800,900)`; from `(0,0,801,899)` → `(0,0,400,900)`; from `(0,0,400,900)` → `(0,0,600,900)`. `cycle_off_repeats_half`: `cycle=false` from `(0,0,600,900)` → `(0,0,600,900)`. `bottom_half_cycles_height`: from `(0,450,1200,900)` → `(0,300,1200,900)`.
- [ ] **Step 2:** `cargo test cycles` → FAIL.
- [ ] **Step 3:** Implement `cycle_target`; in `snap`, a maximized or minimized window is restored first and treated as "not in the cycle" (pass a `vis` that matches nothing). Settings → Window row `window.cycle` (Bool, "Pressing Left Half again makes the window ⅔, then ⅓.").
- [ ] **Step 4:** `cargo test` → PASS; clippy clean.
- [ ] **Step 5:** Commit `feat(window): cycle halves through two-thirds and one-third (K3)`.

### Task 4: K4 — minimize, minimize others, close others, toggle maximize; failure notices

**Files:**
- Modify: `src/commands.rs` (`Cmd`, `ALL`, `destructive`, `run`), `src/apps.rs` (reuse the `EnumWindows` app-window filter behind `running()`), `src/win.rs` (`Msg::Notice`), `src/main.rs` (dispatch at ~1684 sends `Msg::Notice(err)` on `Err`; handler pushes to `self.notice`)
- Test: `src/commands.rs` tests, `src/apps.rs` tests

**Interfaces:**
- Produces: `Cmd::{Minimize, MinimizeOthers, CloseOthers, ToggleMaximize}`; `apps::app_windows() -> Vec<HWND>` (visible, unowned, non-tool top-level windows, the same filter `running()` uses); `win::Msg::Notice(String)`.

- [ ] **Step 1: Write failing tests:** `close_others_asks_twice` (`Cmd::CloseOthers.destructive()` is true, `Cmd::MinimizeOthers.destructive()` is false); in `apps.rs`, an `#[ignore]`d `app_windows_lists_this_machine` printing the count (pattern of `running_apps_are_listed`).
- [ ] **Step 2:** `cargo test close_others` → FAIL.
- [ ] **Step 3:** Implement: Minimize = `ShowWindow(SW_MINIMIZE)`; MinimizeOthers = every `app_windows()` except `target` minimized; CloseOthers = `PostMessageW(WM_CLOSE)` to every other one; ToggleMaximize = `IsZoomed` ? `SW_RESTORE` : `SW_MAXIMIZE`. Titles "Minimize Window", "Minimize Other Windows", "Close Other Windows", "Toggle Maximize". Errors from `run` (none yet besides an invalid target) flow through `Msg::Notice`.
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: three windows open, Close Other Windows → second Enter → only the target stays.
- [ ] **Step 5:** Commit `feat(window): minimize, minimize others, close others, toggle maximize (K4)`.

### Task 5: J1a — glob patterns in "Skip names" (both indexes)

**Files:**
- Modify: `src/files.rs` (`crawl`, `apply`), `src/ntfs.rs` (`Index::left_out`, `Index::new`)
- Test: `src/files.rs` tests, `src/ntfs.rs` tests

**Interfaces:**
- Produces: `files::glob_match(pattern: &str, name: &str) -> bool` (`*` any run, `?` one char, ASCII case-insensitive; patterns made only of `*`/`?` never match); `files::excluded(exclude: &[String], name: &str) -> bool` (exact case-insensitive name or `glob_match`). Callers pass already-lowercased patterns.

- [ ] **Step 1: Write failing tests:** `globs_match_names`: `*.log` matches `Build.LOG`, not `log.txt`; `build-*` matches `build-1`; `?.tmp` matches `a.tmp` not `ab.tmp`; `node_modules` matches only `node_modules`; `*`, `?`, `**` match nothing. Extend `crawl_skips_excluded_and_search_ranks_names` with exclude `*.js` and assert no `.js` file is indexed. In `ntfs.rs`, extend the existing filter test (the one excluding `AppData`, ~line 899) with `*.tmp` excluded.
- [ ] **Step 2:** `cargo test globs` → FAIL.
- [ ] **Step 3:** Implement `glob_match` (iterative two-pointer with backtrack on the last `*`) and route `crawl`, `apply` and `left_out` through `excluded`. Update the Settings help for `files.exclude`: "File or folder names skipped wherever they appear, e.g. node_modules or *.log."
- [ ] **Step 4:** `cargo test` → PASS; clippy clean.
- [ ] **Step 5:** Commit `feat(files): wildcard patterns in Skip names (J1)`.

### Task 6: J1b — `.gitignore` / `.ignore` in the folder index

**Files:**
- Modify: `Cargo.toml` (`ignore = "0.4"`), `src/config.rs` (`Files.gitignore`), `src/files.rs` (`crawl`, `apply`, `cache_key`), `src/settings.rs` (Files → Folders row)
- Test: `src/files.rs` tests

**Interfaces:**
- Consumes: `files::excluded` (Task 5).
- Produces: `files::crawl(roots: &[String], exclude: &[String], gitignore: bool) -> Vec<Entry>`; `files::apply(..., gitignore: bool)`.

Use `ignore::gitignore::Gitignore` per folder that holds a `.gitignore` or `.ignore`, built inside jwalk's `process_read_dir` (a child is dropped when any ancestor matcher says ignored); keep jwalk. A change batch that touches a `.gitignore`/`.ignore` re-crawls that file's folder.

- [ ] **Step 1: Write failing test** `gitignore_hides_ignored_files`: root with `.gitignore` = `dist/\n*.bak`, files `dist/app.js`, `notes.bak`, `src/main.rs`, and `sub/.ignore` = `secret.txt` with `sub/secret.txt`. `crawl(.., true)` indexes `src/main.rs`, `sub` and the two ignore files but not `dist`, `notes.bak`, `sub/secret.txt`; `crawl(.., false)` indexes all.
- [ ] **Step 2:** `cargo test gitignore_hides` → FAIL.
- [ ] **Step 3:** Implement; `cache_key` adds `gitignore`; Settings → Folders row `files.gitignore` (Bool, "Skip what .gitignore and .ignore files list. Folders only, not whole-drive search.").
- [ ] **Step 4:** `cargo test` → PASS; clippy clean.
- [ ] **Step 5:** Commit `feat(files): respect .gitignore in folder search (J1)`.

### Task 7: J2 — hidden files and removable drives

**Files:**
- Modify: `src/config.rs` (`Files.hidden`, `Files.removable`), `src/files.rs` (`crawl`, `apply`, `Files::start`), `src/ntfs.rs` (`put` / wire byte 18, `upsert`, `Node` flag `HIDDEN = 8`, `Filter`, `Index::new`, `left_out`, `ntfs_drives`), `src/settings.rs`
- Test: `src/files.rs`, `src/ntfs.rs` tests

**Interfaces:**
- Produces: `ntfs::Filter { .., hidden: bool, removable: bool }`. Wire format: byte 18 is now flags, bit 0 dir, bit 1 hidden (`FILE_ATTRIBUTE_HIDDEN` from the USN record); old helpers sending `0`/`1` still decode. `files::crawl(.., hidden: bool)` skips `FILE_ATTRIBUTE_HIDDEN` entries when false.

The helper streams fixed and removable NTFS drives (`DRIVE_FIXED | DRIVE_REMOVABLE`); the client's `Index::new` records which drive letters are removable (`GetDriveTypeW`) and drops them unless `removable`. Known ceiling (add a `ponytail:` comment): the USN reason mask has no attribute changes, so hiding a file later is seen only at the next full pass.

- [ ] **Step 1: Write failing tests:** `ntfs`: an index fed one hidden and one plain upsert drops the hidden one with `Filter{hidden:false,..}` and keeps both with `hidden:true`; a record with flags byte `1` still decodes as a plain folder. `files`: `crawl` on a root with a file marked hidden (`attrib +h` via `SetFileAttributesW`) skips it when `hidden = false`.
- [ ] **Step 2:** `cargo test hidden` → FAIL.
- [ ] **Step 3:** Implement; Settings → Files rows `files.hidden` (Bool, "Include hidden files") and `files.removable` (Bool, "Include removable drives. NTFS drives only; network drives aren't searched.") under "Whole-drive search".
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: restart procast (the helper picks up the new build at its next start), toggle hidden off, a known hidden file disappears from File Search.
- [ ] **Step 5:** Commit `feat(files): hidden files and removable drives toggles (J2)`.

### Task 8: J4 — index status and rebuild

**Files:**
- Modify: `src/files.rs` (`built: Instant` → `SystemTime`; `Files::status`), `src/ntfs.rs` (record when the last drive became ready), `src/win.rs` (`Msg::RebuildIndex`), `src/settings.rs` (new `Kind::Status`, `Kind::Action(&'static str)`; `Settings` gets `tx: UnboundedSender<Msg>` and `index_status: Arc<RwLock<String>>`), `src/main.rs` (`open_settings` passes both; `Msg::FilesIndexed` refreshes the string; `Msg::RebuildIndex` drops and restarts `Files`)
- Test: `src/files.rs` tests

**Interfaces:**
- Produces: `files::IndexStatus { entries: usize, whole_drives: bool, drives: Vec<char>, updated: Option<SystemTime>, indexing: bool }`; `Files::status(&self) -> IndexStatus`; `files::status_line(s: &IndexStatus, now: SystemTime) -> String`.

Rebuild = the same path a settings change takes today (`same_settings` false → new `Files::start`), which re-crawls folders or reconnects the helper and re-enumerates the MFT.

- [ ] **Step 1: Write failing test** `status_line_reads_plainly`: `{entries: 2712340, whole_drives: true, drives: [C,D,E], updated: 10:42 local, indexing: false}` → `"2,712,340 entries · whole drives C:, D:, E: · updated 10:42"`; folders case → `"138,617 entries · folders · crawled 10:42"`; `indexing: true` → `"Indexing…"`; `updated: None` and not indexing → `"Not indexed yet"`. Use `win::local_time_of` for the clock (time part only when today, else full date).
- [ ] **Step 2:** `cargo test status_line` → FAIL.
- [ ] **Step 3:** Implement; Settings → Files first group "Index": `Kind::Status` row showing the string, and `Kind::Action("rebuild-index")` drawn with the existing quiet `design::button` ("Rebuild"), sending `Msg::RebuildIndex`.
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: Settings → Files shows the line; Rebuild flips it to "Indexing…" then back.
- [ ] **Step 5:** Commit `feat(files): index status and rebuild in Settings (J4)`.

### Task 9: J3 — File Search detail pane

**Files:**
- Modify: `src/main.rs` (render: `side` also for `Mode::Files`; new `file_detail`; preview cache next to the clipboard image cache ~line 928), `src/files.rs` (`file_info`)
- Test: `src/files.rs` tests

**Interfaces:**
- Produces: `files::FileInfo { kind: &'static str, size: Option<u64>, modified: Option<SystemTime>, created: Option<SystemTime>, folder: String, preview: Preview }`, `files::Preview { Image(PathBuf), Text(String), Folder(usize), Icon }`, `files::file_info(path: &str, home: &str) -> Option<FileInfo>` (None = not found).

Preview rule: image extensions the `image` crate decodes (png, jpg, jpeg, gif, webp, bmp, ico) → `Image`; text-like (the `Code` extensions in `item()` plus txt, md, log, csv, ini, xml) under 256 KB → first 40 lines; folder → entry count (`read_dir().count()`, capped at 10,000); else `Icon`. Images decode on a worker thread into the cache; until ready, show the icon.

- [ ] **Step 1: Write failing test** `file_info_describes_files`: temp dir with `a.txt` (50 lines), `big.txt` (300 KB), `pic.png` (any 2×2 PNG via `image`), `sub/` with 3 files. `a.txt` → `Text` of exactly 40 lines, `kind == "File"`, `size == Some(len)`; `big.txt` → `Icon`; `pic.png` → `Image`; `sub` → `Folder(3)`; a missing path → `None`; `folder` uses `~` under `home`.
- [ ] **Step 2:** `cargo test file_info` → FAIL.
- [ ] **Step 3:** Implement `file_info`; `file_detail` renders preview then Information rows Kind, Size, Modified, Created, Where with the existing row helper; "Not found" text when `None`. Info is computed when the selection changes, not on every render.
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual (and a screenshot per the procast-screenshots memory): File Search, arrow through an image, a text file, a folder.
- [ ] **Step 5:** Commit `feat(files): detail pane in File Search (J3)`.

### Task 10: K6 — layout presets

**Files:**
- Modify: `src/config.rs` (`Layout`, `Slot`, `Window.layouts`, template example), `src/search.rs` (`items`: one item per layout, key `wl:<name>`), `src/search.rs`/`src/main.rs` (`Act::Layout(String)` dispatch on a thread like `Act::Cmd`), `src/commands.rs` (`run_layout`), `src/settings.rs` (Shortcuts category for `wl`)
- Test: `src/config.rs`, `src/commands.rs` tests

**Interfaces:**
- Consumes: `cycle_target`/`with_gaps` (Tasks 2–3), `apps::app_windows` (Task 4).
- Produces: `config::Layout { name: String, slots: Vec<Slot> }`, `config::Slot { app: String, snap: String, display: Option<usize> }`; `commands::snap_by_name(name: &str) -> Option<Snap>`; `commands::run_layout(l: &config::Layout, w: &config::Window) -> Result<(), String>`.

Per slot: find an `app_windows()` entry whose process exe name equals `app` (case-insensitive); none → `win::open(app)` (or `launch_app` when it resolves to a Start-menu app) and poll every 100 ms up to 5 s; then snap it on monitor `display` (1-based, `EnumDisplayMonitors` order) or its current one. Unknown `snap` name or app that never shows a window → `Err("Layout Coding: Code.exe didn't open a window")`-style message.

- [ ] **Step 1: Write failing tests:** `layouts_parse` (the spec's TOML example parses into 2 slots, `display` None); `snap_names_round_trip` (`snap_by_name("FirstTwoThirds") == Some(Snap::FirstTwoThirds)`, `"Nope"` → None, case-sensitive like the variant names).
- [ ] **Step 2:** `cargo test layouts_parse` and `cargo test snap_names` → both FAIL.
- [ ] **Step 3:** Implement; root item title `Layout: <name>`, subtitle "Window".
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: the spec's example with an editor and a terminal.
- [ ] **Step 5:** Commit `feat(window): layout presets from config (K6)`.

### Task 11: K5a — virtual desktop commands over winvd

**Files:**
- Modify: `Cargo.toml` (`winvd = { git = "https://github.com/Ciantic/VirtualDesktopAccessor", rev = "ef74c24", features = ["multiple-windows-versions"] }`), `src/commands.rs` (`Cmd::Desktop(Desk)`, `ALL`, `run`), `src/search.rs` (`items` skips `Cmd::Desktop` when `!desktops::supported()`), `src/main.rs` (`Pending::RenameDesktop`, asked like a one-argument script)
- Create: `src/desktops.rs`
- Test: `src/desktops.rs` tests

**Interfaces:**
- Produces:
  - `commands::Desk { Open(u8), MoveTo(u8), MoveNext, MovePrev, Close, Rename }` (1-based indexes 1–10; `Rename` carries no text — main collects it and calls `desktops::rename`).
  - `desktops::supported() -> bool` (build ≥ 22631 via `RtlGetVersion`, cached).
  - `desktops::call<T>(what: &str, f: impl FnOnce() -> Result<T, winvd::Error>) -> Result<T, String>`: runs `f` on the one desktop thread (a `std::thread` with COM initialised, fed by an `mpsc` channel), catches panics (`catch_unwind`), maps errors to `"<what>: <error>"`.
  - `desktops::open(n: u8)`, `move_window(hwnd: HWND, n: u8)`, `move_next(hwnd)`, `move_prev(hwnd)`, `close()`, `rename(name: &str)` — each `-> Result<(), String>`.

Behaviour (spec): N past the last desktop → `Err("There is no desktop N")`; Move Next on the last desktop creates one and moves there; Move Prev on the first does nothing; moving never switches; Close falls back to the desktop on the left (index 0 → the next one). Titles: "Open Desktop 1"…"Open Desktop 10", "Move to Desktop 1"…"10", "Move to Next Desktop", "Move to Previous Desktop", "Close Desktop", "Rename Desktop"; subtitle "Window"; words "virtual desktop".

- [ ] **Step 1: Write failing tests:** `call_turns_panics_into_errors` (`call("Switch", || -> Result<(), _> { panic!("boom") })` is `Err` containing "Switch"); `out_of_range_is_a_message` (the pure index check `desktops::check_index(n: u8, count: u32) -> Result<u32, String>`: `(11, 4)` and `(5, 4)` → `Err("There is no desktop 5")`-style, `(2, 4)` → `Ok(1)`); `#[ignore]` `desktops_on_this_machine` printing count and current index.
- [ ] **Step 2:** `cargo test desktops::` → FAIL.
- [ ] **Step 3:** Implement; `run` returns the `Err` for `Msg::Notice` (Task 4 path).
- [ ] **Step 4:** `cargo test` → PASS; `cargo clippy --all-targets -- -D warnings` clean (two `windows` versions in the tree is expected). Manual on this PC (build 26200): Open Desktop 2, Move to Next Desktop, Rename Desktop, Close Desktop.
- [ ] **Step 5:** Commit `feat(window): virtual desktop commands via winvd (K5)`.

### Task 12: K5b — Auto Close Empty Desktops

**Files:**
- Modify: `src/desktops.rs` (listener), `src/main.rs` (start/stop listener on config load), `src/settings.rs` (Window row)
- Test: `src/desktops.rs` tests

**Interfaces:**
- Consumes: `desktops::call`, `apps::app_windows` (Task 4).
- Produces: `desktops::Listener::start() -> Option<Listener>` (None when unsupported; dropping it stops); `desktops::should_remove(left: u32, current: u32, count: u32, windows_on_left: usize) -> bool`.

On a desktop-changed event from winvd's listener: wait 500 ms, then if the old desktop still exists, isn't the current one, and `app_windows()` has none on it (`winvd::get_desktop_by_window`), remove it with the current desktop as fallback. Errors are logged only (auto-close is background; no notice spam).

- [ ] **Step 1: Write failing test** `only_empty_left_desktops_go`: `should_remove(1, 2, 3, 0)` true; `(1, 2, 3, 2)` false; `(2, 2, 3, 0)` false (it's current); `(0, 0, 1, 0)` false (last desktop never removed).
- [ ] **Step 2:** `cargo test only_empty` → FAIL.
- [ ] **Step 3:** Implement; Settings → Window row `window.auto_close_empty_desktops` (Bool, "Remove a desktop with no windows when you switch away from it.").
- [ ] **Step 4:** `cargo test` → PASS; clippy clean. Manual: setting on, create an empty desktop, switch to it and back, it disappears within a second.
- [ ] **Step 5:** Commit `feat(window): auto close empty desktops (K5)`.

### Task 13: Roadmap ticks and final check

**Files:**
- Modify: `docs/roadmap.md` (J1–J4, K1–K6 to `[x]` / ✅ with one-line notes: J1 "globs everywhere; .gitignore in folder search only", J2 "removable NTFS drives; no network", K4 "toggle maximize, not full screen", K5 "Windows 11 23H2+, via winvd")

- [ ] **Step 1:** `cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test` → zero warnings, clean, all pass.
- [ ] **Step 2:** Update the roadmap rows.
- [ ] **Step 3:** Commit `docs: roadmap J and K done`.
