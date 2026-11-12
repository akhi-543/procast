# procast design system

procast is a Windows-native keyboard launcher. Everything below is in `src/design.rs`; views
(`main.rs`, `settings.rs`) take every colour, size, radius and component from there.
Screenshots of the current build are in `docs/design/after/`; the critique and the ordered
requirements that led here are in `critique-and-requirements.md`.

## 1. The idea

**The selection wears the colour of the thing.** A selected row lights up in the hue of what
it is: window commands teal, system actions amber, things you paste blue, the calculator pink,
anything destructive red. An app lights up in its own icon's dominant colour (Claude orange,
Windows Settings blue). The action panel opened on a row selects in that row's colour too.

That is the one loud thing. Everything around it stays quiet so it can be heard:

- The body is neutral ink (or neutral paper) and **opaque**, so it is the theme's colour on any wallpaper.
- The accent is for the caret, the root search mark, focus rings, on-states and the one primary button. It never fills a row.
- What you typed is the largest text in the window.
- Native Windows 11: Segoe UI Variable in its three optical sizes, Segoe Fluent Icons, OS-rounded corners.

No third-party artwork, icons, logo or palette is copied.

## 2. Tokens

### Colour

| Token | procast Dark | procast Light | Use |
|---|---|---|---|
| `bg` | `#17171C` | `#F7F7F9` | window body (opaque) |
| `bg_elevated` | `#222229` | `#FFFFFF` | things that float: action panel, dialog |
| `surface` | white 6% | black 5% | cards, inputs, key caps, untinted icon tiles |
| `surface_strong` | white 11% | black 9% | chips, the current segment, the off toggle |
| `hover` | white 4% | black 3% | under the mouse; always lighter than selected |
| `text` | `#F2F2F5` | `#1B1B1F` | primary text |
| `text_muted` | `#A0A0AB` | `#53535D` | subtitles, descriptions, text on a selected fill |
| `text_faint` | `#8B8B97` | `#60606B` | section labels, placeholders, accessories |
| `hairline` | white 8% | black 8% | dividers |
| `border` | white 12% | black 12% | window, inputs, popovers |
| `accent` | `#7C6CFF` | `#5B4BF0` | caret, focus ring, toggle on, primary button, pill where a row has no hue |
| `accent_text` | computed | computed | white or ink, whichever has more contrast on the accent |
| `success` / `warning` / `danger` | `#4CC38A` / `#F5B74F` / `#FF6B6B` | `#1F9D61` / `#946000` / `#D43838` | message dots, the armed line (`warning` as text), destructive button |
| `scrim` | black 40% | black 25% | behind a dialog |

Derived, not stored:

- **Selected fill** = `Tokens::selected(hue)`: the hue at 18% (dark) or 13% (light).
- **Row hue** = `Tokens::hue_for(tint, icon)`: the row's kind tint, else the icon image's dominant colour (`image_hue`, normalised to one saturation and lightness per theme), else the accent.

Named themes (Midnight, Graphite, Forest, Paper, Mist) replace `bg`, `bg_elevated`, the three
text colours and the accent. `[theme]` overrides map as before: `background` → `bg`,
`text` → `text`, `muted` → `text_muted`, `selected` → `accent`, `selected_text` → `accent_text`,
`border` → `border`, `separator` → `hairline`, `container` → `bg_elevated`. A `background`
given as `#RRGGBBAA` is the only way to get acrylic show-through.

**Contrast is tested.** `design::tests::every_theme_meets_aa_contrast` checks all seven themes
for at least 4.5:1 on: text on the accent, faint text on the body and on a card, and muted text
on a selected row in every kind hue and on a selected card row.
`status_colours_meet_aa_as_text` holds `warning` as text on the body and the label on a Danger
button to the same 4.5:1; `tile_glyphs_stand_out_from_their_wash` holds every tile glyph to
3:1 against its wash, at rest and selected.

### Kind tints

| Tint | Dark | Light | Rows |
|---|---|---|---|
| `Accent` | accent | accent | procast's own commands |
| `System` | `#F2B65A` | `#B27100` | lock, sleep, restart, shut down, dark mode |
| `Window` | `#4FC4CF` | `#0F8A96` | window layouts |
| `Paste` | `#6AA8FF` | `#2F6FD6` | clipboard, emoji, snippets |
| `Web` | `#6FD19C` | `#1A8A55` | quicklinks, web and AI fallbacks |
| `File` | `#D9B77E` | `#9A6B1F` | file search |
| `Calc` | `#F08CC0` | `#C23D84` | the calculator |
| `Danger` | danger | danger | clearing history, quitting apps |

A tinted glyph sits on a 14% wash of its hue. When its row is selected the wash deepens to
26% and the glyph steps up to `text`, as an untinted glyph does: a hue on a deeper wash of
itself falls under 3:1.

### Type

Segoe UI Variable **Display** (query, headings, the calculator answer), **Text** (rows, body),
**Small** (12 px and under: key caps, section labels, fine print). Cascadia Mono for code.

| `ts` step | Size | Use |
|---|---|---|
| `CAPTION` | 11 | key caps |
| `SMALL` | 12 | section labels, descriptions, field errors |
| `BODY_SM` | 13 | subtitles, accessories, action bar, buttons, segments |
| `BODY` | 14 | rows, body text |
| `HEADING` | 16 | second-level headings, the calculator's question |
| `DISPLAY` | 20 | page headings, the calculator answer (semibold) |
| `QUERY` | 22 | the search field |

