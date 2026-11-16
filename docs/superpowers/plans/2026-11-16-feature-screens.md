# Feature Screens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the ten o-tools features (Timer and Stopwatch, Translator, Define, Screenshots with Paste Latest, Foreground watcher, Toggle HDR and Auto HDR, Focus, My Schedule, Notes storage and editor) look and behave as the design system's **Feature screens** section says. That covers the screens, their keys and their states.

**Architecture:** Two shared parts come first:
- **InlineArguments**: argument boxes in the launcher's search bar.
- **One Dropdown menu**.

Then each feature, in the spec's build order. All of it lives inside the existing single `Launcher` entity (`src/main.rs`) and the Settings window (`src/settings.rs`). New code goes in new modules only where a task says so:
- `src/menu.rs`: the Dropdown.
- `src/hud.rs`: the Focus pill and notice windows.
- `src/notes/window.rs`: the Notes window.

The feature modules that exist today (`timers`, `translate`, `dict`, `shots`, `hdr`, `focus`, `calendar`, `foreground`, `notes/*`) keep the logic. Each gains the small pure functions its screen needs, and those functions get unit tests.

**Tech Stack:** Rust 2024, gpui-ce 0.3 (`gpui_ce_elements` `EditableTextState` / `text_input`), `windows` 0.62, the browser companion in `browser/`. No new crates.

**Spec:**
- Snapshot: `.superpowers/sdd/2026-11-16-feature-screens/spec/`.
  - `feature-screens.md` is the contract.
  - `components/<Card>/README.md` and `components/bundle.css` (section "feature screens") give sizes and the details the contract leaves out.
  - `icon-map.md` gives icons and tints.
  - `tokens.json` gives colours, which already match `src/design.rs`.
  - `README.md` gives the wording rules.
- Artifact: https://claude.ai/artifact/41jrDsMRQpkkm5fnMMRWPc (group **Feature screens**).

## Global Constraints

- Zero warnings, zero clippy (`cargo clippy --all-targets -- -D warnings`), `cargo test` green before every commit. Build only inside the worktree; never kill or restart the user's running procast.exe.
- The design system snapshot is the spec. Every colour, size, radius, text style and icon comes from src/design.rs tokens (add tokens there if the spec has new ones; never literals in views). No purple. Neutral grey selection. Sticker icon tiles. No tweens or new animations beyond what the spec names.
- New icons: Lucide 0.460.0 only, listed in assets/icons/README.md.
- Every network or OS call fails soft: toast, never a panic or a UI-thread block.
- New config keys get a commented line in the config.toml template in src/config.rs. The user has no old config to migrate; config shapes may change freely.
- Copy strings exactly as the spec writes them.
- One commit per task, message `feat(<area>): … (feature screens)`, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Manual checks** run a second, isolated procast, using the seam from Task 1:
  - Command, in Git Bash from the worktree: `APPDATA="$SCRATCH/appdata" PROCAST_DEV=1 PROCAST_SHOW=root PROCAST_QUERY="…" ./target/debug/procast.exe & echo $!`.
  - Stop it only by that PID (`taskkill //PID <pid>`), never by image name.
  - `PROCAST_E2E=1` makes each render write what's on screen. Use it to read copy and states without a screenshot.

## Spec precedence

When spec files disagree, `feature-screens.md` wins. A component README or preview fills in only what the contract leaves open. The conflicts resolved this way:

| Topic | Contract (used) | Other file (not used) |
|---|---|---|
| Timer subtitle | "25 min · tea" | TimerRows README "25:00 · tea" |
| Timer Ctrl+K | Restart, Rename, Add 5 min, Stop | README Stop, Restart, Add 1 Minute, Copy Time |
| Timer done toast | "tea · 25 min done" | README "Pasta is done · 10:00" |
| Toast timing | success closes after 2 s; errors stay until Esc | Toast README 2.5 s / 4 s |
| Define Ctrl+K | Copy Word, Define Similar Word, Open in Browser | DefineView README Copy Word, Search the Web, Translate |
| Unknown word | `No definition for 'xyz'` | DefineView README double quotes |
| Schedule empty hint | "Paste your calendar's ICS address to see today's meetings here" | ScheduleList README shorter hint |
| Add Calendar | a launcher form | README "opening Settings → Calendar" |
| Detail split | list 40% | bundle.css `.pc-split` 1fr 1fr |
| Notes window | 560×440 | bundle.css `.pc-notes` 560×380 |

The Focus form takes its row width from bundle.css: 500 = label 120 + gap 12 + control. FocusForm README says controls 360; the two agree closely enough.

## Order

| # | Task | Size |
|---|---|---|
| 1 | Icons, tokens and the dev-instance seam | S |
| 2 | Dropdown menu | M |
| 3 | InlineArguments | L |
| 4 | Toast kinds and timing | S |
| 5 | Timer and Stopwatch rows | M |
| 6 | Translator view | M |
| 7 | Launcher form scaffold | M |
| 8 | Settings row recipe and feature sections | M |
| 9 | Translator language sets and Settings → Translator | M |
| 10 | Define view | M |
| 11 | HDR rows, Auto HDR toast, Settings → Games and the foreground row | M |
| 12 | Focus form and running row | L |
| 13 | Focus pill, notice, blocked page and Settings → Focus | M |
| 14 | My Schedule list | M |
| 15 | Add Calendar and Settings → Calendar | M |
| 16 | Screenshot grid | L |
| 17 | Screenshot actions, Quick Look, Paste Latest and Settings → Screenshots | M |
| 18 | Notes window, commands and store wiring | L |
| 19 | Notes editor rendering | L |
| 20 | Notes action list, browse list, formatting bar and Settings → Notes | M |
| 21 | Errors and improvements sweep | S |

Tasks 1–3 come before everything else. After that, the order is the spec's build order with two exceptions:
- Toasts (Task 4) move ahead of Timer, because the timer's done toast needs the sticky kind.
- The two scaffolds (Tasks 7 and 8) sit just before the first task that uses them.

## Review Focus

1. **Box focus and keys.** The InlineArguments boxes, the Dropdown and the panel never fight over keys:
   - Esc order: menu, then box, then boxes, then the view.
   - Tab never leaves the bar while boxes show.
2. **No UI-thread work.** These calls never run on the UI thread: HDR status, calendar fetch, Define subtitle lookup, notes search/create/purge, screenshot thumbnails.
3. **Steady live rows.** Ticking accessories never change a row's width, and only redraw while the launcher is visible.
4. **Notes purge.** It removes only notes that are deleted and older than the keep setting.
5. **Focus after expiry.** Blocking stays on until Complete, Extend or × on the notice.

---

### Task 1: Icons, tokens and the dev-instance seam

**Context:** Each icon is an SVG in `assets/icons/`. It is listed twice in `src/design.rs`:
- in `names!` (line 187), which gives `g::NAME`;
- in `icon_files!` (line 548), which embeds the file.

The `@pane/@tile/@xl/@cap` variants are generated, and the test `every_icon_name_is_served` (design.rs ~1256) checks every name. Sizes live as `pub const` in design.rs (lines 75–89) and in the modules `ts`, `r` and `ic`. There are no spacing constants yet.

**Files:**
- `assets/icons/*.svg` (new) and `assets/icons/README.md`.
- `src/design.rs`: `names!`, `icon_files!`, the constants block after `SIDEBAR` (line 89).
- `src/commands.rs`: `ALL` (line 173), `Cmd::tint` (line 319).
- `src/win.rs`: `single_instance` (line 755).
- `src/main.rs`, startup:
  - `keys::configure` :255
  - `browser::register` :423
  - `win::Hotkeys::spawn` :429
  - `set_start_at_login` :2782 and :4074
  - `single_instance` :4030

**New icons.** Each is the Lucide 0.460.0 file under its Lucide name. Get the files with `npm pack lucide-static@0.460.0` into the scratchpad and take them from `icons/`. Reformat each to the house one-line form: `stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"`, no class, no comment, paths unmodified.
- `alarm-clock`, `calendar-clock`, `scan-search`, `image-down`, `sun-medium`, `monitor-cog`
- `shield-check`, `pause`, `circle-check`, `shield-off`, `rotate-ccw`, `coffee`, `list-checks`, `list-plus`, `import`
- `message-circle`, `users`, `circle-play`, `plane`, `newspaper`, `shopping-cart`
- `notebook-pen`, `notebook-tabs`, `square-pen`, `archive-restore`, `square-menu`, `type`, `heading`, `bold`, `quote`, `list-ordered`
- `circle-help`, `plus`, `audio-lines`, `layout-grid`, `clipboard-paste`, `arrow-up-down`, `scan-text`, `cloud`, `ban`, `eye`

**Old stems to reuse** (no new file):

| Lucide name | procast stem |
|---|---|
| `pencil` | `edit` |
| `image` | `picture` |
| `x` | `cancel` |
| `globe` | `web` |
| `refresh-cw` | `restart` |
| `trash-2` | `delete` |
| `clipboard` | `paste` |
| `square-check` | `checkbox-on` |

**Glyphs and tints in `commands::ALL`** (all from `icon-map.md`):

| Command | Glyph | Tint |
|---|---|---|
| Start Timer | `ALARM_CLOCK` | System |
| Start Stopwatch | `TIMER` | System |
| Translate | `LANGUAGES` | Web |
| Define | `BOOK_OPEN` | **Web** (today it falls through to System) |
| My Schedule | `CALENDAR_CLOCK` | System |
| Search Screenshots | `SCAN_SEARCH` | File |
| Paste Latest Screenshot | `IMAGE_DOWN` | Paste |
| Toggle HDR | `SUN_MEDIUM` | System |
| HDR Settings | `MONITOR_COG` | System |

Focus commands, all System tint:

| Command | Glyph |
|---|---|
| Start | `SHIELD_CHECK` |
| Toggle | `SHIELD` |
| Pause | `PAUSE` |
| Resume | `PLAY` |
| Complete | `CIRCLE_CHECK` |
| Cancel | `SHIELD_OFF` |
| Restart | `ROTATE_CCW` |
| Edit | `EDIT` |
| Snooze | `COFFEE` |
| Search Focus Categories | `LIST_CHECKS` |
| Create Focus Category | `LIST_PLUS` |
| Import Focus Categories | `IMPORT` |

**Tokens to add in design.rs.** Each gets a doc comment naming its bundle.css class. Constants:

| Constant | Value |
|---|---|
| `ARG_MIN` | 72 |
| `ARG_MAX` | 200 |
| `FORM_LABEL` | 120 |
| `FORM_ROW` | 500 |
| `FORM_NOTE_INDENT` | 132 |
| `TAG` | 20 |
| `TOKEN` | 22 |
| `TOKEN_FIELD_MIN` | 52 |
| `HUD` | 32 |
| `HUD_TIME` | 24 |
| `NOTICE_W` | 440 |
| `NOTES_W` | 560 |
| `NOTES_H` | 440 |
| `SWATCH` | 20 |
| `FEED_DOT` | 10 |
| `SET_ROW` | 56 |
| `POP_CENTER_W` | 320 |
| `POP_CENTER_TOP` | 64 |
| `POP_RIGHT_W` | 290 |
| `POP_RIGHT_TOP` | 50 |
| `SPLIT_LIST` | 0.4 |

Modules:
- `pub mod sp { S2 = 2, S4 = 4, S8 = 8, S12 = 12, S16 = 16, S24 = 24 }` for `--space-*`.
- `pub mod mix { TAG_FILL = 0.18, TAG_TEXT = 0.72, HUD_FILL = 0.30, MISSING_RING = 0.32 }`.

New code uses these. Old literals stay; that is not this task's job.

**Dev seam.**
- `win::dev() -> bool` = `std::env::var_os("PROCAST_DEV").is_some()`.
- With it set:
  - `single_instance` uses the mutex `Local\procast-dev-instance`.
  - Startup skips `browser::register`, `set_start_at_login`, `win::Hotkeys::spawn` and `keys::configure`. That leaves no global hooks, no registry writes and no hotkey clash with the user's procast.
- Add a comment that `APPDATA` is how the caller points the dev instance at scratch config.

- [ ] **Step 1:** Add the SVGs, the README list and the `names!` / `icon_files!` entries.
  - The README gets a "Files" list: one line per stem, plus the Lucide name for old stems.
  - `cargo test every_icon_name_is_served` passes.
- [ ] **Step 2:** Write the test `feature_commands_follow_the_icon_map` in commands.rs. It holds a table of (Cmd, glyph, tint) for every row above and asserts against `ALL` and `Cmd::tint`. It fails now.
- [ ] **Step 3:** Update `ALL` and `tint()` so the test passes. Add the tokens and the dev seam.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual: start the dev instance with `PROCAST_SHOW=root PROCAST_QUERY=timer`. The user's procast keeps running and its hotkey still works. The Start Timer tile shows the alarm clock.
- [ ] **Step 5:** Commit `feat(design): feature-screen icons, tokens and a dev instance (feature screens)`.

**Done when:**
- Every icon above is served.
- The icon map test passes.
- A dev instance runs beside the user's procast without touching HKCU or global keys.

