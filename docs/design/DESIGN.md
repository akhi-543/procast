# akscast design system

akscast is a Windows-native launcher. It matches the *structure, density and polish* of the best launchers (Raycast was studied from screenshots of the installed v2.10.1 in `docs/design/reference/`, kept out of git), but has its own identity:

- **Windows 11 materials:** acrylic backdrop, OS-rounded corners, Segoe UI Variable, Segoe Fluent Icons.
- **The Fluent selection pill:** an accent bar on the selected row.
- **akscast's own accent colour.**

No third-party artwork, icons, logo or palette is copied.

---

## 1. Audit of akscast before this redesign (screenshots in `docs/design/before/`)

| Issue | Where | Fix |
|---|---|---|
| Saturated blue block selection that **doesn't span the row** (it shrinks to its content) | every list | Full-width neutral selection fill and an accent **pill** on the left |
| Icon glyphs render as **tofu boxes** (`✎ ↵ ▾ ▸`) | snippets, footer, form, panel | All UI icons come from **Segoe Fluent Icons** (66 codepoints verified to exist); emoji only for emoji |
| No section headers | root, extension lists | `Results`, `Recent`, `Suggestions`, `Use "…" with`, extension sections with counts |
| The kind label sits inline as a subtitle; there's no right-hand accessory column | rows | Title, then muted **subtitle** inline, then **accessory** right-aligned (`Application`, `Command`…) |
| Bright 1 px white dividers | search bar, footer | **Hairline** = text at 8% opacity |
| Footer is tiny grey text; no key caps | footer | **Action bar**: context icon + title on the left; primary action + `↵` cap, divider, `Actions Ctrl K` on the right |
| Ctrl+K replaces the list with a single row | action panel | **Floating popover** bottom-right over the list, with shortcut caps |
| Square, opaque window with a grey border | window | DWM rounded corners, acrylic backdrop, hairline border |
| Mode is a chip at the far right; no way back with the mouse | search bar | **Back button** tile on the left in sub-views; akscast mark at root |
| Form: labels in a left column, everything stretched | forms | Centred 520 px column, right-aligned labels, 34 px rounded inputs, chevron dropdowns |
| No empty states | file search, empty lists | Centred icon, title and hint |
| Text is too big and rows too tall (16 px / 44 px) | everywhere | 14 px text, 40 px rows; a denser and calmer layout |

---

## 2. Tokens

### Colour (dark is the primary theme)

| Token | Dark | Light | Use |
|---|---|---|---|
| `bg` | `#1B1B1F` @ 86% over acrylic | `#F7F7F9` @ 88% | window body |
| `bg_elevated` | `#26262C` | `#FFFFFF` | popovers, dialogs |
| `surface` | `#FFFFFF` @ 6% | `#000000` @ 5% | inputs, key caps, unselected icon tiles, cards |
| `surface_strong` | `#FFFFFF` @ 12% | `#000000` @ 9% | **the selected state everywhere** (rows, settings rows, sidebar, action panel) |
| `hover` | `#FFFFFF` @ 4% | `#000000` @ 3% | under the mouse; always lighter than selection |
| `text` | `#F2F2F5` | `#1B1B1F` | primary text |
| `text_muted` | `#A0A0AB` | `#5F5F6B` | subtitles, labels, accessories |
| `text_faint` | `#8B8B97` | `#6B6B77` | section headers, placeholders, hints, accessories (at least 4.5:1 on the body in every theme) |
| `hairline` | `#FFFFFF` @ 8% | `#000000` @ 8% | dividers |
| `border` | `#FFFFFF` @ 12% | `#000000` @ 12% | window, inputs, popovers |
| `accent` | `#7C6CFF` (akscast violet) | `#5B4BF0` | selection pill, focus ring, active toggle, root mark |
| `accent_text` | `#FFFFFF` | `#FFFFFF` | text on accent |
| `success` / `warning` / `danger` | `#4CC38A` / `#F5B74F` / `#FF6B6B` | `#1F9D61` / `#C98100` / `#D93D3D` | toasts, tags, errors |

### Kind tints

