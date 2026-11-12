# procast design critique and requirements (second pass)

Reviewed on 2026-11-12 from fresh screenshots of the current build (`ai` branch, dark theme,
compact window mode, `width = 0.49` on a 1920 px screen, so the launcher is drawn at about 1.23×).
Screens: root (recent), root search, calculator, fallback rows, action panel, emoji, files,
Quit Applications, store loading, and the General, Launcher, Shortcuts, Theme, Extensions and
Files settings pages. Colours and contrast below were sampled from the screenshots, not taken
from the token table.

The first pass (2026-11-04) is in git history. Its sixteen requirements are done except R13
(root hint) and the tail of R5 (type scale); both are carried into this list.

## Ratings

| Area | First pass | Now | In one line |
|---|---|---|---|
| **Usability** | 6 | **7 / 10** | Ranking and whole rows are fixed. What's left: repeated labels, a section header that costs a whole row, a bare compact root, settings controls that run off the card |
| **Interface** | 7 | **6 / 10** | Tidy but anonymous. Grey selection, a 17 px search field, and a body colour that changes with the wallpaper. Nothing on screen says "procast" |
| **UX** | 6 | **6.5 / 10** | Honest tones and the calculator card landed. The footer repeats the row, hotkeys are captured three different ways, and the theme preview is collapsed to a line |
| **Consistency** | 5 | **5 / 10** | The tokens exist, but views bypass them: 6 radii, 11 icon sizes, 9 gaps, 3 input styles, 4 selected styles, and a design doc that is out of step again |
| **Accessibility** | 5 | **6 / 10** | Body text passes. Accessory text on the selected row is 3.3:1, white on the accent is 3.9:1, and focus is still only a border colour |

Score drop on Interface is deliberate: the first pass rated it against "broken"; this pass rates
it against "would I recognise it in a screenshot". It would not be recognised.

## Design critique: procast launcher and settings

### Overall impression
It is clean, and it is forgettable. The structure is right (search, list, action bar) and nothing
is broken, but every choice is the default one: neutral grey selection, small search text, a flat
list of same-weight rows. The biggest opportunity is to give the window one confident idea and
then make every surface obey the same small set of tokens.

### Usability

| Finding | Severity | Recommendation |
|---|---|---|
| The body colour is whatever is behind the window. Sampled: `#271D36` over one window, `#2A292A` over another, `#1C1B1E` on the second monitor. The popover (`#262529`, opaque) then doesn't match the body it floats over | 🔴 Critical | Make the body near-opaque (about 96%) so acrylic is a hint, not the colour. Popovers and cards derive from the same base hue |
| Accessory text on the selected row is `#8B8B97` on `#41384D`: **3.3:1** | 🔴 Critical | On the selected row, accessories step up to `text_muted`; check contrast against the selected fill, not the body |
| The right column says `Command`, `Application`, `File`, `Web` or `Smileys` on every row, and the action bar repeats it beside a search icon | 🟡 Moderate | Accessory shows only what tells rows apart (alias, hotkey, folder, count). The action bar's left side shows the selected item's icon and name |
| A section header takes a full 40 px row slot, with the label sitting at the bottom. Two headers cost two of ten slots | 🟡 Moderate | Keep the slot (the list is virtualised) but drop the header when a list has only one section, and set the label beside a rule so the slot reads as a divider |
| The action panel's header is a line of faint text, and its grey doesn't match the body it floats over | 🟡 Moderate | Header shows the item's icon and title; the panel's colour is derived from the body's |
| Compact root with nothing typed is a bare bar (R13 from the first pass) | 🟡 Moderate | A faint right-aligned hint in the bar: `↓` recent, `Ctrl K` actions |
| Settings: the label column is a fixed 300 px, text inputs are a fixed 440 px, so on the Theme page the inputs run off the right edge of the card | 🟡 Moderate | The control column fills what's left; inputs take `max_w_full` of it |
| Settings › Theme: the live preview is collapsed to a 2 px line (a flex child with `overflow_hidden` and no `flex_none` shrinks to nothing) | 🟡 Moderate | `flex_none` on the preview; build it from the same row and tile components as the launcher so it is a real preview |
| Hotkey rows use three control styles: key caps plus "Click to change" text, an outlined "Record Hotkey" pill (13 px on General, 12 px on Shortcuts), and an accent-filled "Press keys…" pill | 🟡 Moderate | One hotkey control: a bordered field that holds caps or the word "Record", with an accent ring while recording |
| The settings footer permanently shows the config file path | 🟢 Minor | Show it once on the Advanced page; the footer shows key hints and save status |
| Files rows all carry the same document glyph | 🟢 Minor | Glyph by type (folder, image, code, archive, document) |

