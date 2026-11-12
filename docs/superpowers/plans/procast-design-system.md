# Plan: implement the procast design system in code

**Spec:** `docs/design/system/` — a copy of the published procast Design System (`README.md` is the
brand book, `tokens.json` holds every exact value, `components/<Name>/README.md` the component
rules, `components/<Name>/preview.html` + `components/bundle.css` the reference rendering, and
`notes-*.md` the reasoning). The spec is the binding authority. Where this plan and the spec
disagree, the spec wins.

**Goal:** the running app looks as close to the design system as gpui on Windows allows.

**Branch:** `design-system` (baseline commit `ee57e8c` holds earlier redesign WIP).

## Global Constraints

- UI framework is `gpui-ce = 0.3.0` (crate path `gpui`). No new crate dependencies.
- Every colour, text size, radius, icon size and stroke comes from `src/design.rs`; views
  (`src/main.rs`, `src/settings.rs`, others) never write a literal colour, radius, text size or
  icon size. Values are copied exactly from `docs/design/system/tokens.json`.
- Two themes only: **Dark** and **Light** (`[theme] mode = system|dark|light`, system default).
- **Selection is one neutral grey everywhere** (launcher rows, ActionPanel rows, the calculator
  card, the Settings sidebar): a lozenge filled `sel_top` → `sel_bottom` (top to bottom), a 1 px
  `sel_ring` edge, a highlight along its top, and a 3 × 16 px pill in solid `accent` on the left.
  It never takes a kind hue or an app icon's colour.
- **Colour only in icons:** kind tints (`tint_*`) colour an icon and its tile, and the small kind
  icon in the ActionBar. Nothing else. `accent` is monochrome (= text colour). No purple anywhere.
- Typeface: **Space Grotesk** for everything (weights 400, 600; 500 for KeyCap labels),
  Cascadia Mono for code, Segoe UI Emoji for emoji. Seven text steps only: caption 11, small 12,
  body_sm 13, body 14, heading 16, display 20, query 22. Display and query tracked -0.01em
  where gpui allows letter spacing; skip tracking if it does not.
- Icons: the 60 Lucide outlines in `assets/icons/*.svg` (stroke 1.75 baked in), drawn in a token
  colour. Stroke 1.25 at 40 px (`ic::XL`), 2 inside a KeyCap. No Segoe Fluent Icons glyphs remain.
- Sizes: bar 56, row 40 (row box 36), footer 40, control 32, tile 28, cap 20, list_pad 4,
  window width 750 before scaling. Radii r-xs 4, r-sm 6, r-md 8, r-lg 12, r-xl 16.
  Icon sizes ic-sm 12, ic-md 14, ic-lg 16, ic-xl 40. Spacing 2/4/8/12/16/24.
- Resource budget (spec README "Resource budget"): no in-app blur, no animation besides the 500 ms
  caret blink, no images; at most three blurred shadows on screen (`depth` + `float`s); nothing in
  the list casts a shadow. Gradients only where the spec names them (window sheen, window rim,
  selected lozenge).
- Glass only on the launcher window, ActionPanel and Dialog. The Settings window is opaque
  `bg_solid` with the same controls.
- Contrast: tests in `design.rs` keep every text pair at 4.5:1 and icons on tiles at 3:1 in both
  themes (see spec README "Contrast").
- Build: `cargo build` and `cargo test` must pass at the end of every task. Commit at the end of
  every task with a normal message ending in
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Copy (strings) follows spec README "Content fundamentals" wherever a task touches a string.

## Task 1: Font and vector icon infrastructure

Files: `src/design.rs`, `src/main.rs` (startup only), `src/search.rs` (the `Icon` enum), and every
call site the compiler points at.

1. Embed `assets/fonts/SpaceGrotesk[wght].ttf` with `include_bytes!` and register it at startup with
   `cx.text_system().add_fonts(...)` before any window opens. Set `FONT_UI`, `FONT_DISPLAY` and
   `FONT_SMALL` to `"Space Grotesk"` (or collapse them into one `FONT_UI` constant and update uses).
   Verify the family name the font reports actually resolves (text must not fall back silently);
   say in the report how you verified it.