---

### Task 2: Dropdown menu

**Context:** Many screens need one popover menu:
- InlineArguments `choice` boxes;
- the Translator language set menu and its form selects;
- Focus Duration, Mode and Block;
- the Screenshots filter;
- Settings language pickers;
- the Notes ▾ menus and its Ctrl+K and Ctrl+P lists.

Today, `design::dropdown(label)` (design.rs:1116) draws only the closed trigger: filter icon, label, chevron. There is no open menu.

**Files:**
- Create `src/menu.rs`, for the state.
- `src/design.rs`: add `pop_menu`, `pop_rule` and `pop_row`.
- `src/main.rs`:
  - add the field `menu: Option<menu::Menu>`;
  - add key routing in the `Render` action handlers (~line 3533+): SelectNext, SelectPrev, Launch, Dismiss.

**Interfaces:**
```rust
pub struct Entry { pub id: String, pub label: String, pub icon: Option<&'static str>, pub section: Option<String>, pub sub: String, pub rule_after: bool, pub off: bool, pub two_line: Option<String> }
pub enum Place { Center, Right, Under(f32 /* x */, f32 /* y */) }
pub struct Menu { pub entries: Vec<Entry>, pub search: Option<String> /* placeholder, None = no field */, pub filter: String, pub selected: usize, pub place: Place }
impl Menu {
    pub fn visible(&self) -> Vec<usize>;      // case-insensitive `contains` on label; sections kept only if a child shows
    pub fn step(&mut self, d: isize);         // moves over visible, skips `off`, wraps
    pub fn picked(&self) -> Option<&Entry>;
    pub fn set_filter(&mut self, s: &str);    // resets selected to first visible
}
```

**Look**, from bundle.css `.pc-pop`:
- Container: padding `sp::S4`, gap `sp::S2`, `r::MD`, `bg_elevated`, 1 px `border`, rim + float shadow. Use `design::elevated`, which already has the rim and float.
- Search field: on top, a `design::field` at `CONTROL` height, `sp::S4` below it.
- Section header: `section_header` with `sp::S8` side padding.
- Rows: `selectable` at `CONTROL` height, gap `sp::S8`, icon `ic::MD` in `text_muted`. `sub` sits right-aligned in `text_faint` (`.pc-pop__sub`). An `off` row is in `text_faint`. A `two_line` row is a second line in ts-small `text_muted` with `sp::S4`-ish vertical padding (`.pc-sel--two`, 6 px; add `sp::S6` if needed).
- Rule: 1 px `hairline` with `sp::S4` vertical margin.

**Placement:**
- `Center`: 320 wide at top 64 (`POP_CENTER_W`, `POP_CENTER_TOP`).
- `Right`: 290 wide, right `sp::S12`, top 50.
- `Under`: below the box that opened it, with the box's left edge.

Draw it as an absolute child of the launcher's root, painted last. The list behind stays put; there is no scrim.

**Keys while open:**
- Up / Down: `step`.
- Enter: pick; the opener gets the `Entry.id` through a closure stored with the menu, `on_pick: Box<dyn Fn(&mut Launcher, String, &mut Window, &mut Context<Launcher>)>`.
- Esc: close only.
- Typing goes to the menu's own `EditableTextState` (`menu_input`). Focus returns to the opener on close.
- A click on a row picks it. A click outside closes the menu.

Ruling: gpui has no native popup menu that matches the material, so the menu is drawn inside the launcher window. It cannot spill past the window's edge. Menus are capped at the space below their top, and the list scrolls (`max_h`).

- [ ] **Step 1:** Write tests in menu.rs:
  - `visible` filters by label and keeps a section only when a child matches;
  - `step` skips `off` entries and wraps;
  - `set_filter` reselects the first match;
  - `picked` is `None` when nothing matches.
- [ ] **Step 2:** `cargo test menu` fails.
- [ ] **Step 3:** Implement `menu.rs`, the design helpers and the launcher routing.
  - Opening a test menu needs no UI in this task. Task 3 is the first user, with Translate's To box.
- [ ] **Step 4:** `cargo test` and clippy are clean.
- [ ] **Step 5:** Commit `feat(menu): dropdown menu with search, sections and rules (feature screens)`.

**Done when:**
- The unit tests pass.
- `design::pop_menu` renders from `Menu` with only tokens.
- The launcher routes keys to an open menu before anything else.

---

### Task 3: InlineArguments

**Context:** Today a command that needs input sets `Mode::Args`. `next_argument` (main.rs:2682) then asks one question per view, with the placeholder "question N of M" (`label()` :3005, `placeholder()` :3036). `enum Pending` (main.rs:240: Script, Snippet, RenameDesktop, Focus(Ask, Vec<String>)) carries the answers. Other places:
- `focus::Ask` (focus.rs:419) holds the questions for Focus.
- Root rows come from `search::root` (search.rs:449) and `search::items` (:313).
- The bar is drawn in `Render` (main.rs:3533+).
- Keys: `tab` = SelectNext, `shift-tab` = SelectPrev (main.rs:4040).

**Files:**
- `src/search.rs`: `Arg`, `ArgKind`, `Item.args`, `Item.values`, `with_args`, the root rows.
- `src/main.rs`: the fields `args: Vec<Entity<EditableTextState>>` (≤3), `arg_focus: Option<usize>`, `arg_missing: Option<usize>`, `args_for: Option<String>` (the item key); bar render; keys; launch; remove `Pending` and `next_argument`.
- `src/focus.rs`: delete `Ask` and its `title/count/placeholder/prefill`. Keep `parse_start` and `parse_category`.
- `src/ext.rs`: parse the manifest `arguments`.
- `src/scripts.rs` and `src/snippets.rs`: map their `Arg` to `search::Arg`.
- `src/design.rs`: `arg_box`.

**Interfaces:**
```rust
pub enum ArgKind { Text, Secret, Choice(Vec<(String /*id*/, String /*label*/)>) }
pub struct Arg { pub name: &'static str /* or String */, pub placeholder: String, pub kind: ArgKind, pub optional: bool, pub help: String, pub missing: String }
// Item gains: pub args: Vec<Arg>, pub values: Vec<String> (prefill parsed from the query)
pub fn with_args(item: &Item, values: &[String]) -> Result<Act, (usize, String)>; // Err = (box index, ActionBar message)
pub fn prefill(item: &Item, query: &str) -> Vec<String>; // "timer 25m tea" -> ["25m","tea"]
pub fn mirror(item: &Item, values: &[String]) -> String; // subtitle: "Deep work · 50 min", "25 min · tea"
```

**Argument sources.** All are optional except where marked "required".

| Command | Arguments |
|---|---|
| Start Timer | Duration (required; missing "Type a time: 5m, 1h30m, 1:30"), Name (optional) |
| Start Stopwatch | Name (optional) |
| Translate | Text (required; missing "Type the text to translate"), To (optional choice of `translate::LANGUAGES` names) |
| Define | Word (required; missing "Type a word to define") |
| Start Focus Session | Goal, Minutes, Block (a choice of category names, then "Applications…"); all optional |
| Create Note | Title (optional). Inert until Task 18 wires the command. |
| Rename Desktop | Name (required) |
| Create Focus Category | Name (required), Apps, Sites |
| Import Focus Categories | File (required) |
| Scripts | `scripts::Arg { placeholder, optional }` |
| Snippets with `{argument name=…}` | `snippets::Arg { name, default }`; optional when it has a default |
| Quicklinks with `{query}` | Query (required) |
| Extension commands | manifest `arguments: [{ name, placeholder, type: "text" \| "password" \| "dropdown", required, data: [{title, value}] }]` (Raycast's manifest shape), mapped to Text / Secret / Choice |

Spec quotes used above: the missing message "Type the text to translate" and the placeholders "Goal" and "Type name". The other missing messages are new copy in the same pattern.

**Look**, from bundle.css `.pc-args` / `.pc-arg`:
- Layout: query, then the command icon (`ic::LG` 16 in its tint colour, `.pc-args__icon`), then the boxes. Gap `sp::S8`; the group starts `sp::S4` after the query. The group is capped at 60% of the bar; the query truncates first.
- Box: height `CONTROL`, width clamped to `ARG_MIN`..`ARG_MAX`, padding `0 sp::S8`, inner gap `sp::S4`, `r::SM`, fill `surface`, 1 px `border`, ts-body, `text`; the placeholder is in `text_faint`.
- Focused box: `accent` border plus the ring that `design::field(true)` uses.
- Missing box: `danger` border plus a 2 px ring of `danger` at `mix::MISSING_RING`.
- Optional tag, shown while empty: "optional" in ts-caption, `text_faint` on `surface`, `r::XS`, padding `0 sp::S4`.
- Help mark: `circle-help` at 12 px in `text_faint`, at the box's right edge. Its tooltip (gpui `.tooltip`) is `Arg.help`. Start Focus Session's Goal help is "What are you focusing on?".
- The row's accessory reads "3 arguments" (N = `args.len()`, "1 argument") while it is not selected.
- The row's subtitle mirrors the filled values (`mirror`).
- ActionBar right while boxes show: "Next Box" `Tab`, then the primary label `↵`. For example "Start ↵": "Start Timer"'s verb from `Act::label()`.

Ruling: `text_input` fills its parent. The box's width is set each render from the text width: `window.text_system().shape_line` at ts-body, plus padding, tag and help, clamped to 72..200. Past 200 the input scrolls inside, as `text_input` already does.

Ruling: `text_input` has no password mode. A `Secret` box draws its text in transparent colour with "•" repeated over it. The caret is drawn by the input, so it can sit slightly off the dots. That is acceptable.

**Behaviour:**
- Boxes show only while the selected root row has `args`. Moving the selection drops them; the query stays as typed. Values persist per `args_for` key while the selection returns to that row in the same session.
- Tab from the query goes to box 0, then box to box. Shift+Tab goes back, and from box 0 returns to the query. While boxes show, Tab never selects the next row.
- Typing a space right after an exact command name, alias or keyword moves the caret into box 0. Keywords: `timer`, `t`, `translate`, `tr`, `define`, `def`, or `focus`. The rest of the query pre-fills the boxes through `prefill`:
  - `timer 25m tea` gives `["25m", "tea"]`;
  - `t 90` gives `["90"]`;
  - `timer 1:30` gives `["1:30"]`;
  - `translate guten Morgen` gives `["guten Morgen"]`.
- Space inside a single-word box moves to the next box. Single-word boxes: Duration, Minutes.
- A `Choice` box opens the Task 2 menu (`Place::Under`) on Down or on click. Its search placeholder is "Search…".
- Enter runs `with_args`. On `Err((i, msg))`:
  - box `i` gets the missing ring and focus;
  - the ActionBar shows the error dot and `msg`;
  - nothing runs.
  
  On `Ok(act)` it runs `run(act)`.
- Esc with a box focused returns to the query. A second Esc clears the boxes. After that, the usual Esc.
- `Mode::Args` stays only as "one command row with its boxes focused". It is used by:
  - hotkeys and aliases of commands with arguments;
  - snippet keywords with `{argument}`;
  - Rename Desktop;
  - Task 5's Rename Timer.
  
  It shows that one row and focuses box 0. Delete `Pending`, `next_argument`, every "argument N of M" / "question N of M" string and `focus::Ask`.

- [ ] **Step 1:** Write tests in search.rs:
  - `prefill` for each form above;
  - `with_args`: `Err((0, "Type a time: 5m, 1h30m, 1:30"))` for an empty timer, `Ok(Act::StartTimer(1500, "tea"))` for `["25m", "tea"]`;
  - Translate with To = "German" sets `Job.to = ["de"]`;
  - `mirror` gives "25 min · tea" and "Deep work · 50 min";
  - the "3 arguments" accessory.
  
  Write a test in ext.rs: a manifest with `arguments` gives the three kinds.
- [ ] **Step 2:** `cargo test` fails on the new tests.
- [ ] **Step 3:** Implement the search, ext, scripts and snippets parts, then the launcher bar, keys and launch. Remove the old flow. Each built-in's existing parser (`timers::parse_duration`, `focus::parse_start`, `translate::language`) does the work inside `with_args`.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual, with the dev instance:
  - `PROCAST_QUERY="timer 25m tea"` shows the icon and two boxes, "tea" in Name, and the subtitle "25 min · tea";
  - Enter with the Duration box empty shows the danger ring and "Type a time: 5m, 1h30m, 1:30";
  - Translate's To box opens the menu on Down;
  - Esc, Esc clears the boxes.
- [ ] **Step 5:** Commit `feat(search): inline argument boxes replace question-by-question input (feature screens)`.

**Done when:**
- Every argument source above shows boxes.
- Tab, Shift+Tab, space, Down, Enter and Esc behave as listed.
- No "question N of M" string is left (`grep -rn "question" src/` is clean of it).

---

### Task 4: Toast kinds and timing

**Context:** `src/toast.rs` holds a cloaked PopUp window created once:
- `IN` 150 ms, `HOLD` 4 s (line 21), `OUT` 200 ms;
- `show(cx, slot, ok: bool, text, theme)` at line 51;
- `win::make_overlay` gives NOACTIVATE | TRANSPARENT | LAYERED | TOPMOST.

`Msg::Toast(bool, String)` (win.rs:114) is sent from about 30 places. `design::toast(text, ok, t)` (design.rs:1069) draws a check or a warning icon.

**Files:** `src/toast.rs`, `src/design.rs` (`toast`), `src/win.rs` (`Msg::Toast`, `make_overlay`), `src/keys.rs` (`configure` :149, `hook` :177), `src/main.rs` (every `Msg::Toast(` sender, the handler, `show()` :1383).

**Interfaces:**
```rust
pub enum Kind { Done, Error, Info }          // toast.rs
pub enum Stay { Auto, Sticky }               // Done/Info default Auto (2 s); Error default Sticky
// Msg::Toast(toast::Kind, toast::Stay, String)  — helpers: Msg::done(s), Msg::error(s), Msg::info(s), Msg::sticky(kind, s)
pub fn clip(text: &str) -> String;           // ≤ 60 chars, else 59 + "…"
```

**Look and motion** (Toast README):
- Pill on `bg_elevated` with the rim and float shadows. Height `ROW`, side padding `sp::S12` / `sp::S16`, an `ic::MD` icon, then ts-body_sm text in `text`.
- Icons: Error is the warning triangle in `danger`; Done is the check in `success`; Info is the `info` glyph in `text_muted`.
- In over 150 ms (fade and an 8 px rise), out over 200 ms (fade). This already exists; keep it.
- **Auto** toasts close after 2 s.
- A new toast replaces the one showing.

**Sticky** toasts stay until Esc. They are used for errors, the timer's done toast (Task 5) and meeting reminders (Task 14).

Ruling: a no-focus window never receives Esc. While a sticky toast shows, `keys.rs` installs its low-level hook even when no hyper, tap or expander is configured; add `wanted |= sticky`. The first Esc key-down is swallowed and sends `Msg::ToastClose`.

A sticky toast also closes when:
- it is clicked (sticky toasts drop `WS_EX_TRANSPARENT` and keep `NOACTIVATE`);
- the launcher opens. The launcher then shows the same text in its ActionBar once, with the error dot for errors.

Never a tween beyond the existing in/out.

- [ ] **Step 1:** Write tests in toast.rs:
  - `clip` on 59, 60 and 61 characters;
  - error defaults to Sticky; Done and Info default to Auto.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement:
  - Change `Msg::Toast` and update every sender. `ok=true` becomes `done`; `ok=false` becomes `error`, which is sticky by default.
  - Audit error copy so each says what to do next. Example: "No text selected" becomes "Select some text first". Keep the strings the spec names.
  - Wire the `keys.rs` sticky hook.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev instance with `PROCAST_DEV=1`; the sticky hook is allowed in dev):
  - a done toast closes after 2 s;
  - an error toast stays until Esc or a click;
  - typing in another app is never blocked.
