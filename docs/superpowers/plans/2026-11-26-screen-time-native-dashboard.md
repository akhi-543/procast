# Screen Time Native Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Screen Time's list-and-text launcher view with a native, scrolling dashboard (Day / Week / Month) drawn in GPUI, and remove the web dashboard on 127.0.0.1:8737.

**Architecture:** Pure period and chart math lives in `src/wellbeing.rs` next to `stats` / `days`, unit-tested. A new `src/screen_time_view.rs` holds `impl Launcher` rendering (the pattern `src/music_view.rs` uses) and replaces the Screen Time list body and right-hand pane in `src/main.rs`. `Mode::ScreenTime` carries a `wellbeing::Period` (range + anchor day) instead of a `Range`. The HTTP module and its page are deleted last.

**Tech Stack:** Rust 2024, GPUI, rusqlite 0.37, serde_json. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-26-screen-time-native-dashboard-design.md` (builds on `docs/superpowers/specs/2026-11-25-screen-time-design.md`).

## Global Constraints

- Work only in `D:\Pro\akscast-screen-time` (branch `feat/screen-time`). Never touch `D:\Pro\akscast`, never stop or start the user's procast, never run `schtasks`.
- Never bind, connect to or test against port 8737.
- No new dependencies. Rust 2024, GPUI, rusqlite as already in `Cargo.toml`.
- procast design system: `docs/superpowers/plans/procast-design-system.md` and `procast-design-system-rulings.md`; theme `Tokens`, `design::ts`, `design::r`, `design::sp`, Space Grotesk. Cards on the theme surface with `r::LG` corners, 16 px page padding, 12 px gaps.
- Launcher width is fixed (`base_width(cfg)`, 750 by default); one column.
- Category colours (light / dark), copied from `assets/screen-time/dashboard.html` before it is deleted. Slots in order Development, Productivity, Messaging, Social, Streaming, Gaming, Browsing:
  light `#2a78d6 #eb6834 #1baf7a #eda100 #e87ba4 #008300 #4a3aa7`, spare `#e34948`, Other `#aeaeb2`;
  dark `#3987e5 #d95926 #199e70 #c98500 #d55181 #008300 #9085e9`, spare `#e66767`, Other `#636366`.
  Rule: a slot category gets its slot colour; the first non-slot, non-Other category (largest time) gets the spare; every other category is Other's grey.