2. Implement a `gpui::AssetSource` in `design.rs` that serves the 60 files in `assets/icons/`
   from `include_bytes!` (an explicit list or a `macro_rules!` list; no build script, no new crate).
   Paths: `icons/<name>.svg` (stroke 1.75 as stored), `icons/<name>@xl.svg` (same file with
   `stroke-width="1.25"`), `icons/<name>@cap.svg` (with `stroke-width="2"`). Register it with
   `Application::with_assets` (or the gpui-ce equivalent) in `main()`.
3. Replace `fluent(glyph: char, size, color)` with `icon(name: &'static str, size: f32, color: Hsla)`
   returning a gpui `svg()` element of that size in that colour; it picks the `@xl` variant when
   `size >= ic::XL`. Add `icon_cap(name, color)` for use inside KeyCaps (`@cap`, `ic::SM`).
4. Change every `g::` constant from `char` to `&'static str` holding a Lucide file stem from
   `assets/icons/` (e.g. `g::SEARCH = "search"`, `g::APP = "app"`). Map each old Fluent meaning to
   the nearest of the 60 icons; `assets/icons/` names are semantic (`dock-left`, `sleep`,
   `sign-out`, `switch-apps`, `personalize`, `checkbox-on`, …). Remove `FONT_ICON`.
5. Rename `Icon::Fluent(char)` in `src/search.rs` to `Icon::Vector(&'static str)` and fix every use.
   Extension icons given as `"icon:Name"` (see `src/main.rs` around "An extension icon value")
   currently map names to Fluent glyphs: map them to Lucide stems instead, falling back to
   `"command"`.
6. Keep visual output otherwise unchanged in this task (tokens and components come next).
   Add one unit test that every `g::` constant names a file the AssetSource serves.

## Task 2: Tokens and themes

Files: `src/design.rs`, `src/config.rs`, `src/settings.rs` (only where theme names are listed),
`src/main.rs` (only where `Tokens`/theme APIs that change are called).

1. Rewrite `Tokens::dark()` and `Tokens::light()` with every colour token from
   `docs/design/system/tokens.json` `color.tokens`, exact hex (8-digit = with alpha). Add the new
   fields: `bg_solid`, `edge`, `edge_dim`, `sel_top`, `sel_bottom`, `sel_ring`, `danger_text`,
   `scrim_soft` (and keep `scrim`). `accent` = text colour; `accent_text` per tokens.json.
2. Rewrite `Tint` hues per tokens.json `tint_*` (dark, light). Add `Tint::Danger` if missing
   (= `danger`) and `Tint::Accent` (= `accent`) if missing; keep the existing kinds.
3. Remove named palettes: `DARK_THEMES`/`LIGHT_THEMES` become just Dark and Light (or are removed),
   `Tokens::named` goes. Config keys `[theme] dark` / `light` holding old names must still parse
   (ignored). Keep `[theme]` overrides but only for `background` (→ `bg`/`bg_solid`), `text`,
   `muted` (→ `text_muted`) and `selected` (→ `accent`, with `accent_text` recomputed); drop the
   rest from `from_theme`, leaving the config fields parseable.
4. Remove per-kind / per-app selection colour: delete `hue_for`, `selected(hue)`, `image_hue` and
   every caller's hue plumbing (callers will be fixed properly in Tasks 3–4; in this task pass
   whatever keeps it compiling, e.g. the selection fns ignore hue).
5. Glass decision: add `pub glass: bool` to `Tokens`. It is false when Windows transparency effects
   are off (`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\EnableTransparency`
   = 0), when high contrast is on (`SystemParametersInfoW(SPI_GETHIGHCONTRAST)`), or on battery
   saver (`GetSystemPowerStatus` `SystemStatusFlag == 1`); else true. Add `Tokens::body()` =
   `bg` when glass else `bg_solid`. Read the system state once per theme build, not per frame.
6. Shadows as functions returning `Vec<gpui::BoxShadow>` from tokens.json `shadow`: `ring(t)`,
   `ring_color(c)` (2 px spread, no blur, colour at 32%), `float(t)`, `rim(t)` (inset), `depth(t)`.
   If gpui-ce `BoxShadow` lacks inset support, draw `rim` as 1 px inner lines instead and say so.