- [ ] **Step 5:** Commit `feat(toast): done, error and info kinds; errors stay until Esc (feature screens)`.

**Done when:**
- All senders use the new kinds.
- Auto toasts close after 2 s; sticky ones close on Esc, on click or when the launcher opens.
- Texts never pass 60 characters.

---

### Task 5: Timer and Stopwatch rows

**Context:**
- `src/timers.rs`:
  - `Entry { id, label, start, length }` has no pause.
  - `fmt_duration` gives "4:32".
  - `from_query` knows only the `timer` keyword.
  - `Timers::{start, stopwatch, cancel, due, matching, rows}`, `rows_changed`.
- `src/main.rs`:
  - `timer_rows` :955 and `watch_timers` :961;
  - `on_timer_tick` :979 sends the toast "Timer done: {label}";
  - `refresh` :735 puts the timer rows into root.
- `search::root` :484 builds "Start a timer for X".

**Files:** `src/timers.rs`, `src/main.rs` (`timer_rows`, `watch_timers`, `on_timer_tick`, `run` :2243 for `StartTimer`/`StopTimer`), `src/search.rs` (`root` :484 timer row; `Act` :31 gains `PauseTimer(u64)`, `RestartTimer(u64)`, `AddTime(u64, u64)`, `RenameTimer(u64)`; `Item.live`), `src/design.rs` (`list_row` :832, tabular accessory).

**Interfaces:**
```rust
// Entry gains: paused_left: Option<Duration> (timers) / paused_at: Option<SystemTime> (stopwatch), saved in timers.json via write-temp-then-rename (fixes the non-atomic save noted in the O ledger)
pub fn fmt_spoken(d: Duration) -> String; // "25 min", "1 h 30 min", "45 s"
pub fn from_query(q: &str) -> Option<(Option<Duration>, String)>; // keywords "timer" and "t"; "timer" alone -> Some((None, ""))
impl Timers { pub fn pause(&mut self, id, now); pub fn resume(&mut self, id, now); pub fn restart(&mut self, id, now); pub fn add(&mut self, id, d); pub fn rename(&mut self, id, name) }
pub fn done_text(label: &str, length: Duration) -> String; // "tea · 25 min done"; unnamed -> "Timer · 25 min done"
```

**Root behaviour:**
- `timer 25m tea`, `t 90` and `timer 1:30` put **Start Timer** first with the Task 3 boxes. Its subtitle is the mirror "25 min · tea".
- A bare `timer` shows Start Timer with the subtitle "Type a time: 5m, 1h30m, 1:30".
- Nothing ranks above it: no Define or web fallback rows when the query is an exact command name or keyword (`search::root`, fallbacks block :510).
- **Running** section heads root when the query is empty or fuzzy-matches "timer". It is always headed, with its count; `display_rows` :208 shows the count for "Running" even in root.
- Running rows:

  | Part | Value |
  |---|---|
  | Title | the name, or "Timer" / "Stopwatch" when unnamed |
  | Subtitle | "Timer" or "Stopwatch" |
  | Icon | `alarm-clock` for a timer, `timer` for a stopwatch; System tint |
  | Accessory, timer | "4:12 left", from `fmt_duration` |
  | Accessory, stopwatch | "12:48" |
  | Accessory, paused | "Paused · 4:12" |

- Live accessory (`.pc-live`): tabular figures in `text_muted`. Add `Item.live: bool`; `list_row` then sets `font_features` `tnum` and a fixed width measured on the widest form "00:00 left", so the row width never changes.
- Redraw at most once a second, and only while the launcher is visible. `watch_timers` returns early when hidden (fixes the ledger's "cx.notify every second").

**Keys on a running row:**
- Enter: pause or resume.
- Ctrl+Enter: Stop.
- Ctrl+K: Restart, Rename, Add 5 min, Stop. Rename uses Task 3 `Mode::Args` with one box Name, prefilled.

**Done:**
- The toast is `done_text` as a sticky Done toast, with `timers::beep()`.
- The row stays with the accessory "Done" in `success` until Enter or Stop.
- Starting a timer shows the toast "Timer set · 5 min · tea" (Toasts block of the contract).

- [ ] **Step 1:** Write tests in timers.rs:
  - `fmt_spoken` for 25 min, 90 min and 45 s;
  - `from_query("t 90")`, `from_query("timer")` and `from_query("timer 1:30")`;
  - pause then resume keeps the time left;
  - `add` adds 300 s;
  - `done_text("tea", 25 min)` is "tea · 25 min done";
  - the save is atomic (a temp file is renamed).
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the timers functions, the root rows, keys, Ctrl+K, the live accessory and the done path.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev): start "timer 1m tea".
  - The Running section shows "tea", "Timer", "0:59 left" ticking without width jumps.
  - Enter pauses it and the row shows "Paused · 0:5x".
  - When it ends, the sticky toast reads "tea · 1 min done".
- [ ] **Step 5:** Commit `feat(timers): running section, pause, rename and done toast (feature screens)`.

**Done when:**
- Every key above works.
- A bare `timer` lists no Define row.
- The Running section shows only while a timer exists and the query is empty or matches.

---

### Task 6: Translator view

**Context:**
- `src/translate.rs`:
  - `Translation { text, detected, to, pronunciation, suggestion, corrected }`;
  - `Job { text, from, to: Vec, all, after }`;
  - `pick_target`, `build_url`, `parse_reply`;
  - `request_error` gives "Could not reach Google Translate";
  - `translate_all`, `run`, `from_query`, `quick_query`.
- `src/main.rs`:
  - `translate()` :1560 and `on_translated` :1585;
  - `refresh` groups the Translate rows under "<lang> to <lang>" sections;
  - `placeholder()` :3036 returns "Enter copies the translation";
  - the split helper uses `base_width*0.4` in `Render`.

**Files:** `src/translate.rs`, `src/main.rs` (Translate mode in `refresh`, `translate`, `on_translated`, `placeholder`, `Render` split and detail), `src/search.rs` (`items` :313 drops "Instant Translate" / "Quick Translate", adds Instant Translate Copy and Instant Translate Paste; `root` Translate rows), `src/commands.rs` (`Cmd::InstantTranslateCopy`, `Cmd::InstantTranslatePaste`, both `languages`, Web), `src/design.rs` (`tag(label, hue, t)`), `src/config.rs` (`[translator]`).

**Config** (replaces today's `[translator]` fields; template lines commented):
```toml
[translator]
from = "auto"            # source language code, or "auto"
primary = "en"           # first target
secondary = ""           # second target, empty for none
action = "copy"          # Enter: "copy" or "paste"; Ctrl+Enter does the other
fill_selection = true    # open Translate with the selected text
cross_first = true       # a target equal to the source goes last
proxy = ""               # HTTP proxy for Google Translate, e.g. http://127.0.0.1:8080
```
`fetch` passes `proxy` to `ureq` (`AgentBuilder::proxy`) when it is set.

**View:**
- Bar: Back, then the source text, with the placeholder "Text to translate…". Task 9 adds the language-set Dropdown on the right; until then, show `design::dropdown` with the label from `set_label(from, [primary, secondary])`, for example "Auto-Detect → English, Spanish".
- Empty source: EmptyState "Type to translate", hint "Or select text anywhere before opening Translate". Its glyph is `languages`.
- List (left, `SPLIT_LIST` 40%): one row per target.
  - Title: the translation. Accessory: the target language name ("English").
  - No sections.
  - Pronunciation row last, when the service gives one: `audio-lines`, title = the pronunciation, subtitle "Pronunciation", accessory = the source language name.
- Same language: when the detected source equals a target, that row moves to the bottom (`cross_first`) and is never selected on open.
- Translate returns the source unchanged when target = source. Show it, but ranked last; do not hide it.
- Detail (right), padding `sp::S16`:
  - The translation in ts-body `text`, selectable, filling the space.
  - Then the `.pc-meta` block: a hairline top border, padding `sp::S12 sp::S16`, rows `sp::S8` apart.
  - "Source Language" (`text_muted`), then tags "Auto-Detect" (System hue, only when `from = auto`) and the detected language (Web hue).
  - "Target Language", then its tag (Paste hue).
- Tag (`.pc-tag`):

  | Property | Value |
  |---|---|
  | Height | `TAG` 20 |
  | Padding | `0 sp::S8` |
  | Radius | `r::XS` |
  | Text | ts-small, weight 600 |
  | Fill | hue at `mix::TAG_FILL` |
  | Text colour | hue mixed 72% into `text` (`mix::TAG_TEXT`) |

**Keys:**
- Enter: Copy Translation, or Paste when `action = "paste"`. Ctrl+Enter: the other.
- Ctrl+K: Paste to Active App, Copy Source, Swap Languages, Open in Google Translate, Change Target.
  - Swap Languages: the query becomes the translation, with a reversed one-off pair; detected becomes the target.
  - Open in Google Translate: `https://translate.google.com/?sl={from}&tl={to}&text={urlencoded}&op=translate`.
  - Change Target: opens the Task 9 set menu; until Task 9 it does nothing.
- Typing re-translates after 300 ms of quiet. A seq counter drops stale replies; `translate_seq` exists. While waiting, the old rows and detail render with `text_faint` text (dimmed). No spinner.
- Offline or error: the ActionBar shows the error dot and "Couldn't reach Google Translate" (change `request_error`); the last result stays.
- Instant Translate Copy and Instant Translate Paste have no view. They read the selection, translate it to `primary` (`secondary` when the source already is `primary`), copy or paste it, then show the toast "Copied translation · German → English" (Paste: "Pasted translation · German → English").
- With `fill_selection`, opening Translate with an empty query reads the selection off the UI thread and puts it in the bar.

- [ ] **Step 1:** Write tests in translate.rs:
  - `set_label("auto", ["en", "es"])` is "Auto-Detect → English, Spanish";
  - `order(rows, detected, cross_first)` puts the same-language row last and the first selectable index skips it;
  - `google_url` encodes text;
  - `request_error` copy;
  - `instant_toast("Copied", "de", "en")` is "Copied translation · German → English".
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the config, view, debounce, dimming, keys, Instant commands and proxy.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev): `PROCAST_SHOW=root PROCAST_QUERY="translate guten Morgen"`, Enter.
  - The split shows a list with "English" and the detail tags Auto-Detect / German / English.
  - With the network off, the ActionBar reads "Couldn't reach Google Translate".