- Heatmap shades h0..h4: light `#e8e8ed #9ec5f4 #5598e7 #256abf #0d366b`, dark `#2c2c2e #104281 #1c5cab #3987e5 #9ec5f4`. Productive `#2a78d6` / `#3987e5`, distracting `#e34948` / `#e66767`.
- Heatmap level of a day: `0` under 60 s, else `min(4, 1 + floor(seconds / max * 4))` where `max` is the largest day total in the shown range.
- Week share: a week with under 60 s total has no point (gap).
- Rows stay at the top 12 apps and 12 sites (`stats` already truncates).
- Launcher height in this view: `max(window_height(cfg), 640.)` before interface scaling.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` (or the implementing model's own name).
- Verification per task: `cargo test` and `cargo clippy --all-targets -- -D warnings` clean.

## Review Focus

1. Month stepping from the 31st (Jan 31 → Feb) and across a year (Dec → Jan, Jan → Dec): no skipped or doubled month. Test in Task 1.
2. A week or month that crosses a month or year boundary names itself correctly ("29 Dec – 4 Jan"). Test in Task 1.
3. A category with time only in the previous period still appears in the comparison card, with "0m" now. Test in Task 2.
4. A typed filter keeps ← / → for the text caret; the period only steps with an empty field. Binding test in Task 4.
5. Switching period quickly: an older read's answer is dropped (the stale check compares the whole `Period`). Test in Task 1 via a pure `fresh(...)` check.

---

### Task 1: Periods — Day / Week / Month with an anchor day

**Files:**
- Modify: `src/wellbeing.rs` (`Range`, `span`, `header`, tests)
- Modify: `src/main.rs` (`Mode::ScreenTime`, every `Mode::ScreenTime(range)` use, `load_screen_time`, `screen_time_loaded`, command handlers at ~4135)

**Interfaces:**
- Produces (in `wellbeing`):
  - `pub enum Range { Day, Week, Month }` (rename `Today` → `Day`), `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`.
  - `pub struct Period { pub range: Range, pub anchor: i64 }`, same derives.
  - `pub fn span(p: Period, today: i64) -> ((i64, i64), (i64, i64))` → `(shown, before)`. `shown`: Day `(a, a)`; Week Monday..Sunday of `a`; Month first..last of `a`'s month. `before`: the to-date part of `shown` (`(shown.0, min(shown.1, today))`) moved back one period: Day −1 day; Week −7 days; Month: previous month's first day plus the same length, clamped to that month's last day.
  - `pub fn step(p: Period, dir: i64, today: i64) -> Period` — ±1 period; Month lands on the 1st of the adjacent month; a step whose `shown.0 > today` returns `p` unchanged.
  - `pub fn with_range(p: Period, r: Range) -> Period` — same anchor, new range.
  - `pub fn title(p: Period, today: i64) -> String` — "Today", "Yesterday", "Wed 19 Nov"; "This week", "17 – 23 Nov" / "29 Dec – 4 Jan"; "This month", "October 2026".
  - `pub fn caption(p: Period, today: i64) -> String` — current period: "so far today" / "this week" / "this month"; else "on Wed 19 Nov" / "17 – 23 Nov" / "in October 2026".
  - `pub fn fresh(current: Period, asked: Period, current_gen: u64, asked_gen: u64) -> bool` — true only when both match (used by `screen_time_loaded`).
  - `header(total, previous, p: Period, today)` keeps its text for current periods and uses "the day before" / "the week before" / "the month before" for past ones.
- Produces (in `main.rs`): `Mode::ScreenTime(wellbeing::Period)`. **Screen Time** → `Period { range: Day, anchor: today }`, **Screen Time This Week** → `Period { range: Week, anchor: today }`.
- Date math: `calendar::days_from_civil`, `civil_from_days`, `weekday` (all `pub(crate)`).

- [ ] **Step 1: Write the failing tests** in `wellbeing.rs`'s test module (day numbers via `parse_iso`):

```rust
#[test]
fn spans_per_period() {
    let d = |s| parse_iso(s).unwrap();
    let today = d("2026-11-26"); // Thursday
    let p = |range, a| Period { range, anchor: d(a) };
    assert_eq!(span(p(Range::Day, "2026-11-26"), today), ((today, today), (today - 1, today - 1)));
    assert_eq!(span(p(Range::Week, "2026-11-26"), today), ((d("2026-11-23"), d("2026-11-29")), (d("2026-11-16"), d("2026-11-19"))));
    assert_eq!(span(p(Range::Week, "2026-11-12"), today), ((d("2026-11-09"), d("2026-11-15")), (d("2026-11-02"), d("2026-11-08"))));
    assert_eq!(span(p(Range::Month, "2026-11-26"), today), ((d("2026-11-01"), d("2026-11-30")), (d("2026-10-01"), d("2026-10-26"))));
    assert_eq!(span(p(Range::Month, "2026-03-31"), today), ((d("2026-03-01"), d("2026-03-31")), (d("2026-02-01"), d("2026-02-28"))));
}