7. Contrast tests: replace the old seven-theme tests with Dark/Light checks of the pairs in spec
   README "Contrast": text, text_muted, text_faint on `bg_solid` and on `bg` composited over the
   worst backdrop (Dark: `bg` over white levelled to the darkest backdrop the alpha allows — use
   composite over `#808080` as the stand-in worst case and document it; Light: over `#404040`),
   text_muted on the selected fill (sel_top over bg_solid), accent_text on accent, danger_text on
   danger, warning on bg_solid, each tint at 3:1 on its 14% wash over bg_solid, and `text` (icon in
   a selected tile) at 3:1 on the tile wash over the selected fill. If a spec value fails a test,
   keep the spec value, mark that assertion `#[ignore]` with a comment naming the pair and the
   measured ratio, and report it.

## Task 3: Components in design.rs

File: `src/design.rs` (plus call-site signature fixes elsewhere only as needed to compile).
Read each `docs/design/system/components/<Name>/README.md` and its rules in `bundle.css` before
writing it. Match the spec's anatomy, sizes, tokens and states exactly.

- `selectable(height, on, t)`: r-md, hover fill `hover` when off; on = neutral lozenge per Global
  Constraints (vertical `linear_gradient` sel_top→sel_bottom, 1 px `sel_ring` border, a 1 px top
  highlight in `edge_dim` or an inset shadow, accent pill 3 × 16 centred on the left edge).
  No hue parameter.
- `icon_tile(icon, t, selected, tint)`: 28 box, r-md. Tinted: icon (ic-lg) in the hue on a 14% wash
  of the hue with a 1 px inner rim at 24% of the hue; selected keeps the wash and the icon steps up
  to `text`. Untinted: `surface` fill, `hairline` rim, icon `text_muted`; selected `surface_strong`
  and `text`. App image: 24 px inside the 28 box, no wash or rim. Emoji: bare, 18 px.
- `list_row(icon, title, subtitle, accessory, on, t)`: 40 slot, 8 inset, 36 selectable, 12 gap,
  title text up to 70% width, subtitle body_sm `text_muted` truncating first, accessory
  `text_faint` (`text_muted` when on).
- `key_cap(label, t)` / `key_caps(spec, t)` / `shortcut(label, keys, t)`: 20 tall, r-xs, `surface`
  fill, 1 px `border`, caption 11 weight 500 `text_muted`; Enter/Up/Down draw `return`/`up`/`down`
  icons via `icon_cap`. Gap 4.
- `section_header(title, count, t)`: full row slot, small 12 weight 600 `text_faint` label + rule
  in `hairline`, inset 16.
- `empty_state(icon, title, hint, t)`: icon ic-xl (1.25 stroke) `text_faint`, title weight 600,
  hint body_sm `text_muted`.
- `field(focused, t)`: 32 tall, r-sm, `surface`, 1 px `border`; focused = 1 px `accent` border +
  `ring`.
- `button(label, kind, focused, t)`: 32 tall, r-sm, body_sm, padding 12. Primary: `accent` fill,
  `accent_text` label. Quiet: `surface` + `border`, `text`. Danger: `danger` fill, `danger_text`.
  Focus ring: accent ring (Primary, Quiet), danger-coloured ring (Danger).
- `toggle(on, t)`: 36 × 20, off `surface_strong` track + `text_muted` knob left; on `accent` track +
  `accent_text` knob right; 2 px inset.
- `segmented(options, current, t)` (add if missing): `surface` track 32, r-sm, 2 px padding,
  segments 28, r-xs; current = `surface_strong`, weight 600, a 6 px `accent` dot before its label.
- `hotkey_control(...)` per HotkeyControl README if a hotkey recorder exists in code (Settings).
- Glass surfaces:
  - `glass_window(t)`: the launcher body. Fill `t.body()`; when `t.glass`, a sheen (diagonal
    `linear_gradient` from `edge_dim` at the top-left to transparent by 42%), a 1 px rim border
    (gradient `edge` top-left → `edge_dim` → `border` bottom-right if gpui-ce supports gradient
    borders; else solid `border` plus a 1 px `edge` top line, and say which), `rim` thickness and
    `depth` shadow; radius r-xl. Without glass: `bg_solid` with the rim only.
  - `elevated(t)`: ActionPanel / Dialog surface: `bg_elevated`, r-lg, rim + `float` shadow, no blur.
  - `scrim(color)`: an absolute full-size overlay.