- [ ] **Step 5:** Commit `feat(translate): split view with language tags and instant commands (feature screens)`.

**Done when:**
- The view matches the TranslatorView card.
- No key hint is left in the placeholder.
- Every key works.
- The same-language row goes last.

---

### Task 7: Launcher form scaffold

**Context:** Extension forms already render:
- `render_form` (main.rs:3163): 120 px label column, 520 px column;
- `sync_form` :2103, `focus_field` :2148, `move_field` :2161, `submit_form` :2175, `is_form`;
- fields come from `ext::Field` / `FieldKind`.

Native features need the same: the Focus form (Task 12), Add Calendar (Task 15) and Add Language Set (Task 9).

**Files:** `src/main.rs` (`Mode::Form(FormKind)`; generalise `render_form` / `sync_form` / `move_field` / `submit_form`), `src/design.rs` (`form_row(label, control, t)`, `form_note(text, t)`, `field_with_icon`).

**Interfaces:**
```rust
enum FormKind { Focus, AddCalendar, AddLanguageSet(Option<usize> /* edit */) }
enum Ctl { Text { placeholder: String, icon: Option<&'static str> }, Select(Vec<menu::Entry>, bool /* search */), Tokens(Vec<menu::Entry>), Swatches(Vec<Tint>) }
struct FormField { label: &'static str, ctl: Ctl, value: String /* or picked ids, comma-joined */ }
```
`render_form` takes a `&[FormField]`. Ext forms map their `ext::Field` into it.

**Look** (bundle.css `.pc-form--center`):
- The form is centred with `sp::S24` top padding. Rows are `FORM_ROW` 500 wide, aligned to the top, gap `sp::S12`.
- Labels: `FORM_LABEL` 120, right-aligned, at `CONTROL` height, in ts-body_sm `text_muted`.
- Controls fill the rest; they are `design::field` at `CONTROL`. A trailing or leading icon is `text_muted`.
- Note: `form_note` indents `FORM_NOTE_INDENT` 132, ts-body_sm `text_faint`.
- Bar: Back only. Use the existing `back_tile`; no query text.
- ActionBar: the left text is the form's title; the right is the primary action with Ctrl+Enter. No Actions.

**Keys:**
- Tab / Shift+Tab move between fields.
- Down on a Select or Tokens field opens the Task 2 menu under it.
- Ctrl+Enter submits.
- Esc closes an open menu, otherwise goes back.

- [ ] **Step 1:** Write a test for `form_values` (pure): it collects text and picked ids by label. Write a test that moving past the last field wraps.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Refactor the ext form onto the scaffold with no visual change for ext forms, except width 500 / label 120 per the spec. Add the empty `FormKind` arms; Tasks 9, 12 and 15 fill them.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual: an extension form (any installed ext with a form) still works and submits.
- [ ] **Step 5:** Commit `feat(launcher): shared form scaffold for native forms (feature screens)`.

**Done when:**
- Ext forms work on the scaffold.
- `Mode::Form` exists with select, tokens and swatches controls, and the menu opens from them.

---

### Task 8: Settings row recipe and feature sections

**Context:** `src/settings.rs` has:
- 13 `SECTIONS` (line 120) and `SECTION_ICONS` (line 122);
- `Kind` (line 25);
- `Row { label, help, path, kind, group }`;
- `row_view` (line 1109) draws label + help + control, with no icon except Command rows;
- `choices` (line 1348), a segmented control;
- the Games, Focus and Calendar groups sit inside the **Launcher** section (lines 244–271);
- `Kind::Status` shows the file index status only.

**Files:** `src/settings.rs`, `src/foreground.rs` (an `AtomicBool RUNNING` set while the hook thread lives), `src/design.rs` (`set_row`).

**Changes:**
- **Row recipe** (bundle.css `.pc-setrow`): add `Row.icon: &'static str`.
  - Layout: icon (`ic::LG`, `text_muted`, untinted), then the title in ts-body, then `help` below it as the description in ts-body_sm `text_muted` (gap 2), then the control on the right.
  - Size: min height `SET_ROW` 56, padding `sp::S8 sp::S16`, gap `sp::S12`, hairlines between rows, inside a `surface` group with `r::MD`.
  - Group label: ts-small weight 600 `text_faint`, padding `sp::S8 sp::S16 sp::S4`.
  - Rows without an icon keep today's look until Task 21's audit gives them one.
- **New sections**, after Extensions. Their icons are the commands' icons, by "a setting that controls a command takes the command's icon":

  | Section | Icon |
  |---|---|
  | Translator | `languages` |
  | Screenshots | `scan-search` |
  | Focus | `shield-check` |
  | Calendar | `calendar-clock` |
  | Notes | `notebook-pen` |
  | Games | `gamepad-2` |

  Each page starts with a header: the section's sticker tile, its name in ts-heading, and a one-line description in `text_muted` (`.pc-sett__head`, gap `sp::S12`). Calendar's description is "Today's meetings in search, with a one-key join." Each later task gives its own line.
- Move the Games, Focus and Calendar groups out of Launcher into their sections. Tasks 11, 13 and 15 rewrite their rows.
- `Kind::Minutes(&'static [u32])`: a segmented control that writes an integer. `Kind::Choice` writes strings.
- `Kind::Language`: a dropdown trigger (`design::dropdown`) that opens a Task 2-style menu inside the Settings window, with search and the language names. The Settings window gets its own `menu: Option<menu::Menu>` and routes keys the same way.
- **Advanced:** a read-only row "Foreground watcher: running" (`Kind::Status`). It reads "Foreground watcher: stopped" when `foreground::RUNNING` is false. Icon `eye`.
- `PROCAST_OPEN_SETTINGS=<page>` accepts the new names.

Ruling: icon-map.md's "Settings sections" table has no rows for the new sections. Their icons follow the rule "a setting that controls a command takes that command's icon".

- [ ] **Step 1:** Tests in settings.rs:
  - every row in the six new sections has an icon;
  - `Kind::Minutes` writes an integer through `config::write_value`;
  - the section list contains the six new names in order.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the row recipe, sections, moves, `Minutes`, `Language` with the Settings menu, and the foreground row.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual: `PROCAST_DEV=1 APPDATA=… PROCAST_OPEN_SETTINGS=Advanced` shows "Foreground watcher: running".
- [ ] **Step 5:** Commit `feat(settings): row recipe with icons and feature sections (feature screens)`.

**Done when:**
- The six sections exist with headers.
- Rows follow the 56 px recipe.
- The Advanced foreground row shows the live state.

---

### Task 9: Translator language sets and Settings → Translator

**Context:**
- Task 6 left a static Dropdown label and a no-op Change Target.
- `config::edit_table(text, list, TableEdit::{Add, Duplicate, Remove})` (config.rs:1029) edits top-level `[[list]]` tables of string fields.
- `config::write_value` (config.rs:985) sets one value.

**Files:** `src/translate.rs` (`LangSet`), `src/config.rs` (`[[language_set]]`, `[translator] current`), `src/main.rs` (Dropdown in the Translate bar; Ctrl+P; `Mode::LanguageSets`; `FormKind::AddLanguageSet`), `src/settings.rs` (Translator section rows).

**Config:**
```toml
[translator]
current = 0              # index into [[language_set]]; the first set is built from from/primary/secondary
[[language_set]]
from = "auto"
to1 = "en"
to2 = "es"               # "" for none
```
Set 0 is always `from / primary / secondary`, so the Settings rows keep their meaning.

**Behaviour:**
- Bar: a Dropdown on the right with the current set's label, for example "Auto-Detect → English, Spanish". Ctrl+P and a click open a `Place::Right` menu (290 wide, top 50):
  1. search field "Search…";
  2. **Manage Language Sets…** (`edit`, which is Lucide pencil);
  3. a rule;
  4. every set, the current one selected.
  
  Picking a set makes it current (writes `current`) and re-translates at once.
- Empty-view Enter switches back to the previous set (kept in memory). The ActionBar right label reads "Previous Language Set".
- `Mode::LanguageSets` list:
  - placeholder "Search language sets…";
  - first row **Add Language Set…** (`plus`);
  - then the sets. The current set has the `check` icon and the accessory "Current".
  - Enter on a set: Make Current. Ctrl+K: Make Current, Edit, Delete (danger, asks first).
- Add form (`FormKind::AddLanguageSet`, Task 7 scaffold). ActionBar left "Add Language Set".
  - Source language: a Select with search; "Auto-Detect" first.
  - Target language 1: a Select with search.
  - Target language 2: a Select with search; "None" first.
  
  Ctrl+Enter adds the set and makes it current. Edit uses the same form, prefilled.
- **Settings → Translator.** Header line: "Translate text with Google Translate, from the launcher or the selection."

  | Row | Control | Icon |
  |---|---|---|
  | Translate from | `Kind::Language` with Auto-Detect | `languages` |
  | Primary language | `Kind::Language` | `languages` |
  | Secondary language | `Kind::Language` with None | `languages` |
  | Fill with selected text | Bool, on | `text-cursor-input` |
  | Default action | Choice `copy` / `paste`, shown "Copy" / "Paste" | `clipboard-paste` |
  | Cross-language results first | Bool, on | `arrow-up-down` |
  | HTTP proxy | Text | `network` |

  Then the existing Translate commands with alias and hotkey, as other sections do.

- [ ] **Step 1:** Tests in translate.rs:
  - `LangSet::label` with and without `to2`;
  - set 0 is built from the `[translator]` fields;
  - adding through `edit_table` then `current` points at the new set;
  - deleting the current set falls back to 0.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the menu wiring, the sets list, the form, Change Target (opens the menu) and the Settings rows.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - Ctrl+P opens the menu with "Manage Language Sets…".
  - Add a set "Auto-Detect → Japanese" with Ctrl+Enter; it becomes current and the list re-translates.
  - Settings → Translator shows seven rows with icons.
- [ ] **Step 5:** Commit `feat(translate): language sets and Settings → Translator (feature screens)`.

**Done when:**
- Sets can be added, edited, deleted, made current and picked from the menu.
- Previous Language Set works from the empty view.
- The Settings page matches the spec's list.

---

### Task 10: Define view

**Context:**
- `src/dict.rs`:
  - `Pos::name()` gives "Adjective";
  - `Sense { pos, gloss, examples, synonyms }`;
  - `WordNet::lookup`, `knows`, `only_known`;
  - `define()` (downloads ~16 MB on first use);
  - `spelling()` uses ISpellChecker;
  - `installed()` runs `read_dir` and is called on the UI thread.
- `src/main.rs`:
  - `define()` :1621 and `on_defined` :1633;
  - `refresh` builds a "Did you mean" section, or senses with `section = pos.name()`;
  - `placeholder()` returns "Enter copies the definition".
- `search::root` builds a `Define "{word}"` row tinted System.

**Files:** `src/dict.rs`, `src/main.rs` (Define mode, split detail, keys, root subtitle lookup), `src/search.rs` (root Define row: Web tint; subtitle = first sense).

**Behaviour:**
- Root: the Define row (`book-open`, Web) takes argument Word (Task 3). Its subtitle is the first short sense when known; otherwise it is empty.
  - The lookup runs on a background task, cached by word in `HashMap<String, Option<String>>`, with a seq to drop stale replies.
  - Only when `dict::installed()` is cached true. Cache `installed()` once per show, off the UI thread (fixes the ledger item).
  - "Short" means the gloss cut at the first `;`, max 60 chars.
- View: placeholder "Word to define…". The bar keeps the word, and typing looks up again after 300 ms.
- Left list (40%): senses sectioned by part of speech with counts ("Noun 1", "Adjective 2"; `section_header(title, Some(count))`). Order: Noun, Verb, Adjective, Adverb. Then a "Did you mean" section when there are suggestions.
  - Title: the gloss. Icon `book-open`, Web.
  - When the word is found, "Did you mean" holds only `spelling()` suggestions that differ from the word.
- Right pane:
  - the word in ts-display;
  - "noun · 1 sense · WordNet" in `text_faint`, with a lowercase pos and "1 sense" / "N senses" for that pos;
  - the gloss in ts-body;
  - each example in `text_muted` in straight quotes `"…"`;
  - "Similar: fluke, luck" (the synonyms joined with ", ") as plain text in `text_muted`.
- Misspelled (no senses, with suggestions): "Did you mean" is the only section and **its header shows** (always headed in `display_rows`). The closest match is selected (first suggestion), and the bar still shows what was typed.
- Unknown (nothing at all): EmptyState `No definition for 'xyz'`, with the word filled in. Hint "Check the spelling, or search the web". Action **Search the Web** on Enter.
- Not installed: EmptyState "Download Dictionary" with its size. Today's download path stays.
- Enter: Copy Definition (copies the selected gloss). Ctrl+K:
  - Copy Word;
  - Define Similar Word (opens a menu of the synonyms, then defines the pick);
  - Open in Browser (`https://en.wiktionary.org/wiki/{word}`).