### Visual hierarchy
- **What draws the eye first:** the selected row. Correct, but it is a grey bar; the accent is a 3 px sliver.
- **Reading flow:** search text → selection → action bar. The search text is the user's own words and the top of the hierarchy, yet it is 17 px regular, barely larger than a row title.
- **Emphasis:** tinted glyph tiles (26 px, coloured wash) are louder than app icons (20 px, bare), so built-in commands outweigh the apps people actually launch. The two icon kinds don't share a size or a baseline box.
- **Whitespace:** 24 px of dead space above each section label; none between the last row and the action bar.

### Consistency

Counted from `src/main.rs` and `src/settings.rs`:

| Element | Issue | Recommendation |
|---|---|---|
| Radius | 4, 5, 6, 8, 10, 12 plus rem-based `rounded_md` in 25 places. DESIGN.md names five radius tokens; none exist in code | Four tokens in `design::r`: `XS` 4 (caps), `SM` 6 (tiles, inputs, buttons), `MD` 8 (rows, chips groups), `LG` 12 (cards, popovers, dialogs) |
| Type | Literals 12, 12.5, 13, 14, 16 and 20 in `main.rs` bypass `design::ts` (markdown H1/H2, panel header, notices, form labels) | Everything through `ts`; add one step (`HEADING` 16) for markdown H2 and card titles; H1 uses `DISPLAY` |
| Icon size | Glyphs drawn at 10, 11, 12, 13, 14, 15, 16, 17, 18, 26 and 44 | Four steps in `design::ic`: `SM` 12, `MD` 14, `LG` 16, `XL` 40 |
| Spacing | Gaps of 2, 4, 5, 6, 8, 10, 12, 14, 16; rem helpers (`px_2`, `p_3`, `gap_1`) mixed with `px()` | 4 px grid through `px()` only: 4, 8, 12, 16, 24 |
| Control height | 20, 32, 34, 36, and padding-derived (about 30) in settings | Two heights: `CONTROL` 32 (inputs, buttons, segmented, dropdown), cap 20 |
| Selected state | Launcher row: grey fill + 16 px pill. Settings row: grey fill + full-height bar. Sidebar: grey fill + pill. Segmented control and dialog button: solid accent fill. Favourites hover uses the *selected* fill | One recipe: accent-tinted fill + accent pill. Solid accent only on a primary button |
| Inputs | Launcher form: 34 px, `surface`, radius 6. Settings: `rounded_md` + `py_1`, placeholder in `text_muted` (launcher uses `text_faint`) | One `design::field` box used by both |
| Buttons | "Record Hotkey" (two sizes), "Open" pill, dialog buttons: three ad-hoc styles | One `design::button` with `primary` / `quiet` / `danger` |
| Glyphs | Settings chips use Unicode `×` and `←`, which DESIGN.md forbids | Fluent `Cancel` and `Up` |
| Danger | The dialog decides "destructive" by looking for "delete"/"remove" in the button label | Carry it explicitly, as tones already do |
| Raw colour | Dialog scrim is a literal `hsla(0,0,0,0.35)` in `main.rs` | `scrim` token |
| Footers | Launcher 40, settings 34, preview 36 | One `FOOTER` |
| Card fill | Settings cards use `bg_elevated` (the popover colour); calculator card uses `surface` | Cards use `surface`; `bg_elevated` is for things that float |
| Design doc | Says popover 320 (code 330), action row 32 (34), dialog 320 (340), form column 520 (540), label 120 (130), emoji 18 (17), empty glyph 48 (44) | Rewrite DESIGN.md from the code after this pass |

### Accessibility
- **Colour contrast (sampled):**
  - `text_faint` on the body: 4.3 to 5.1:1 depending on the wallpaper; passes only sometimes. A near-opaque body fixes the swing.
  - `text_faint` accessories on the selected row: **3.3:1**, fails.
  - White on accent `#7C6CFF` (segmented controls, "Press keys…"): **3.9:1** at 13 px, fails AA.
  - `text_muted` on the body: 5.6 to 6.6:1, passes.
- **Focus:** inputs change border colour only; no ring. The selected launcher row has no outline in high-contrast use.
- **Targets:** segmented options are about 26 px tall; chip buttons 22 px. Acceptable for mouse, small for touch.
- **Readability:** 14 px rows and 12 px descriptions at 17 px line height are fine.

### What works well
- The three-band structure and real key caps in the action bar.
- The calculator card: question, arrow, rounded answer, full value below.
- Kind tints as an idea: they make the list scannable without reading.
- Ranking is now right (`cl` → Clipboard History, Clock, Claude).
- Settings as grouped cards with wrapping descriptions.

## Requirements, in order

Ordered by what a user hits first and how much each one unblocks the ones after it.
Status after the 2026-11-12 pass: ✅ done · ◐ partly · ☐ not yet.