### Size, radius, icons

| Token | Value | Use |
|---|---|---|
| `BAR` | 56 | search bar |
| `ROW` | 40 | a list slot; the row's box inside it is 36 |
| `FOOTER` | 40 | action bar, in the launcher, Settings and the theme preview |
| `TILE` | 28 | the icon box: glyph tiles fill it, images draw at 24 inside it |
| `CONTROL` | 32 | inputs, buttons, dropdowns, action-panel rows, sidebar rows; segments are 28 inside a 32 track |
| `CAP` | 20 | key caps, the toggle's height, colour swatches |
| `LIST_PAD` | 4 | above and below the list |
| `r::XS` / `SM` / `MD` / `LG` | 4 / 6 / 8 / 12 | caps and segments / tiles, inputs, buttons, chips / rows / cards, popovers, dialogs |
| `ic::SM` / `MD` / `LG` / `XL` | 12 / 14 / 16 / 40 | inside caps and chips / action bar and panel rows / tiles and the search mark / empty states and dialogs |

Spacing is on a 4 px grid (4, 8, 12, 16, 24); 2 px appears only inside controls (the segmented
track, label-to-description). Window height = 2 (border) + `BAR` + `ROW` × (rows shown + 1) +
2 × `LIST_PAD` + `FOOTER`. Window width 750 before scaling.

## 3. Components

All in `design.rs`. If a view needs something that looks like one of these, it uses it.

| Component | What it is | Rules |
|---|---|---|
| `selectable(height, on, hue)` | The selectable box: radius `MD`, hover, and when on, the hue wash plus the 3×16 pill | The one selected recipe. Used by launcher rows, action-panel rows and the Settings sidebar. Settings card rows use the same wash and pill in the accent |
| `list_row(icon, title, subtitle, accessory, on, hue)` | A launcher row in its `ROW` slot | Title keeps priority; the subtitle truncates first. Also what the theme preview is built from |
| `icon_tile(icon, selected, tint)` | The `TILE` box | Glyphs on a wash, images bare, emoji bare |
| `section_header(title, count)` | A label and a rule in a row-height slot | Sentence case. No header when the list has one section, and none for "Results" directly under the search field |
| `key_cap` / `key_caps` / `shortcut` | Caps; a hotkey as caps; a label followed by caps | `↵ ↑ ↓` draw Fluent glyphs |
| `field(focused)` | The one input box | Focus = accent border plus a 2 px ring. Launcher forms, Settings text, hotkey capture |
| `button(label, kind, focused)` | `Primary` (solid accent), `Quiet`, `Danger` | At most one `Primary` on a screen. A dialog is destructive only when its caller says so |
| `toggle(on)` | 36×20 switch | On: accent track, `accent_text` knob |
| `empty_state(glyph, title, hint)` | Centred glyph, title, hint | The hint says what to do next |
| `fluent(glyph, size, colour)` | A Segoe Fluent Icons glyph | The only way to draw a UI icon. No Unicode symbols |

In the views:

| Part | Anatomy |
|---|---|
| **Search bar** | Mark (root) or Back tile, the query at `QUERY`, then a dropdown or, in compact root with nothing typed, `Recent ↓` |
| **Row accessory** | Root search: the row's hotkey as caps, else its alias, else nothing. Other lists: the row's accessory, said once per run of rows that share it. Steps up to `text_muted` on the selected row |
| **Action bar** | Left: a tone dot and message, or where you are. In root search that is the selected row's own icon and its kind (the one place the kind is stated). Right: primary action and `↵`, then `Actions Ctrl K` |
| **Calculator card** | Two rows tall, radius `LG`; selects in the calculator's pink like any row |
| **Action panel** | 320 wide, radius `LG`, `bg_elevated`. Header: the item's icon and title. Rows: `selectable` at `CONTROL` height in the item's hue |
| **Dialog** | 320 wide over the scrim; a 40 px glyph, title, message, then `button`s stacked |
| **Settings** | 192 px sidebar of `selectable` rows; pages are cards (`surface`, radius `LG`) under small labels; a row is a 280 px label column and a control column that takes the rest. Footer: key hints as caps, or the last save's outcome with a tone dot |
| **Segmented control** | A `surface` track; the current option is a raised `surface_strong` segment in semibold with an accent dot |
| **Hotkey control** | A `field` holding the caps, or a keyboard glyph and "Record"; while recording it has the focus ring |

## 4. Icon map (Segoe Fluent Icons)

Codepoints are in `design::g`; each was checked to exist in the font. Added in this pass:
`DOWN` E74B, `PICTURE` E8B9, `VIDEO` E714, `AUDIO` E8D6, `ZIP` F012, `PDF` EA90. File rows pick
their glyph by extension (image, video, audio, archive, PDF, code, program, document, folder).

## 5. Do and don't

| Do | Don't |
|---|---|
| Take every colour, text size, radius and glyph size from `design.rs` | Write a literal `rounded(px(…))`, `text_size(px(…))` or glyph size in a view |
| Let the selected row carry the colour | Fill anything else with a hue or the accent |
| Use `field`, `button`, `selectable` | Style a one-off input, pill or highlight |
| Say a row's kind once, in the action bar | Repeat "Command" or "Application" down a column |
| Add a theme, then run `cargo test design` | Ship a palette that hasn't passed the contrast test |