Ruling: "Open in Browser" names no site. Wiktionary is used because it is free and keyless.

- [ ] **Step 1:** Tests in dict.rs:
  - `short_gloss` cuts at `;` and 60 chars;
  - `meta_line(Pos::Noun, 1)` is "noun · 1 sense · WordNet";
  - `group(senses)` orders by pos with counts;
  - `similar_line(["fluke", "luck"])` is "Similar: fluke, luck";
  - `missing_title("xyz")` is "No definition for 'xyz'".
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the view, keys, root subtitle and the `installed()` move.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev, dictionary downloaded in the scratch APPDATA):
  - `define serendipity` shows the split with a "Noun 1" header;
  - `define serendipty` shows only "Did you mean" with its header;
  - `define xyzq` shows the EmptyState.
- [ ] **Step 5:** Commit `feat(dict): split Define view with Did you mean and Web tint (feature screens)`.

**Done when:**
- The Define view matches the DefineView card.
- The root subtitle shows the first sense without blocking typing.
- No key hint is left in the placeholder.

---

### Task 11: HDR rows, Auto HDR toast, Settings → Games and the foreground row

**Context:**
- `src/hdr.rs`:
  - `HdrDisplay { adapter, id, gdi, supported, enabled }` has no friendly name;
  - `displays()`, `set()`;
  - `toggle()` returns "HDR on/off" or Err("No HDR display");
  - `next(auto, game, ours, hdr_on)`;
  - `Auto::start()` runs its own thread with no `tx`; `Auto::foreground(auto, game, hwnd)`.
- `commands::run` :350 calls `toggle`.
- `foreground::watch(tx)` sends foreground events.

**Files:** `src/hdr.rs`, `src/commands.rs` (`Cmd::ToggleHdr` row subtitle), `src/main.rs` (HDR status cache; root "Auto HDR" section; `Item.toggle`), `src/search.rs` (`Item.toggle: Option<bool>`, `Act::SetAutoHdr(bool)`), `src/settings.rs` (Games section rows), `src/design.rs` (`list_row` accessory may be a `toggle`).

**Interfaces:**
```rust
// HdrDisplay gains: name: String  (DisplayConfigGetDeviceInfo DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, monitorFriendlyDeviceName)
pub fn subtitle(displays: &[HdrDisplay]) -> String; // "On · DELL U2723QE" | "Off · DELL U2723QE" | "Off · 2 displays" | "No HDR display"
pub fn bar_label(displays: &[HdrDisplay]) -> &'static str; // "Turn HDR Off" | "Turn HDR On"
pub fn switch_text(on: bool, app: &str) -> String; // "HDR on for Elden Ring"
```
The display text rule:
- one HDR-capable display: its name;
- several: "N displays", where N counts the supported ones;
- none: "No HDR display".

**Behaviour:**
- **Toggle HDR** (`sun-medium`, System):
  - subtitle from `subtitle()`;
  - ActionBar primary label from `bar_label()`;
  - after it runs, a done toast "HDR on" / "HDR off".
  - With no HDR display, the row stays with the subtitle "No HDR display", and Enter shows the error toast "No HDR display · connect an HDR monitor".
  - The status is read with `hdr::displays()` on a background task each time the launcher shows and after a toggle. It is cached in `hdr_status: Vec<HdrDisplay>`, never read on the UI thread.
- **HDR Settings** (`monitor-cog`, System), subtitle "Windows Settings".
- Root: when the query fuzzy-matches "hdr", an **Auto HDR** section shows one row:
  - title "Auto HDR for games";
  - subtitle "On while a full-screen game is in front";
  - icon `gamepad-2`, System;
  - accessory: a `design::toggle(on)` (`Item.toggle = Some(cfg.games.auto_hdr)`).
  
  Enter flips it: it writes `games.auto_hdr` with `config::write_value`, and the toggle redraws.
- Auto HDR switch toast: `Auto` gets the `tx`. When it turns HDR on for a game, it sends the done toast `switch_text(true, foreground::app_label(exe))`, for example "HDR on for Elden Ring", once per launch. The key is the PID; remember the last toasted PID.
- **Settings → Games.** Header line "Turn HDR on by itself while you play."

  | Row | Control | Icon |
  |---|---|---|
  | Auto HDR for games | Bool; help "On while a full-screen game is in front" | `sun-medium` |
  | Games | List; keep today's help | `gamepad-2` |

  The foreground row lives in Advanced (Task 8).

- [ ] **Step 1:** Tests in hdr.rs, on fixture `HdrDisplay` values:
  - `subtitle` for each of the four cases;
  - `bar_label`;
  - `switch_text`;
  - once-per-launch: the same PID twice gives one toast (pure helper `should_toast(last, pid)`).
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the friendly name, status cache, rows, toggle accessory, Auto toast and Settings rows.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev): typing "hdr" shows Toggle HDR with "On · …" or "Off · …" or "No HDR display", and the Auto HDR section with a toggle that flips.
- [ ] **Step 5:** Commit `feat(hdr): state in the row, Auto HDR toggle and switch toast (feature screens)`.

**Done when:**
- Toggle HDR always shows its state and is never hidden.
- Auto HDR can be flipped from root and from Settings → Games.
- The switch toast fires once per game launch.

---

### Task 12: Focus form and running row

**Context:**
- `src/focus.rs`:
  - `Mode { Block, Allow }`;
  - `Session` (title, mode, apps, sites, until, paused_left, snoozed, …) with `row()` giving (title, "X left" / "Paused · X");
  - `parse_start`, `category`, `app_label`, `minutes_left`;
  - `Ask` is removed in Task 3.
- `src/main.rs`:
  - `focus_rows` :1077 builds the root row with Pause/Resume, Edit, Restart, Complete, Cancel;
  - `focus_begin` :1104, `focus_cmd` :1168, `focus_end` :1228;
  - `on_focus_tick` :1246 ends the session on expiry.
- Config `[[focus_category]] name, apps, sites`.