**P0: wrong on screen today.**
1. ✅ **D1 Stable body.** Every theme's body is opaque, so its colour is the theme's, not the wallpaper's (`#17171C` sampled on both monitors). The popover and dialog colours sit one step above it in the same hue.
2. ✅ **D2 Contrast on selection.** Accessories step up to `text_muted` on a selected row. Text on the accent is white or ink, whichever reads better. A unit test holds all seven themes to 4.5:1; it failed four light and one dark theme on first run, and their text tokens were darkened until it passed.
3. ✅ **D3 Theme preview shows,** and is built from the launcher's own row, tile, header and key-cap components.
4. ✅ **D4 Settings controls fit.** The control column takes the remaining width and inputs shrink inside it.

**P1: the system. One of each.**
5. ✅ **D5 Tokens in code.** `design::r` (radius), `design::ic` (glyph size), `CONTROL`, `CAP` and `TILE` beside `ts`. Views contain no literal radius, text size or glyph size (checked by grep). Spacing stays as literals on a 4 px grid.
6. ✅ **D6 One selected recipe.** A wash of the hue plus the pill, for launcher rows, action-panel rows, settings rows and the sidebar. Hover is lighter. Solid accent only on the primary button.
7. ✅ **D7 One field, one button, one segmented control.** `design::field` and `design::button` are shared by the launcher form, the dialog and Settings; hotkey capture is a field.
8. ✅ **D8 Icons share a box.** One 28 px box: glyph tiles fill it, images draw at 24.
9. ✅ **D9 Fluent glyphs only.** Settings chips use Fluent `Cancel` and `Back`.
10. ✅ **D10 Explicit danger.** The extension runtime already sent `destructive`; the dialog now uses it instead of reading the label.

**P2: identity and information.**
11. ✅ **D11 A signature.** See "Direction".
12. ✅ **D12 The query is the hero.** 22 px Display; nothing else in the window is larger.
13. ✅ **D13 Accessories earn their place.** Root rows show their hotkey or alias; other lists say an accessory once per run. The action bar shows the selected row's icon and kind.
14. ✅ **D14 Section headers read as dividers** (label and rule), and a single-section list, or a leading "Results", has none.
15. ✅ **D15 Action panel.** Header with the item's icon and title; rows select in the item's colour.
16. ✅ **D16 Root hint** (was R13). Compact root shows `Recent ↓` in the bar.
17. ✅ **D17 Settings chrome.** Footer shows key hints as caps, or the save status with a tone dot; the config path is on the Advanced page.
18. ✅ **D18 File glyphs by type.**

**P3: record it.**
19. ✅ **D19 DESIGN.md rewritten from the code**, with fresh screenshots in `docs/design/after/`.

## Ratings after this pass

| Area | Before | After | Why not higher |
|---|---|---|---|
| **Usability** | 7 | **8** | The emoji picker is still a list, not a grid; a list without headers leaves one empty slot at the bottom of the window |
| **Interface** | 6 | **8** | Extension views (forms, detail, store) inherit the tokens but were not redrawn with the same care |
| **UX** | 6.5 | **7.5** | Only two actions have shortcuts; no preview pane for clipboard or files |
| **Consistency** | 5 | **8.5** | Spacing is a convention, not a token; the segmented control lives in `settings.rs` because nothing else uses it yet |
| **Accessibility** | 6 | **8** | Contrast is now tested. Hit targets are unchanged (22 to 28 px); no high-contrast theme |

## Direction

**The selection wears the colour of the thing.** A selected row lights up in the hue of what it
is: its kind tint, or for an app, the dominant colour of its own icon. Destructive rows glow
red before you press Enter. The action panel inherits its item's colour. Everything else was
made quieter so this can carry the identity: an opaque neutral body, the accent kept for the
caret, focus and the one primary button, no repeated labels.

Why this and not a new palette: the kind tints were already the one idea procast had that no
other launcher does. The first pass confined them to a 26 px tile. This pass lets them own the
selection, which is where the eye already is.

What was considered and dropped: a key-cap-shaped selected row (skeuomorphic, and it fights
the Windows 11 materials), a display typeface other than Segoe (the product's promise is that
it feels like part of Windows; the three optical sizes of Segoe UI Variable give the range
instead), and taking the accent from the Windows system colour (a feature, not a design fix).

## Not checked in this pass

- Extension forms, detail views, confirm dialogs, the store, clipboard history and Ask AI were
  changed through the shared components but not screenshotted.
- `spikes/e2e.py` was not run (it needs the release build and drives the keyboard).
- Screenshots were taken through a warm display filter, so their light-theme colours read
  slightly beige; the contrast figures come from the token test, not from the images.