#[test]
fn steps_and_titles() {
    let d = |s| parse_iso(s).unwrap();
    let today = d("2026-11-26");
    let p = |range, a| Period { range, anchor: d(a) };
    assert_eq!(step(p(Range::Month, "2026-01-31"), 1, today), p(Range::Month, "2026-02-01"));
    assert_eq!(step(p(Range::Month, "2026-01-15"), -1, today), p(Range::Month, "2025-12-01"));
    assert_eq!(step(p(Range::Day, "2026-11-26"), 1, today), p(Range::Day, "2026-11-26"), "no future");
    assert_eq!(step(p(Range::Week, "2026-11-26"), 1, today), p(Range::Week, "2026-11-26"));
    assert_eq!(title(p(Range::Day, "2026-11-26"), today), "Today");
    assert_eq!(title(p(Range::Day, "2026-11-25"), today), "Yesterday");
    assert_eq!(title(p(Range::Day, "2026-11-19"), today), "Thu 19 Nov");
    assert_eq!(title(p(Range::Week, "2026-11-26"), today), "This week");
    assert_eq!(title(p(Range::Week, "2026-01-01"), today), "29 Dec – 4 Jan");
    assert_eq!(title(p(Range::Month, "2026-10-09"), today), "October 2026");
    assert_eq!(caption(p(Range::Day, "2026-11-26"), today), "so far today");
    assert_eq!(caption(p(Range::Month, "2026-10-09"), today), "in October 2026");
    let a = p(Range::Day, "2026-11-26");
    assert!(fresh(a, a, 3, 3));
    assert!(!fresh(p(Range::Week, "2026-11-26"), a, 3, 3), "an answer for another period is dropped");
    assert!(!fresh(a, a, 4, 3));
}
```

Update the existing `span` / `header` tests to the new signatures; keep their expectations for the current day and week.

- [ ] **Step 2: Run** `cargo test --bin procast wellbeing` — expect compile errors / FAIL on the new names.

- [ ] **Step 3: Implement** the interfaces above in `wellbeing.rs`, then move `main.rs` to `Mode::ScreenTime(Period)`: `load_screen_time` takes `Period` and reads `stats(shown, before)`; `hours_by_name` only when `shown.0 == shown.1`; `screen_time_loaded` uses `fresh`. The existing list view keeps working (it now shows the period's header). Window title (~5352) and icon (~5448) match on `p.range` (Month uses `g::CALENDAR_CLOCK` too).

- [ ] **Step 4: Run** `cargo test` and `cargo clippy --all-targets -- -D warnings` — PASS, clean.

- [ ] **Step 5: Commit** `feat(screen-time): Day, Week and Month periods with an anchor day`.

---

### Task 2: Dashboard data — series, comparison, colours

**Files:**
- Modify: `src/wellbeing.rs` (pure helpers, tests)
- Modify: `src/main.rs` (`load_screen_time` also reads `days`; new state field)

**Interfaces:**
- Consumes: Task 1's `Period`, `span`.
- Produces (in `wellbeing`):
  - `pub fn week_shares(days: &Value, productive: &[String], first_monday: i64, weeks: usize) -> Vec<Option<f64>>` — productive seconds / total seconds per week, `None` under 60 s.
  - `pub fn heat_levels(days: &Value, from: i64, to: i64) -> Vec<u8>` — one level per day `from..=to` (rule in Global Constraints; days after `to` are not included).
  - `pub fn compare_rows(stats: &Value) -> Vec<(String, u64, u64)>` — `(category, now, before)`: the seven slot categories always, in slot order, then any other category with time in either period, largest `now` first.
  - `pub const SLOTS: [&str; 7] = ["Development", "Productivity", "Messaging", "Social", "Streaming", "Gaming", "Browsing"];`
  - `pub fn color_slot(category: &str, stats: &Value) -> Option<usize>` — `Some(0..7)` slot, `Some(7)` the spare, `None` grey (rule in Global Constraints; "first non-slot" = largest in `stats["categories"]`).
  - `pub fn busiest(stats: &Value) -> Option<(String, u64)>` — the bucket with the most seconds, labelled "11 AM" (hourly) or "Wed 19" (daily); `None` when all are zero.
- Produces (in `main.rs`): field `st_days: Option<Value>` — `wellbeing::days(con, today - 370, today, rules)` read in the same background task as `stats`. 371 days covers both the 12-week line (`first_monday` = Monday of this week − 77) and the 53-week heatmap.

- [ ] **Step 1: Write the failing tests** (build `stats` / `days` JSON by hand in the shape `stats()` and `days()` return):

```rust
#[test]
fn comparison_keeps_categories_that_went_quiet() {
    let stats = json!({"categories": [{"name": "Development", "seconds": 120}, {"name": "Reading", "seconds": 60}],
                       "previous_categories": {"Development": 2340, "Messaging": 120}});
    let rows = compare_rows(&stats);
    assert_eq!(&rows[..3], &[("Development".into(), 120, 2340), ("Productivity".into(), 0, 0), ("Messaging".into(), 0, 120)]);
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[7], ("Reading".into(), 60, 0));
}