**Files:** `src/focus.rs`, `src/main.rs` (`FormKind::Focus`, `focus_rows`, `focus_cmd`, keys), `src/config.rs` (built-in categories), `src/design.rs` (`token`, `token_field`), `src/win.rs` (running apps for the Applications section: reuse the Running mode's app list).

**Built-in categories** (`focus::BUILT_IN`, overridden by a `[[focus_category]]` of the same name):

| Category | Icon | Apps | Sites |
|---|---|---|---|
| Messaging | `message-circle` | `discord.exe`, `slack.exe`, `telegram.exe`, `whatsapp.exe`, `signal.exe` | `discord.com`, `slack.com`, `web.whatsapp.com`, `web.telegram.org`, `messenger.com` |
| Social | `users` | — | `facebook.com`, `instagram.com`, `x.com`, `twitter.com`, `tiktok.com`, `reddit.com`, `linkedin.com`, `threads.net`, `bsky.app` |
| Streaming | `circle-play` | `spotify.exe` | `youtube.com`, `netflix.com`, `twitch.tv`, `primevideo.com`, `disneyplus.com` |
| Travel | `plane` | — | `booking.com`, `airbnb.com`, `expedia.com`, `skyscanner.net` |
| News | `newspaper` | — | `news.google.com`, `cnn.com`, `bbc.com`, `nytimes.com`, `theguardian.com` |
| Shopping | `shopping-cart` | — | `amazon.com`, `ebay.com`, `etsy.com`, `aliexpress.com` |
| Gaming | `gamepad-2` | `steam.exe`, `epicgameslauncher.exe`, `battle.net.exe`, `riotclientservices.exe` | `store.steampowered.com`, `epicgames.com` |

User categories use `list-checks`.

**Interfaces:**
```rust
pub fn parse_minutes(s: &str) -> Option<Option<u32>>; // "90"->Some(Some(90)), "1h30"->90, "1 hour 30 minutes"->90, "50 min"->50, "No limit"->Some(None), junk->None
pub fn blocking_line(s: &Session) -> String;          // "Blocking 3 categories" | "Blocking 1 app" | "Allowing 2 categories"
pub fn extend(&mut Session, now);                     // +5 min
```

**Form** (`FormKind::Focus`, Task 7 scaffold). It opens when Start Focus Session runs with Goal empty; with Goal filled it starts at once.
- Goal: Field, placeholder "Goal".
- Duration: Field with the `clock` icon. It parses free text with `parse_minutes`. Down opens the menu: 25 min, 50 min, 1 hour, 1 hour 30 minutes, No limit.
- Mode: Select; menu with a search field. Options: "Block Apps & Websites", "Allow Apps & Websites".
- Block, the **token field**:
  - Field: min height `TOKEN_FIELD_MIN` 52, padding `sp::S4 sp::S8`, a chevron at the right.
  - Tokens: height `TOKEN` 22, padding `0 sp::S4 0 sp::S8`, gap `sp::S4`, `r::XS`, `surface_strong`, weight 600, ts-body_sm. Each token: icon 12 (`ic::SM`) in `text_muted`, the name, then × (`cancel` 12).
  - Menu: search "Search…", then sections **Focus Categories** (the built-ins plus your own) and **Applications** (running and installed apps by exe). When typed text looks like a host (contains a dot), a "Block {host}" / "Allow {host}" row shows at the top.
  - Picked items leave the menu and become tokens. Backspace in the empty search removes the last token.
- ActionBar: "Focus" on the left; **Start Focus Session** with Ctrl+Enter on the right; no Actions.
- Edit Focus Session opens the same form, prefilled.

**Running row.** A **Focus** section at the top of root (above Running) when a session exists:
- title: the goal;
- subtitle: `blocking_line`, for example "Blocking 3 categories";
- icon `shield-check`, System;
- live accessory "23:10 left" or "Paused · 23:10" (Task 5 `Item.live`).

Keys on the row:
- Enter: pause or resume.
- Ctrl+Enter: Complete.
- Ctrl+K: Edit, Extend, Restart, Cancel.
- The ActionBar right shows "Pause ↵ | Complete Ctrl ↵ | Actions". Add `Item.bar_second: Option<String>`, so the ActionBar shows a second action with its keys when set.

- [ ] **Step 1:** Tests in focus.rs:
  - `parse_minutes` for every form above;
  - `blocking_line` singular and plural;
  - a user category named "Social" overrides the built-in;
  - `extend` adds 300 s;
  - token pick and remove round-trip (pure `Picks` vec helper).
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the form, token field, menus, running row, keys and `bar_second`.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - "Start Focus Session" with empty boxes, Enter: the form shows with right-aligned labels;
  - pick Messaging and Social;
  - Ctrl+Enter: root shows the Focus row with "Blocking 2 categories" and "24:59 left".
- [ ] **Step 5:** Commit `feat(focus): one form with token field and a live session row (feature screens)`.

**Done when:**
- One form replaces the questions.
- Duration parses all three spec forms.
- Category icons follow the map.
- The Focus row shows and ticks.

---

### Task 13: Focus pill, notice, blocked page and Settings → Focus

**Context:**
- `focus_end` (main.rs:1228) ends the session on expiry, and `on_focus_tick` :1246.
- The toast is the model for a no-focus overlay window: `toast.rs` with `win::make_overlay` (win.rs:228) and `place_toast` (:332).
- `browser/background.js` has `focusRule` and `blockedHost` and redirects to `blocked.html?site=`. `browser/blocked.js` writes "{site} is blocked by procast Focus" as plain text.
- `focus::sites_message` (focus.rs:260) sends the rules to the browser.

**Files:** Create `src/hud.rs`. Modify `src/main.rs` (open and close the hud on session start and end; the notice instead of auto-end), `src/win.rs` (`drag_window(hwnd)`: `ReleaseCapture` + `SendMessage(WM_NCLBUTTONDOWN, HTCAPTION)`), `browser/background.js`, `browser/blocked.html`, `browser/blocked.js`, `src/focus.rs` (`sites_message` carries `snooze` minutes), `src/config.rs` (`[focus] pill = true`), `src/settings.rs` (Focus section rows).

**Pill.** A PopUp window, NOACTIVATE + TOPMOST but **not** TRANSPARENT, so it can be clicked and dragged. It sits at the top centre of the primary monitor, `sp::S8` from the top.
- Body (`.pc-hud`): height `HUD` 32, padding `0 sp::S12 0 sp::S4`, gap `sp::S8`, `r::MD`, `bg_elevated`, 1 px `border`, rim + float, weight 600, ts-body_sm.
- Time capsule: height `HUD_TIME` 24, padding `0 sp::S8`, `r::SM`, fill System hue at `mix::HUD_FILL`, tabular figures, for example "23:10". The goal follows it.
- Updates once a second.
- Mouse down then move: `win::drag_window`. A click without moving opens the launcher with the Focus row selected.
- Hidden when `[focus] pill = false`, a config key only. Comment: "false hides the floating Focus timer".

**Notice** on expiry. The session stays blocking; the tick no longer auto-ends it.
- Window: a PopUp at the bottom-right of the primary monitor's work area, `sp::S24` in. NOACTIVATE, clickable.
- Body (`.pc-notice`): width `NOTICE_W` 440, padding `sp::S12`, gap `sp::S12`, `r::LG`, `bg_elevated`, border, rim + float.
- Content: the Focus `shield-check` sticker tile; title "Focus session is up." (weight 600 `text`); subtitle "Extend it if you need more time, or complete the session." (`text_muted`).
- Buttons **Extend** (Quiet) and **Complete** (Primary), stacked on the right with gap `sp::S4` and `margin-right` `sp::S16`.
- Close ×: `cancel`, `text_muted`, absolute top `sp::S8` / right `sp::S8`. × completes the session.
- Sound: `focus::beep` when `sound` is on.

Ruling: the README places the notice "at the corner of the screen" without naming one. It uses the bottom-right, where Windows notifications sit.

**Blocked app toast:** "Discord is blocked · Deep work", built as `app_label(exe) · goal`. It is a Done-kind Auto toast.

**Blocked page** (`browser/blocked.html` + `blocked.js`):
- Centred column on the page background `bg_solid`: the Focus icon (inline `shield-check` SVG, 40 px, `text_muted`); "instagram.com is blocked by procast" in 20 px weight 600; a primary button **Snooze for 3 minutes**, where 3 comes from `snooze`; at the bottom, "End your focus session to open this page." in `text_muted`.
- The button posts `{type: "snooze", site}` to background.js. That allows the host for `snooze` minutes, then navigates back.
- With snooze 0, the button is hidden.
- Colours are copied from `tokens.json` as CSS variables at the top of `blocked.html`. Each carries a comment naming its token, with light and dark through `prefers-color-scheme`. Font: system UI, because the extension cannot load Space Grotesk without bundling it.

Ruling: the extension page cannot read `design.rs`, so the token values are copied with their names.

**Settings → Focus.** Header line "Block distracting apps and websites while you work."

| Row | Control | Icon |
|---|---|---|
| Snooze duration | `Kind::Minutes(&[1, 3, 5, 10])`, default 3 | `coffee` |
| Allow all websites | Bool; help "When a browser is allowed but no sites are listed" | `web` |
| Reopen closed apps | Bool; help "When the session ends" | `rotate-ccw` |
| Play sound | Bool | `volume-2` |

`focus.default_minutes` stays a config key without a row. Then the Focus commands with alias and hotkey. Then a **Focus categories** group: one row per built-in with its icon, plus "Your categories" from config; read-only names.

- [ ] **Step 1:** Tests:
  - focus.rs: `sites_message` includes `snooze`; `pill_text(left)` is "23:10"; expiry no longer ends the session (a session past `until` with `expired()` true stays in the store until complete);
  - `win::` helpers: none;
  - JS: none. Manual check instead.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement `hud.rs` (the pill and notice windows, created once and cloaked when unused, like the toast), drag, the expiry flow, the blocked page and the Settings rows.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - start a 1-minute session: the pill shows at the top centre and can be dragged;
  - when time is up, the notice shows with Extend and Complete;
  - Extend adds 5 min.
  
  Browser page: the user checks it with their companion (the dev instance skips registration).
- [ ] **Step 5:** Commit `feat(focus): timer pill, time-up notice, blocked page and Settings → Focus (feature screens)`.

**Done when:**
- The pill and notice match the FocusForm card.
- Blocking continues until Complete, Extend or ×.
- The blocked page shows the spec copy and Snooze works.
- Settings → Focus lists the four rows.

---

### Task 14: My Schedule list

**Context:**
- `src/calendar.rs`:
  - `Event { start, end, title, join, all_day }` has no calendar name or UID;
  - `Calendar` has feeds, fetched, fetching, failed and fired;
  - `REFRESH` is 600 s;
  - `due()` gives the reminder text "{title} starts in N min";
  - `within`, `today_and_tomorrow`, `near`, `day_label`;
  - `span` gives "All day" or "HH:MM – HH:MM";
  - `row()` (line 462) uses icon `g::CALENDAR` with subtitle = span;
  - `fetch`, `link_host`, `join_link`.
- `src/main.rs`:
  - `schedule_now` :1017 (root, empty query);
  - `schedule_rows` :1025 (its empty state has Edit Config);
  - `calendar_tick` :1041.
- Config `[calendar] ics: Vec<String>, tolerance_secs, confirm_join, remind_minutes, ignore`.

**Files:** `src/calendar.rs`, `src/config.rs` (`[[calendar_feed]]`), `src/main.rs` (Schedule mode, root Next section, reminder), `src/commands.rs` (`Cmd::JoinMeeting`, `video`, System, title "Join Meeting"), `src/search.rs` (`Act::OpenEvent`, `Act::HideEvent`).

**Config** (replaces `ics` and `tolerance_secs`):
```toml
[calendar]
refresh_minutes = 15     # how often the ICS feeds are fetched: 5, 15, 30 or 60
remind_minutes = 5       # notice before meetings with a link: 0 (off), 2, 5, 10, 15 or 30
confirm_join = true      # ask before opening the meeting link
next_in_search = true    # show the next meeting within 15 minutes at the top of search
ignore = []              # events whose title contains one of these words are hidden
[[calendar_feed]]
url = ""                 # the calendar's secret ICS address
name = "Work"
colour = "system"        # system, window, paste, web or calc
```

**Interfaces:**
```rust
// Event gains: feed: usize, location: String, uid: String
pub fn service(join: &str) -> Option<&'static str>; // meet.google.com -> "Google Meet", teams.microsoft.com|teams.live.com -> "Teams", zoom.us -> "Zoom", webex.com -> "Webex"
pub fn subtitle(feed_name: &str, e: &Event) -> String; // "Work · Google Meet" | "Work"
pub fn sections(events: &[Event], now: i64) -> Vec<(&'static str, Vec<Event>)>; // "Now", "Today", "Tomorrow"; past today dropped; all-day first in their day
pub fn until(e: &Event) -> String;   // "until 11:00"
pub fn in_minutes(e: &Event, now: i64) -> String; // "in 8 min"
pub fn calendar_url(e: &Event) -> Option<String>; // google -> https://calendar.google.com/calendar/r/day/Y/M/D ; outlook -> https://outlook.live.com/calendar/0/view/day/Y/M/D
```
Hidden events live in `config::dir()/calendar-hidden.json` as a list of `"{uid or start|title}"`.

**View:**
- Sections Now, Today and Tomorrow, each with a count (always headed). All-day events sit at the top of their day with the accessory "All day".
- Row:
  - title;
  - subtitle from `subtitle()`, for example "Work · Google Meet";
  - accessory "14:00 – 14:30" (live, tabular);
  - icon `video` when it has a join link, else `calendar-clock`; System tint.
- The now-event is selected on open, and its accessory reads "until 11:00" in `accent`, weight 600 (`.pc-now`). Add `Item.now: bool` for `list_row`.
- Enter: Join Meeting when there is a link (with the `confirm_join` dialog). Otherwise Open in Calendar: `calendar_url`, else Copy Details.
- Ctrl+K: Copy Join Link, Copy Details, Hide This Event.
  - Copy Details: title, span, location and link, one per line.
- Loading: the last events show at once. The ActionBar left shows "Updated 5 min ago", from the last fetch.
- Root, when `next_in_search` is on and an event starts within 15 min: a **Next** section heads root with that row, accessory "in 8 min".
- Reminder: `remind_minutes` before an event with a link. The sticky info toast reads "{title} · in 5 min". The next launcher open selects the Next row, so Enter joins.
- `Cmd::JoinMeeting`: joins the now or next event that has a link. If there is none, the error toast "No meeting with a link right now".
- Empty (no feeds): EmptyState "No calendar connected", hint "Paste your calendar's ICS address to see today's meetings here", glyph `calendar-clock`. Enter is **Add Calendar** (Task 15). Until then it opens Settings → Calendar.
- `REFRESH` comes from `refresh_minutes`. Fetch on the background executor, as today.

- [ ] **Step 1:** Tests in calendar.rs:
  - `service` for each host;
  - `subtitle` with and without a link;
  - `sections` drops past events, keeps all-day first and counts;
  - `until` and `in_minutes`;
  - `calendar_url` for Google and Outlook hosts and `None` otherwise;
  - hidden events are filtered.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the config, Event fields, view, root Next section, reminder, `JoinMeeting` and the Hide store.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev): point a `[[calendar_feed]]` at a local ICS file served by `python -m http.server` in the scratchpad.
  - Sections, the "until" accent and the Next section show.
  - Hide This Event removes the row.
- [ ] **Step 5:** Commit `feat(calendar): Now / Today / Tomorrow schedule with join and hide (feature screens)`.

**Done when:**
- The list matches the ScheduleList card.
- The empty state no longer mentions config.toml.
- The Next section and reminder work.

---

### Task 15: Add Calendar and Settings → Calendar

**Context:**
- Task 14 moved feeds to `[[calendar_feed]] url, name, colour`.
- `calendar::fetch(url)` returns `Result<Vec<Event>, String>`.
- The Task 7 form scaffold has `Ctl::Swatches`.
- `config::edit_table` adds and removes `[[list]]` tables.

**Files:** `src/main.rs` (`FormKind::AddCalendar`), `src/calendar.rs` (per-feed `updated: Option<Instant>` shared as `Arc<RwLock<Vec<FeedStatus>>>`), `src/settings.rs` (Calendar section; `Kind::Feeds`), `src/design.rs` (`swatch(tint, on, t)`, `feed_row`).

**Add Calendar form.** It opens from the Schedule empty state, from Ctrl+K in My Schedule ("Add Calendar") and from Settings.
- ICS address: Field with the `link` icon, focused first, placeholder "https://calendar.google.com/…/basic.ics".
- Name: Field, placeholder "Work".
- Colour: five swatches, the kind tints System, Window, Paste, Web and Calc.
  - Each swatch: `SWATCH` 20 round, inset 1 px `hairline`.
  - The selected swatch: a 2 px `bg_solid` ring, then a 4 px `accent` ring.
  - Left and Right move between swatches.
- Note (`form_note`): "Google: Settings → your calendar → Secret address in iCal format. Outlook: Settings → Shared calendars → Publish → ICS."
- ActionBar: "Add Calendar" on the left, **Add Calendar** with Ctrl+Enter on the right.
- Ctrl+Enter fetches once on the background executor.
  - Success: `edit_table` adds the feed, the form goes to My Schedule, and the toast reads "Added Work".
  - Failure: the ActionBar shows the error dot and "Couldn't read this calendar", and the form stays.

**Settings → Calendar.** Header: tile plus "Calendar" and "Today's meetings in search, with a one-key join."
- **Calendars** group (`Kind::Feeds`, icon `calendar-clock`). One `feed_row` per feed:
  - height `ROW`, padding `0 sp::S16`, gap `sp::S12`, hairline top;
  - a dot `FEED_DOT` 10 in the feed colour;
  - the name, weight 600;
  - "Google · updated 5 min ago" in `text_muted`. The provider comes from the URL host: Google, Outlook, iCloud, else the host. "updated N min ago" comes from the shared status; "not fetched yet" before the first fetch.
  - a `delete` icon in `text_faint` on hover or selection, which removes the feed with `edit_table`.
  
  The last row is "Add Calendar…" (`plus`, `text_muted`). It sends `Msg` to the launcher to open the form.
- Rows (description = the help line, verbatim):

  | Row | Control | Help | Icon |
  |---|---|---|---|
  | Refresh every | `Minutes(&[5, 15, 30, 60])`, 15 | "How often the ICS feeds are fetched" | `restart` (Lucide refresh-cw) |
  | Remind me before | `Minutes(&[0, 2, 5, 10, 15, 30])`, 5; segment label for 0 is "Off" | "A notice with Join, before each meeting with a link" | `bell` |
  | Confirm before joining | Bool | "Ask before opening the meeting link" | `video` |
  | Next meeting in search | Bool | "Show the next meeting within 15 minutes at the top of search" | `calendar-clock` |
  | Ignore events with | Text | "Comma-separated words; matching events are hidden"; placeholder "lunch, focus time" | `ban` |

Ruling: "Ignore events with" is a comma-separated Text field, per the spec. Config keeps `ignore` as a list. The row splits on commas when writing and joins with ", " when reading (`Kind::Csv`).

- [ ] **Step 1:** Tests:
  - calendar.rs: `provider("https://calendar.google.com/…")` is "Google"; `updated_text(Some(5 min))` is "updated 5 min ago";
  - settings.rs: `Kind::Csv` round-trips "lunch, focus time" to `["lunch", "focus time"]`;
  - the swatch tint list is exactly System, Window, Paste, Web, Calc.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the form, the fetch-on-submit, the shared status, the Settings page and the Msg to open the form.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - a bad URL shows "Couldn't read this calendar" and the form stays;
  - the local ICS URL adds the feed;
  - Settings → Calendar lists it with "updated 0 min ago".
- [ ] **Step 5:** Commit `feat(calendar): Add Calendar form and Settings → Calendar (feature screens)`.

