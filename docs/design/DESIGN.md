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
| `surface` | `#FFFFFF` @ 6% | `#000000` @ 5% | selected/hovered row, inputs, key caps |
| `surface_strong` | `#FFFFFF` @ 10% | `#000000` @ 8% | pressed, icon tiles |
| `text` | `#F2F2F5` | `#1B1B1F` | primary text |
| `text_muted` | `#A0A0AB` | `#5F5F6B` | subtitles, labels, accessories |
| `text_faint` | `#6E6E7A` | `#8E8E9A` | section headers, placeholders, hints |
| `hairline` | `#FFFFFF` @ 8% | `#000000` @ 8% | dividers |
| `border` | `#FFFFFF` @ 12% | `#000000` @ 12% | window, inputs, popovers |
| `accent` | `#7C6CFF` (akscast violet) | `#5B4BF0` | selection pill, focus ring, active toggle, root mark |
| `accent_text` | `#FFFFFF` | `#FFFFFF` | text on accent |
| `success` / `warning` / `danger` | `#4CC38A` / `#F5B74F` / `#FF6B6B` | `#1F9D61` / `#C98100` / `#D93D3D` | toasts, tags, errors |

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

| Token | Font | Size / weight | Use |
|---|---|---|---|
| `font_ui` | Segoe UI Variable Text | 14 / regular | rows, body |
| `font_display` | Segoe UI Variable Display | 17 regular (search), 20 semibold (H1) | search field, headings |
| `font_mono` | Cascadia Mono | 13 | code, detail code blocks |
| `font_icon` | Segoe Fluent Icons | 16 (row), 14 (footer), 48 (empty state) | all UI icons |
| `text_xs` | | 11 / semibold | key caps |
| `text_sm` | | 12.5 / semibold | section headers |
| `text_md` | | 13 | subtitles, accessories, footer |

### Spacing, size, radius

- **Spacing (4 px base):** `s1` 4, `s2` 8, `s3` 12, `s4` 16, `s5` 20, `s6` 24.
- **Sizes:**
  - search bar 56
  - row 40 (section-header rows share the 40 px slot for virtualisation and sit on the bottom edge)
  - action bar 40
  - icon tile 26 with a 16 px glyph
  - selection pill 3×16
  - window 760 wide
- **Radius:** window 8 (set by the OS); `r_row` 8, `r_tile` 6, `r_input` 6, `r_popover` 10, `r_cap` 4.
- **Elevation:** popovers use `bg_elevated`, a `border` and a large soft shadow; nothing else is shadowed.

---

## 3. Components

| Component | Anatomy | States / rules |
|---|---|---|
| **Window** | Acrylic body, hairline border, rounded corners, three bands: search bar / content / action bar | Light, dark or system; everything comes from tokens |
| **Search bar** | `[mark or back tile] [input, display 17] [accessory: dropdown]` | Root shows the accent mark (Search icon). Sub-views show a Back tile (clickable = Esc). The placeholder is `text_faint` |
| **Section header** | `Title  count`, `text_sm` in `text_faint`, sitting on the bottom of its slot | Not selectable; the cursor skips it |
| **List row** | `[pill][icon tile] Title  subtitle ······ accessory` | **Default:** transparent. **Hover:** `surface`. **Selected:** `surface` plus the 3×16 accent pill on the left edge. The title truncates first, the subtitle before the accessory |
| **Icon tile** | 26 px rounded `surface_strong` square holding a Fluent glyph; app/extension images draw at 20 px with no tile | Emoji draw at 18 px with no tile |
| **Accessory** | Right-aligned `text_md` in `text_muted`; tags are tinted pills | |
| **Key cap** | `text_xs` on `surface`, `border`, `r_cap`, 18 px tall, min width 18 | `↵` uses the Fluent ReturnKey glyph |
| **Action bar** | Left: context icon (16) + title, or a toast (coloured dot + text). Right: `Primary ↵ │ Actions Ctrl K` | The second action shows as `Ctrl ↵` in the panel and runs on Ctrl+Enter |
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