The one distinctive element: a row's icon tile takes the hue of what the row *is*, as a wash
(18% alpha, 30% when selected) with the glyph in the full hue. Images and emoji draw untinted.
Everything else in the window stays neutral, and the accent is kept for selection and focus.

| Tint | Dark | Light | Rows |
|---|---|---|---|
| `Accent` | accent | accent | akscast's own commands (Settings, Store, Reload) |
| `System` | `#F2B65A` | `#B27100` | lock, sleep, restart, shut down, dark mode |
| `Window` | `#4FC4CF` | `#0F8A96` | window layouts |
| `Paste` | `#6AA8FF` | `#2F6FD6` | clipboard, emoji, snippets |
| `Web` | `#6FD19C` | `#1F9D61` | quicklinks, web search fallbacks |
| `File` | `#D9B77E` | `#9A6B1F` | file search |
| `Calc` | `#F08CC0` | `#C23D84` | the calculator |
| `Danger` | danger | danger | clearing history, quitting apps |

`[theme]` overrides map like this:
- `background` → `bg`
- `text` → `text`
- `muted` → `text_muted`
- `selected` → `accent`
- `selected_text` → `accent_text`
- `border` → `border`
- `separator` → `hairline`
- `container` → `bg_elevated`

### Typography

Fonts: Segoe UI Variable Text (UI), Segoe UI Variable Display (search field, headings, the
calculator answer), Cascadia Mono (code), Segoe Fluent Icons (every UI icon: 15 in tiles, 14 in
the action bar, 44 in empty states).

Six text sizes, in `design::ts`. Views use nothing else.

| Step | Size | Use |
|---|---|---|
| `CAPTION` | 11 | key caps |
| `SMALL` | 12 | section headers (semibold), settings descriptions, footers |
| `BODY_SM` | 13 | subtitles, accessories, the action bar |
| `BODY` | 14 | rows, body text |
| `TITLE` | 17 | the search field |
| `DISPLAY` | 22 | page headings, the calculator answer (semibold) |

### Spacing, size, radius

- **Spacing (4 px base):** `s1` 4, `s2` 8, `s3` 12, `s4` 16, `s5` 20, `s6` 24.
- **Sizes:**
  - search bar 56
  - row 40 (section-header rows share the 40 px slot for virtualisation and sit on the bottom edge)
  - list padding 4 above and below
  - action bar 40
  - window height = 2 (border) + 56 + 40 × (rows shown + 1 header) + 8 + 40, so the list always holds whole rows
  - icon tile 26 with a 15 px glyph
  - selection pill 3×16
  - calculator card two rows tall
  - window 750 wide before scaling (`width` ≤ 1 is a share of the screen)
- All of these live in `design.rs` only; views import them.
- **Radius:** window 8 (set by the OS); `r_row` 8, `r_tile` 6, `r_input` 6, `r_popover` 10, `r_cap` 4.
- **Elevation:** popovers use `bg_elevated`, a `border` and a large soft shadow; nothing else is shadowed.

---

## 3. Components