**Done when:**
- Feeds can be added from the launcher and Settings and deleted from Settings.
- All six Calendar rows match the spec copy.

---

### Task 16: Screenshot grid

**Context:**
- `src/shots.rs`:
  - `Shot { path, mtime, text, failed }` has no `pinned`;
  - `ShotIndex::{load, update, search, prune}`;
  - `rescan(folders, keep, all_media, ocr, show)`;
  - `rows()` (line 280): title = file name, subtitle = the OCR first line, accessory = age, File tint.
- `src/main.rs`:
  - `scan_shots` :1647, `on_shots` :1663, `shot_to_clipboard` :1682;
  - `file_preview(key)` :1476 runs a worker with a 100 ms settle, a `file_seq` check and keys `p:` / `i:`;
  - the emoji grid: `grid()` :3368 (emoji only), `grid_view` :3372 (`gpui::list`), `grid_rows` :3432, `grid_move` :3351.

**Files:** `src/shots.rs`, `src/main.rs` (Shots in `grid()`; thumbnail tiles; banner; filter dropdown), `src/config.rs` (`[screenshots] columns = 3`), `src/design.rs` (`thumb(img, caption, on, t)`, `banner`).

**Interfaces:**
```rust
// Shot gains: pinned: bool (serde default)
pub enum Field { Any, Name, Text, Date(DateQ) }
pub enum DateQ { Today, Yesterday, Day(i32,u32,u32), Month(i32,u32) }
pub fn parse_filter(q: &str) -> Vec<(Field, String)>;      // "name:inv text:total date:yesterday foo"
pub fn matches(shot: &Shot, f: &[(Field, String)], now: SystemTime) -> bool;
pub fn caption(mtime: SystemTime, now: SystemTime) -> String; // "Today, 14:59" | "Yesterday, 18:07" | "12 Nov, 09:50"
pub fn text_hit(shot: &Shot, words: &[String]) -> Option<String>; // the matched words, for a text: caption
pub enum Kind { All, Screenshots, Recordings, Pinned }      // recordings = video extensions when all_media
// Msg::Shots gains progress: Option<(usize, usize)>
```

**View:**
- Grid: `columns` across, 3 by default, 3–6 in Settings.
  - Padding `0 sp::S16 sp::S8`, gap `sp::S12`.
  - One section **Screenshots & Recordings** with the total count formatted with a thousands comma ("1,050").
- Thumbnail (`.pc-thumb`):
  - Image: 16/10 aspect, `r::MD`, `surface` fill, inset 1 px `hairline`, cover-fit (`ObjectFit::Cover`).
  - Selected: a 2 px `accent` ring and no fill change.
  - Caption: ts-body_sm weight 600, `text_muted` (selected: `text`), single line with ellipsis, gap `sp::S4`. The caption is `caption()`, or `text_hit` when the filter has `text:`.
- Thumbnails: key `t:{path}`, decoded at 320 px wide on the background executor. They skip the 100 ms settle that previews use, and are cached in `file_images` with the same seq guard.
- Placeholder: "Filter screenshots by name:, text: or date:…". Plain words search name and text. `date:yesterday`, `date:2026-11` and `date:2026-11-12` work.
- Dropdown on the right of the bar, with the `list` icon instead of the filter icon (`design::dropdown` gets an icon parameter): All, Screenshots, Recordings, Pinned.
- Banner under the bar while OCR runs (`.pc-banner`):
  - height `ROW`, padding `0 sp::S16`, gap `sp::S8`, bottom hairline;
  - the `scan-text` icon in `text_muted`, then "Reading text in screenshots", then the count "94 of 996" in `text_faint`.
  - Static: it updates with progress messages and has no animation.
- ActionBar: "Search Screenshots" on the left; "Copy to Clipboard ↵" and "Actions Ctrl K" on the right.
- Empty: EmptyState "No screenshots found", hint = the folders searched joined with ", ", action **Add Folder**, which opens Settings → Screenshots.
- Grid keys: arrows move (`grid_move`), Enter copies.

- [ ] **Step 1:** Tests in shots.rs:
  - `parse_filter` for each field;
  - `matches` on `date:yesterday` and `date:2026-11`;
  - `caption` for today, yesterday and older (fixed `now`);
  - `text_hit`;
  - `Kind::Pinned` filters;
  - count formatting "1,050";
  - the old index without `pinned` still loads.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Generalise `grid()` / `grid_rows` for Shots with the column count. Implement thumbnails, captions, the filter, the dropdown, the banner and progress.
  - Remove the per-keystroke allocations the ledger noted, by caching lowercase name and text in `Shot` at load.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev, `folders` pointed at a scratch folder with a few PNGs):
  - the grid shows three across with "Today, …" captions;
  - `text:` shows the matched words;
  - the banner shows during the first scan.
- [ ] **Step 5:** Commit `feat(shots): thumbnail grid with captions, filters and OCR banner (feature screens)`.

**Done when:**
- The grid matches the ScreenshotGrid card.
- Titles no longer truncate to "Screenshot 20…".
- Filters and the dropdown work.

---

### Task 17: Screenshot actions, Quick Look, Paste Latest and Settings → Screenshots

**Context:**
- The Ctrl+K panel is `panel: Option<(Item, usize)>`, drawn by `design::action_panel` (320 × max 320) and `panel_row(glyph, label, keys, on, danger, t)`.
- Today, panel rows show keys only for row 0 (↵) and row 1 (Ctrl ↵): `Render` ~3533, `act_glyph` :3488.
- `Cmd::PasteLatestScreenshot` calls `shot_to_clipboard(None, true)`.

**Files:** `src/main.rs` (panel keys, filter, Quick Look), `src/search.rs` (`Act::OpenWith(String)`, `Act::QuickLook(String)`, `Act::PinShot(String, bool)`, `Act::CopyText(String)`, `Act::DeleteShot(String)`; `act_keys(act) -> Option<&'static str>`), `src/shots.rs` (`pin`, `delete` to the Recycle Bin via `SHFileOperationW` with `FOF_ALLOWUNDO`), `src/settings.rs` (Screenshots section), `src/config.rs` (`[screenshots] cloud = false`, `columns`).

**ActionPanel:**
- Header: the file name.
- Rows:

  | Action | Keys | Glyph |
  |---|---|---|
  | Copy to Clipboard | Enter | |
  | Paste | Ctrl+Enter | |
  | Open | Ctrl+O | `file` |
  | Open With… | Ctrl+Shift+O | `app`; `OpenAs_RunDLL` through `rundll32 shell32.dll,OpenAs_RunDLL <path>` |
  | Quick Look | Ctrl+Y | `eye` |
  | Show in File Explorer | Ctrl+Alt+O | `folder-search` |
  | Pin / Unpin | | `pin` |
  | Copy Text | | `copy`; the OCR text |
  | Delete | | danger, `delete`; asks first with `design::dialog` |

- Every panel row now shows its KeyCaps from `act_keys`, for every panel, not only Screenshots. The shortcuts also work on the selected item without opening the panel. Add the key bindings in the launcher context: `ctrl-o`, `ctrl-shift-o`, `ctrl-y`, `ctrl-alt-o`.
- Panel filter: when the panel has more than 6 actions, a search field sits at the panel's bottom edge and filters by label. Typing goes to it while the panel is open.

**Quick Look** (Ctrl+Y):
- The picture large over the list, contain-fit, inside the window body.
- Under it, the name (weight 600), then "2.4 MB · Today, 14:59 · Screenshots" in `text_muted` (size, `caption()`, folder name).
- Esc or Ctrl+Y closes it. Arrows move to the next shot.

**Paste Latest Screenshot** (`image-down`, Paste) pastes and shows the done toast "Pasted Screenshot 14:59", with the time from mtime as HH:MM.

**Settings → Screenshots.** Header line "Find screenshots by the words in them."

| Row | Control | Icon |
|---|---|---|
| Column count | `Kind::Columns`: four small grid pictures for 3, 4, 5 and 6, each a `segment` holding a 3–6-cell mini grid drawn with divs | `layout-grid` |
| Folders to search | List (add, remove) | `folder` |
| Include all media | Bool | `picture` |
| Text recognition | Bool | `scan-text` |
| Read cloud-only files | Bool, off; help "Downloads OneDrive files to read them" | `cloud` |
| Keep screenshots for | Choice; help "Pinned screenshots are kept" | `clock` |

Read cloud-only files: with it off, `rescan` skips files with `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`.

Keep screenshots for: prune never drops pinned shots.

- [ ] **Step 1:** Tests:
  - search.rs: `act_keys` for each action above;
  - the panel filter keeps matching labels;
  - shots.rs: `prune` keeps pinned shots; a recall-on-access file is skipped when `cloud` is false (pure attribute check helper);
  - `paste_toast(mtime)` is "Pasted Screenshot 14:59".
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the panel keys and filter, the bindings, Quick Look, the actions, delete with confirm, Paste Latest copy and the Settings rows.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - Ctrl+K on a shot shows nine actions with keys and a bottom filter;
  - Ctrl+Y shows Quick Look;
  - Pin then prune keeps it;
  - Settings → Screenshots shows six rows and the column picker.
- [ ] **Step 5:** Commit `feat(shots): actions with shortcuts, Quick Look and Settings → Screenshots (feature screens)`.

**Done when:**
- Every panel shortcut works and shows its KeyCaps.
- Quick Look, Pin and Delete (with confirm) work.
- The Paste Latest toast matches the spec.

---

### Task 18: Notes window, commands and store wiring

**Context:**
- `src/notes/store.rs`:
  - `NoteMeta { id, pinned, created, modified, deleted_at, title }`;
  - `Store::{open, list, deleted, text, create -> Result<Id, Exists>, duplicate, save, pin, delete, restore, purge, history, search}`;
  - `title_of`.
- `src/notes/editor.rs`: `Buffer` and `spans`.
- `src/main.rs:21` reads `#[cfg(test)] mod notes;` ("Task 11 wires…"), so the module is not built into the app yet.

Deferred notes from the O ledger, all handled here or in Task 19:
- remove `#[cfg(test)]` on `mod notes;`;
- cache `spans()` per line (Task 19);
- purge must guard live notes;
- store search and create off the UI thread.

**Files:** `src/main.rs` (`mod notes;` without cfg; commands; `Mode::Notes(NotesList)`), create `src/notes/window.rs`, `src/notes/store.rs` (`opened`, the `purge` guard, `purge_expired`, id validation), `src/commands.rs`, `src/config.rs` (`[notes]`), `src/win.rs` (`set_topmost(hwnd, on)`).

**Config:**
```toml
[notes]
folder = ""              # one markdown file per note; empty = %APPDATA%\procast\notes
keep_deleted_days = 30   # recently deleted notes are removed after this many days
```

**Store changes:**
- `NoteMeta.opened: i64` (serde default 0), set by a new `Store::open_note(id, now)`.
- `purge(id)` returns `Err` unless the note is deleted. This is the guard.
- `purge_expired(now, days)` purges deleted notes older than `days`.
- Ids are validated as digits only before they touch a path (ledger item).
- The store lives in `Arc<Mutex<Store>>`. `search`, `create`, `purge_expired` and file reads run on `cx.background_executor()` and post results back.

**Commands** (all Paste tint):

| Command | Icon | Behaviour |
|---|---|---|
| Open Notes | `notebook-pen` | opens the window on the last opened note, or a new one |
| Search Notes | `notebook-tabs` | `Mode::Notes(All)` in the launcher, same rows as Task 20's browse list; Enter opens the note in the window |
| Create Note | `square-pen` | Task 3 argument Title, optional; creates a note with `# Title` when given; `Exists` opens the empty one |
| Recently Deleted Notes | `archive-restore` | `Mode::Notes(Deleted)`; Enter restores and opens; Ctrl+K: Restore, Delete Permanently (danger, asks first) |

**Window** (`notes/window.rs`, entity `NotesWindow`):
- Size and place:
  - size `NOTES_W` × `NOTES_H` (560×440), resizable, using `design::glass_window`;
  - no OS title bar, so the header is the drag area (`WindowControlArea::Drag`);
  - always on top while visible: `win::set_topmost` after open;
  - it reopens where it was: bounds saved to `config::dir()/notes-window.json` on move, resize and close.
- Header, height `CONTROL`, padding `0 sp::S12`, three columns:
  - left: icon buttons Actions (`square-menu`, Ctrl+K), Browse (`notebook-tabs`, Ctrl+P) and New (`plus`, Ctrl+N), `text_muted`, gap `sp::S12`;
  - centre: the title (`title_of`, "Untitled" when empty) in ts-body_sm weight 600 `text_muted`;
  - right: close × (`cancel`).
- Body: padding `sp::S16`. In this task, a plain `text_input`-free render of the text lines. Task 19 adds the editor. The placeholder "Start writing…" is in `text_faint`.
- Footer, height `CONTROL`, three columns, no border:
  - centre: "186 characters" (char count, "1 character") in ts-small `text_faint`;
  - right: the `type` toggle (Task 20 shows the bar).