- Delete helpers that no longer have a use (`fluent`, `on_color` if unused, `pill(hue, …)` hue
  param, …).

## Task 4: The launcher window

Files: `src/main.rs`, `src/win.rs`, `src/files.rs` and other view code using the components.
Use the Task 3 components everywhere; no one-off styling.

- Window: body = `glass_window`. With `t.glass`, keep `WindowsWindowBackground::Blurred` and paint
  `bg` (alpha) so the OS blur shows through; without glass paint `bg_solid`. Corners: aim for
  r-xl 16. Try `CreateRoundRectRgn` + `SetWindowRgn` (sized with the scaled window, updated on
  resize); if that breaks the acrylic or clips badly, keep the DWM round corner preference and
  report it. Height formula = 2 + bar + row × (rows shown + 1) + 2 × list_pad + footer.
- SearchBar (56): root = `search` icon ic-lg in `accent`; other views = a Back tile (`surface`,
  r-sm, `back` icon); query at ts query 22 in `text`, placeholder `text_faint`; caret `accent`,
  500 ms blink; `hairline` beneath; right side per SearchBar README (filter dropdown, or "Recent"
  + Down cap in compact root with nothing typed, if that feature exists).
- List: `list_row`/`section_header` per their READMEs; one neutral selection; no hue anywhere.
- CalculatorCard: two row slots tall, r-lg, `surface` at rest, neutral lozenge + pill when
  selected; question heading 16 weight 400 `text_muted`, `arrow`/`forward` icon, answer display 20
  weight 600; only the calculator icon carries `tint_calc`.
- ActionBar (40): `hairline` above, body_sm. Left: selected row's icon (ic-md) in its hue and its
  kind; or a status: 8 px dot (`accent` info, `success`, `danger`) + message in `text`.
  Right: primary action + Enter cap, `vdivider`, "Actions" Ctrl K. Panel open: "Run" ↵, "Close" Esc.
  Armed: "Press ↵ again to confirm" in `warning`.
- ActionPanel: `elevated`, 320 wide, ≤ 320 tall, 8 px above the ActionBar on the right; window
  behind dimmed with `scrim_soft`; header = IconTile + title weight 600 over `hairline`; rows are
  `selectable` at control height with icon ic-md, label, and a KeyCap on every row that has a
  shortcut; destructive rows icon + label in `danger`.
- Dialog: `scrim` over the window; `elevated` 320 wide, padding 16; icon ic-xl `warning` icon in
  `danger` (destructive) or `info` in `accent`; title, message body_sm; buttons stacked full width,
  primary first and focused, then Quiet "Cancel".
- EmptyState / loading per EmptyState README (clock icon, "Loading…", no spinner).
- File rows pick icons by type (picture, video, audio, archive, code, document, folder) —
  `src/files.rs`.

## Task 5: The Settings window

File: `src/settings.rs` (and `src/main.rs` only where settings open/close).

- Opaque window: `bg_solid` (no glass), same tokens and fonts.
- 192 px sidebar of `selectable` rows at control height (icon ic-md + label), neutral selection.
- Pages: ts-small weight-600 `text_faint` section labels over `surface` cards, r-lg, 24 between
  cards; a row = label column (title `text`, optional ts-small description `text_muted`) and a
  control column.
- Controls only: Toggle (on/off), SegmentedControl (≤5 options), dropdown (one of many), Field
  (text), HotkeyControl (keys). Replace any other control style.
- Theme page: mode Segmented (System / Dark / Light); remove the named-palette pickers and any
  override swatches no longer backed by `from_theme`; keep a live preview if one exists, built from
  the real components.
- Footer status line with dot, per spec (applies at once, no Save button) where a footer exists.

## Task 6: Design docs

Files: `docs/design/DESIGN.md` only.

Rewrite it short: the design system lives in `docs/design/system/` (copied from the published
Design System); `src/design.rs` implements it; list what the code could not match exactly (collect
from the task reports listed in the dispatch) and why. Remove the old selection-wears-the-colour
text and the named-theme table.