| Component | Anatomy | States / rules |
|---|---|---|
| **Window** | Acrylic body, hairline border, rounded corners, three bands: search bar / content / action bar | Light, dark or system; everything comes from tokens |
| **Search bar** | `[mark or back tile] [input, display 17] [accessory: dropdown]` | Root shows the accent mark (Search icon). Sub-views show a Back tile (clickable = Esc). The placeholder is `text_faint` |
| **Section header** | `Title  count`, `text_sm` in `text_faint`, sitting on the bottom of its slot | Not selectable; the cursor skips it |
| **List row** | `[pill][icon tile] Title  subtitle ······ accessory` | **Default:** transparent. **Hover:** `hover`. **Selected:** `surface_strong` plus the 3×16 accent pill on the left edge. Subtitles name the group ("Window", "Clipboard"), never "akscast" on every row. Accessories are `text_faint` |
| **Icon tile** | 26 px rounded `surface_strong` square holding a Fluent glyph; app/extension images draw at 20 px with no tile | Emoji draw at 18 px with no tile |
| **Accessory** | Right-aligned `text_md` in `text_muted`; tags are tinted pills | |
| **Key cap** | `CAPTION` on `surface`, `border`, `r_cap`, 20 px tall, min width 20 | `↵` uses the Fluent ReturnKey glyph |
| **Action bar** | Left: context icon (14) + where you are (in root search: the selected row's kind), or a message with a dot: accent = info, success = done, danger = error. Right: `Primary ↵ │ Actions Ctrl K` | Messages carry their tone explicitly; colour is never guessed from the words |
| **Calculator card** | Two rows tall above the list: tinted calculator glyph, the question (TITLE, muted), a forward arrow, the answer (DISPLAY semibold, at most four decimals) and the full value below it | Selected like a row (fill plus pill); Enter copies the full value |
| **Settings** | Sidebar of pages; each page is cards under headings; a row is label + wrapping description (300 px) and its control | Choices are segmented controls with short labels, clicked directly; lists are chips (× and ← are 22 px targets) with an Add field; the footer shows Saved/neutral messages muted and errors in danger |
| **Action panel** | A 320 px popover anchored above the action bar on the right. Header: item title. Rows: icon + label + caps (32 px) | Esc closes it; ↑↓ moves; Enter runs the action |
| **Detail pane** | 40/60 split with a hairline divider. Markdown: H1 20 semibold, body 14 at 1.5, code in mono on `surface`. Metadata: an `Information` header, then label/value rows with hairlines | |
| **Form** | Centred 520 px column; label (120, right-aligned, muted) then control | Inputs are 34 px with `surface` fill and a `border`; the focused field gets an `accent` ring. Dropdowns show value + chevron, checkboxes the Fluent Checkbox glyph |
| **Empty state** | Centred 48 px Fluent glyph (`text_faint`), title (`text`), hint (`text_muted`) | |
| **Dialog** (confirmAlert) | Centred `bg_elevated` card, 320 wide, primary + Cancel buttons | The primary button is accent; a destructive one is `danger` |
| **Toggle** (settings) | 36×20 track with a knob | Off: `surface_strong` track; on: `accent` track with a white knob |

## 4. Icon map (Segoe Fluent Icons)

| Use | Glyph | Codepoint |
|---|---|---|
| Search / root mark | Search | E721 |
| Back | Back | E72B |
| Dropdown | ChevronDown | E70D |
| Clipboard | Paste | E77F |
| Copy | Copy | E8C8 |
| Emoji | Emoji2 | E76E |
| Lock | Lock | E72E |
| Sleep | QuietHours | E708 |
| Restart / reload | Refresh | E72C |
| Shut down | PowerButton | E7E8 |
| Sign out | SignOut | F3B1 |
| Recycle bin | Delete | E74D |
| Dark mode | Personalize | E771 |
| Window left / right | DockLeft / DockRight | E90C / E90D |
| Window bottom | DockBottom | E90E |
| Maximize | FullScreen | E740 |
| Restore | BackToWindow | E73F |
| Next display | SwitchApps | E8F9 |
| Center / thirds | Tiles | ECA5 |
| Document | Document | E8A5 |
| Folder | Folder | E8B7 |
| Settings | Setting | E713 |
| Store | Shop | E719 |
| Extension | Puzzle | EA86 |
| Install | Download | E896 |
| Script / Run | CommandPrompt | E756 |
| Calculator | Calculator | E8EF |
| Web | Globe | E774 |
| Quicklink | Link | E71B |
| Snippet | Edit | E70F |
| Info | Info | E946 |
| Warning | Warning | E7BA |
| Check | CheckMark | E73E |
| Close | Cancel | E711 |
| Enter key | ReturnKey | E751 |
| App fallback | AppIconDefault | ECAA |
| History | History | E81C |
| Checkbox off / on | Checkbox / CheckboxComposite | E739 / E73A |
| Filter | Filter | E71C |

## 5. Do and don't

| ✅ Do | ❌ Don't |
|---|---|
| Use tokens from `src/design.rs` for every colour, size and radius | Hard-code hex values or pixel sizes in views |
| Use Fluent glyphs for UI and emoji only for emoji content | Use Unicode arrows or symbols (they render as tofu) |
| Show one primary action plus a key cap; put the rest in Ctrl+K | Put more than one action button in a row |
| Truncate the title last and the subtitle first | Wrap rows onto two lines |
| Use the accent for selection, focus and on-states only | Fill large areas with the accent |