- Esc hides the window. Autosave saves 500 ms after the last edit on the background executor, and on hide.
- `purge_expired` runs once at startup, in the background.

- [ ] **Step 1:** Tests in store.rs:
  - `purge` of a live note is `Err` and the file stays;
  - `purge_expired` removes only deleted notes older than N days;
  - `opened` round-trips and an old `notes.json` without it loads;
  - a non-digit id is rejected.
  
  Window bounds JSON round-trip.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the store changes, the commands and modes, the window shell, topmost, saved bounds, autosave and background work.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - Open Notes shows a 560×440 glass window with the header and footer;
  - it stays on top of other windows;
  - close and reopen keeps its place;
  - Create Note "Groceries" makes a note titled Groceries.
- [ ] **Step 5:** Commit `feat(notes): Notes window and commands on the note store (feature screens)`.

**Done when:**
- `mod notes;` is built into the app.
- The four commands work.
- Purge cannot touch live notes.
- No store call runs on the UI thread.

---

### Task 19: Notes editor rendering

**Context:**
- `src/notes/editor.rs`:
  - `Buffer::{new, text, cursor, anchor, selection, undo, redo, set_cursor, insert, backspace, delete, newline, left/right/home/end/word_left/word_right/up/down, toggle_mark, set_block, toggle_task}`;
  - `Style { heading, bold, italic, code, strike, underline, highlight, link, task }`;
  - `Span { range, style }` (marker characters belong to their span);
  - `spans(line)`, with `STYLE_MAX` 2048.
- Task 18 left the body as plain lines.

**Files:** `src/notes/editor.rs` (`marks(line) -> Vec<Range<usize>>`; `SpanCache`), `src/notes/window.rs` (body render, keys, mouse).

**Interfaces:**
```rust
pub fn marks(line: &str) -> Vec<Range<usize>>; // byte ranges of markdown markers: "# ", "**", "*", "~~", "`", "==", "<u>", "</u>", "> ", "- ", "1. ", "- [ ] ", "- [x] ", link "[", "](url)"
pub struct SpanCache { lines: Vec<(u64 /* hash */, Vec<Span>)> }
impl SpanCache { pub fn get(&mut self, i: usize, line: &str) -> &[Span]; } // recomputes only when the hash changed
pub fn display(line: &str, hide: bool) -> (String, Vec<usize> /* display->source byte map */);
```

**Render:**
- Each line is a `StyledText` with `HighlightStyle`s from its spans:

  | Style | Rendering |
  |---|---|
  | Heading 1 / 2 / 3 | ts-display / ts-heading / ts-body weight 600 |
  | Bold | weight 600 |
  | Italic | italic |
  | Strike | strikethrough, `text_faint` |
  | Underline | underline |
  | Highlight | Paste hue fill at `mix::TAG_FILL` |
  | Code | Cascadia Mono on `surface` |
  | Link | `accent` |
  | Quote | `text_muted` with a 2 px left bar in `hairline` |
  | Task | a 14 px box (`r::XS`, 1.5 px `text_muted` border; done: `accent` fill), clickable to `toggle_task`; a done line is `text_faint` and struck |

- Marks are hidden except on the cursor's line: `display(line, hide = i != cursor_line)`.
- The cursor is a 1.5 px `accent` caret with the existing caret blink. The selection uses `sel_ring` at the input's selection colour.
- Typing: `on_key_down` uses `keystroke.key_char` for text. Mapped keys:
  - Enter: `newline`; Backspace, Delete;
  - arrows, Home, End, and Ctrl+arrows;
  - Shift extends the selection;
  - Ctrl+Z / Ctrl+Y: undo / redo;
  - Ctrl+A;
  - Ctrl+C / Ctrl+X / Ctrl+V as plain text through the existing clipboard helpers;
  - Mark shortcuts (Raycast's): Ctrl+B Bold, Ctrl+I Italic, Ctrl+U Underline, Ctrl+Shift+S Strike, Ctrl+E Code, Ctrl+Shift+M Highlight, Ctrl+Shift+B Quote, Ctrl+Alt+C Code block, Ctrl+Alt+0 Paragraph, Ctrl+Alt+1..3 H1–H3.
- Mouse: a click maps x through `shape_line` positions and the display map to a source offset. A drag extends the selection.
- The body scrolls when longer than the window.

Ruling: IME composition is not supported in this task. Typing goes through `key_char`, which covers Latin and dead-key input. The upgrade path is implementing gpui's `EntityInputHandler` for `NotesWindow`. Add `// ponytail: key_char only, EntityInputHandler for IME`.

- [ ] **Step 1:** Tests in editor.rs:
  - `marks` for each marker;
  - `display(line, true)` strips marks and the map points back to source bytes;
  - `SpanCache` recomputes only the changed line (count calls through a test hook);
  - a heading line keeps the heading style when hidden.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement `marks`, `display`, `SpanCache` and the body render, keys and mouse.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - type `# Title`, then `**bold**` on the next line; moving off a line hides its marks;
  - Ctrl+B wraps the selection;
  - clicking a task box ticks it;
  - undo works.
- [ ] **Step 5:** Commit `feat(notes): live markdown editor with hidden marks (feature screens)`.

**Done when:**
- Notes render as typed, with marks only on the cursor's line.
- Every key in the list works.
- Spans are cached per line.

---

### Task 20: Notes action list, browse list, formatting bar and Settings → Notes

**Context:**
- Task 18 built the window shell (header buttons, footer with the `type` toggle).
- Task 19 built the editor (`Buffer::toggle_mark`, `set_block`).
- Task 2 built `menu::Menu` and `design::pop_menu`.

**Files:** `src/notes/window.rs`, `src/settings.rs` (Notes section).

**Ctrl+K action list.** A `Place::Center`-style menu floating over the note, with search "Search for actions…" on top:

| Action | Keys |
|---|---|
| New Note | Ctrl+N |
| Duplicate Note | |
| Pin Note | Ctrl+Shift+P; "Unpin Note" when pinned |
| Browse Notes | Ctrl+P |
| Go Back | Ctrl+[ |
| Go Forward | Ctrl+] |
| Copy as Markdown | |
| Delete Note | danger |

- Rows show KeyCaps.
- Go Back and Go Forward are `off` (`text_faint`) when there is no history in that direction.
- The Actions header button opens the same list.

**Ctrl+P browse list.** A menu that drops from the header, under it at the header's left:
- search "Search notes…", then a **Notes** section;
- two-line rows:
  - the current note: "● Current · 186 characters";
  - other notes: "Opened 2 days ago · 397 characters", from `opened`;
- Pin (`pin`) and Delete (`delete`) icons on the selected or hovered row (`.pc-notes__rowtools`, `text_muted`);
- order: pinned first, then last opened; Recently Deleted at the end, as one row that opens the launcher's Recently Deleted mode;
- the search runs `Store::search` in the background.

Ago text: "just now", "N min ago", "N hours ago", "yesterday", "N days ago".

**History:** a back/forward stack of note ids per window session.

**Formatting bar:** the `type` toggle in the footer replaces the footer with the bar (`.pc-notes__bar`).
- Bar: height `CONTROL`, centred, gap `sp::S8`, `text_muted`; ▾ glyphs at 10 px.
- Items:

  | Item | Icon | Action |
  |---|---|---|
  | Heading ▾ | `heading` | menu Heading 1, Heading 2, Heading 3, Paragraph |
  | Text style ▾ | `bold` | menu Bold, Italic, Underline, Strikethrough, Highlight |
  | Link | `link` | wraps the selection as `[text](url)` with the caret in `url` |
  | Image | `picture` | inserts `![](...)` with the caret inside |
  | Code | `code` | inline code |
  | (a 1 px hairline divider) | | |
  | Quote | `quote` | |
  | Check list | `checkbox-on` (Lucide square-check) | |
  | List ▾ | `list` | menu Bulleted List (`list`), Numbered List (`list-ordered`) |
  | × | `cancel` | closes the bar, at the right edge |

Ruling: the spec names the formatting bar's items but not their icons, and icon-map.md has no Notes formatting rows. They take the matching Lucide icons above, per "one icon per idea".

**Settings → Notes.** Header line "Quick notes in a floating window."

| Row | Control | Icon |
|---|---|---|
| Notes folder | Text with a folder picker button (reuse the Files section's folder row if one exists, else Text) | `folder` |
| Keep deleted notes for | `Minutes`-style segmented of days 7, 30, 90, shown "7 days" / "30 days" / "90 days"; default 30 | `archive-restore` |

Then the four Notes commands with alias and hotkey.

- [ ] **Step 1:** Tests:
  - `ago(now, then)` for each bucket;
  - browse ordering (pinned first, then `opened` descending);
  - history back and forward and the `off` state;
  - `char_count_text(1)` is "1 character" and `(186)` is "186 characters".
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the two menus, history, the formatting bar and its menus, and the Settings rows.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev):
  - Ctrl+K lists eight actions, with Go Back faint;
  - Ctrl+P shows "● Current · N characters";
  - the bar's Heading ▾ makes an H2;
  - Settings → Notes shows two rows.
- [ ] **Step 5:** Commit `feat(notes): action list, browse list, formatting bar and Settings → Notes (feature screens)`.

**Done when:**
- Ctrl+K, Ctrl+P, Ctrl+N, Ctrl+Shift+P, Ctrl+[ and Ctrl+] work.
- The bar formats.
- Settings → Notes saves both keys.

---

### Task 21: Errors and improvements sweep

**Context:** These are the "Errors and improvements found while mapping" items not closed by Tasks 1–20:
- the fallback order;
- the Windows Focus page outranking procast's Focus commands;
- icons and tints per `icon-map.md`.

Code involved:
- `search::root` (search.rs:449), with fallbacks at :510;
- `config` default `fallbacks ["web", "chatgpt", "ai", "define"]` (config.rs:672) and the template line :716;
- `tier()` (search.rs:410: exact title = 1000);
- `SETTINGS_PAGES` rows keyed `ms:{page}` with the System tint.

**Files:** `src/search.rs`, `src/config.rs`, `src/commands.rs`, `src/settings.rs` (row icons).

**Changes:**
- **Fallbacks:**
  - Drop `"define"` from the default `fallbacks` (it stays allowed when listed).
  - Never add the Define fallback when the query's first word is an exact command title, alias or keyword (`timer`, `t`, `translate`, `tr`, `define`, `def`, `focus`).
  - Update the template comment.
- **Ranking:** at an equal tier, procast commands rank above Windows Settings rows (`key` starting `ms:`). So "focus" puts Start Focus Session above the Windows Focus page. Do it with a tie-break in the sort, not by changing `tier`.
- **Icon audit:**
  - Walk `icon-map.md`'s Commands tables for the o-tools features and the "Settings rows" tables for Translator, Focus, Focus categories, Screenshots, Notes and Calendar.
  - Fix any glyph or tint that still differs (Tasks 1, 9, 11, 13, 15, 17 and 20 should have covered most).
  - Add the icon-map icon to any settings row in the six new sections that lacks one.
- **Leftovers:** grep for "question", "Enter copies", "Edit Config" in Schedule, and "Timer done:". Each must be gone.

- [ ] **Step 1:** Tests in search.rs:
  - `root("timer")` has no Define row;
  - `root("focus")[0]` is Start Focus Session and the `ms:` Focus page ranks below it;
  - the default config's fallbacks are `["web", "chatgpt", "ai"]`;
  - a configured "define" fallback still shows for `serendipity`.
  
  Update `define_offers_a_row_for_the_query_and_as_a_fallback` and `launcher_favorites_hidden_sensitivity_and_fallbacks` to the new defaults.
- [ ] **Step 2:** The tests fail.
- [ ] **Step 3:** Implement the fallbacks, the tie-break, the audit fixes and the leftover cleanup.
- [ ] **Step 4:** Run `cargo test` and clippy. Manual (dev): `PROCAST_QUERY=focus` shows Start Focus Session first; `PROCAST_QUERY=timer` shows no Define row.
- [ ] **Step 5:** Commit `feat(search): fallback order, Focus ranking and icon-map audit (feature screens)`.

**Done when:**
- Every item in the spec's "Errors and improvements" list is fixed. Each one maps to a task:

  | Item | Task |
  |---|---|
  | Key hints in placeholders | 6, 10 |
  | Same-language source returned unchanged | 6 |
  | Did you mean header | 10 |
  | Define fallback | 21 |
  | Screenshot titles | 16 |
  | HDR state | 11 |
  | Focus ranking | 21 |
  | Focus questions | 3, 12 |
  | Schedule empty state | 14 |
  | Notes command and window | 18 |
  | Tints and icons | 1, 21 |

- The icon-map audit finds no difference.

---

## Wrap-up

- [ ] Run `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo build --release` in the worktree; all clean.
- [ ] Walk every card in the artifact's Feature screens group against the dev instance in Dark and Light. Note any gap as a follow-up; do not fix it silently.
- [ ] Update `.superpowers/sdd/2026-11-16-feature-screens/progress.md` with what shipped and what was deferred: IME, secret-box caret, Quick Look zoom.