#[test]
fn shares_levels_colours_and_busiest() {
    let d = |s| parse_iso(s).unwrap();
    let days = json!({"days": {"2026-11-16": {"Development": 3000, "Other": 1000}, "2026-11-24": {"Other": 30}}});
    let shares = week_shares(&days, &["Development".into()], d("2026-11-16"), 2);
    assert_eq!(shares, vec![Some(0.75), None]);
    assert_eq!(heat_levels(&days, d("2026-11-16"), d("2026-11-17")), vec![4, 0]);
    let stats = json!({"categories": [{"name": "Development", "seconds": 9}, {"name": "Reading", "seconds": 8}, {"name": "Cooking", "seconds": 7}, {"name": "Other", "seconds": 6}],
                       "buckets": [{"label": 10, "categories": {"Other": 5}}, {"label": 11, "categories": {"Development": 9}}]});
    assert_eq!(color_slot("Development", &stats), Some(0));
    assert_eq!(color_slot("Reading", &stats), Some(7));
    assert_eq!(color_slot("Cooking", &stats), None);
    assert_eq!(color_slot("Other", &stats), None);
    assert_eq!(busiest(&stats), Some(("11 AM".into(), 9)));
}
```

- [ ] **Step 2: Run** `cargo test --bin procast wellbeing` — FAIL (missing functions).
- [ ] **Step 3: Implement** the helpers and the `st_days` read (reset to `None` when a new period starts loading, like `st_stats`).
- [ ] **Step 4: Run** `cargo test` and clippy — PASS, clean.
- [ ] **Step 5: Commit** `feat(screen-time): dashboard series, comparison rows and colours`.

---

### Task 3: The dashboard body

**Files:**
- Create: `src/screen_time_view.rs` (`impl Launcher` rendering, colour tables)
- Modify: `src/main.rs` (`mod screen_time_view;`, body selection ~6334 and ~6528, `target_height`, `screen_time_rows` / `screen_time_actions`, remove `screen_time_detail`)

**Interfaces:**
- Consumes: Task 1 `Period`, `title`, `caption`; Task 2 helpers, `st_days`; existing `st_stats`, `st_hours`, `wellbeing::rows`, `wellbeing::detail`.
- Produces: `impl Launcher { pub(crate) fn screen_time_dashboard(&self, t: &Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement }`, and `pub(crate) fn category_color(slot: Option<usize>, dark: bool) -> Hsla`, `pub(crate) fn heat_color(level: u8, dark: bool) -> Hsla` in `screen_time_view.rs`.
- `self.items` stays the selection model: item 0 is a non-selectable anchor (title = the header text, `extra` = `[Export CSV]`, Enter does nothing); then the Apps rows and Sites rows (`section` "Apps" / "Sites"), filtered by the query as today; each row's `extra` is `[Move to Category…, Export CSV]`. Category rows are no longer items. Off / Loading / error / "No matches" states keep `empty_row` and the existing empty-state body.
- New state: `st_scroll: gpui::ScrollHandle` for the dashboard column; selection changes call `st_scroll.scroll_to_item` for the selected row's child index.

- [ ] **Step 1: Write the failing test** for the pure part of the colours:

```rust
#[test]
fn colours_match_the_web_palette() {
    assert_eq!(category_color(Some(0), false), gpui::rgb(0x2a78d6).into());
    assert_eq!(category_color(Some(7), true), gpui::rgb(0xe66767).into());
    assert_eq!(category_color(None, true), gpui::rgb(0x636366).into());
    assert_eq!(heat_color(0, false), gpui::rgb(0xe8e8ed).into());
    assert_eq!(heat_color(4, true), gpui::rgb(0x9ec5f4).into());
}
```

- [ ] **Step 2: Run** `cargo test --bin procast screen_time_view` — FAIL.
- [ ] **Step 3: Implement the body**, sections in spec order (spec "Layout"): header (segmented Day / Week / Month, ‹ › buttons, `title`; clicks call the same handlers Task 4 binds), total card (display-size total, `caption`, stacked bars from `buckets` with a 3-line grid and labels every 6 h for Day / every day for Week / every 7th day for Month), Categories, four stat tiles (`focus.first`/`last`, `busiest`, `focus.longest`, `focus.switches`), Productive and distracting (share bar from `config.screen_time.productive` / `distracting`, legend, 12-week line drawn with `gpui::canvas` + `PathBuilder` — `src/notes/window.rs` has the one existing `canvas` use), comparison card (`compare_rows`, solid vs faded bars, signed difference or "No change"), Past year (53 × 7 `heat_levels` grid, month labels; the shown period's cells get a ring), then Apps and Websites rows. The selected row shows one inline line under it: `wellbeing::detail` reduced to time, share and (Day only) the non-zero hours. Empty span: zero cards plus one muted line "No screen time recorded". Dark/light from the current theme. `target_height` returns `max(window_height(cfg), 640.)` while `Mode::ScreenTime` with data. Remove `screen_time_detail` and the `side` arm for Screen Time.
- [ ] **Step 4: Run** `cargo test`, clippy; build `cargo build --release` and check by hand with `PROCAST_DEV=1` (dev instance; it never registers the browser host): Day / Week / Month render, light and dark, an empty past day, a long site name truncates.
- [ ] **Step 5: Commit** `feat(screen-time): native dashboard in the launcher`.

---

### Task 4: Keys and clicks

**Files:**
- Modify: `src/main.rs` (actions, key bindings ~6968–7013, key context next to `grid_keys` ~5872, binding tests ~7737)

**Interfaces:**
- Consumes: Task 1 `step`, `with_range`; Task 3 body.
- Produces: actions `NextPeriodKind`, `PrevPeriodKind`, `PrevPeriod`, `NextPeriod` in the `procast` namespace; `fn screen_time_keys(&self, cx) -> Option<&'static str>` returning `"ScreenTime ScreenTimeArrows"` when in `Mode::ScreenTime` with no panel, menu or confirm and an empty query, `"ScreenTime"` with a non-empty query, else `None`.
- Bindings: `tab` → `NextPeriodKind`, `shift-tab` → `PrevPeriodKind` in `ScreenTime > {DEFAULT_INPUT_CONTEXT}`; `left` → `PrevPeriod`, `right` → `NextPeriod` in `ScreenTimeArrows > {DEFAULT_INPUT_CONTEXT}`. Handlers set the mode to the new `Period` and reload (keeping the query and resetting selection to the first row).

- [ ] **Step 1: Write the failing binding test** next to the existing ones (same `first(key, contexts)` helper):

```rust
assert_eq!(first("tab", &["ScreenTime", input]), "NextPeriodKind");
assert_eq!(first("shift-tab", &["ScreenTime", input]), "PrevPeriodKind");
assert_eq!(first("left", &["ScreenTime", "ScreenTimeArrows", input]), "PrevPeriod");
assert_eq!(first("right", &["ScreenTime", "ScreenTimeArrows", input]), "NextPeriod");
assert_ne!(first("left", &["ScreenTime", input]), "PrevPeriod", "a typed filter keeps its caret keys");
assert_eq!(first("tab", &[input]), "SelectNext", "other views keep Tab");
```

- [ ] **Step 2: Run** `cargo test --bin procast` — FAIL.
- [ ] **Step 3: Implement** actions, bindings, context, handlers; wire the header's segment and ‹ › clicks to the same handlers; Up/Down already move `selected` through the rows.
- [ ] **Step 4: Run** `cargo test`, clippy — PASS, clean. Check by hand in the dev build: Tab cycles, ← → step and stop at the current period, typing then ← moves the caret.
- [ ] **Step 5: Commit** `feat(screen-time): Tab and arrow keys for the dashboard`.

---

### Task 5: Remove the web dashboard

**Files:**
- Delete: `src/wellbeing_http.rs`, `assets/screen-time/dashboard.html`
- Modify: `src/main.rs` (`mod wellbeing_http`, server start in `start_screen_time` ~454, `OpenScreenTimeDashboard` arms ~4137–4142, `screen_time_actions`), `src/commands.rs` (`Cmd::OpenScreenTimeDashboard` and its entries at ~122, 224, 370, 430, 806), `src/config.rs` (sample comment ~974–975: drop the URL; "productive on the dashboard" stays true), `README.md` (~119, ~191), `docs/superpowers/specs/2026-11-25-screen-time-design.md` (one line under its dashboard section: "Superseded by 2026-11-26-screen-time-native-dashboard-design.md: the dashboard is native and the web server is gone."), `D:\Pro\micro bot\README.md` (only the "Moved into procast" note at the top, uncommitted, outside this repo)

**Interfaces:**
- Consumes: Task 3 (the native view replaces every place the dashboard was opened).
- Produces: no `8737`, `wellbeing_http`, `dashboard.html` or `OpenScreenTimeDashboard` anywhere in `src/`, `assets/`, `README.md`.

- [ ] **Step 1: Write the failing test** in `commands.rs`'s tests: `assert!(!ALL.iter().any(|(_, title, _, _)| title.contains("Dashboard")));`
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Delete and edit** as listed. README line 119 becomes: "...shows today, this week and this month by category, app and site in a dashboard inside the launcher (**Screen Time**, **Screen Time This Week**)..." with the 8737 clause removed. micro bot note: replace "(Screen Time Today / This Week, the same dashboard on `127.0.0.1:8737`)" with "(a Screen Time dashboard inside the launcher)"; leave the rest of that file.
- [ ] **Step 4: Run** `cargo test`, clippy; `rg -n "8737|wellbeing_http|dashboard.html|OpenScreenTimeDashboard" src assets README.md` prints nothing.
- [ ] **Step 5: Commit** (procast repo only) `refactor(screen-time): remove the web dashboard`.
