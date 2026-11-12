# Notes: themes and settings

Working notes from reading the Raycast manual (Themes, Settings, Action Panel pages, read 2026-11-12), set against what procast has today. They record how an established launcher structures these things and what procast should decide. They are not a spec, and nothing here copies that product's artwork, palette or names.

## Themes

**What the manual describes**

- A theme is small: a background (solid, or a gradient), a few primary colours and a few support colours. It is not a full token set.
- Every theme is marked Light or Dark. The person assigns one theme to each, and the app follows the system appearance between the two. Following the system can be switched off to pin one theme.
- A built-in theme editor: start from an included theme or from scratch, edit colours by hex or picker, watch a live preview, name it, mark it Light or Dark, save.
- A separate command to browse installed themes and apply one.
- Themes travel as a link or as JSON on the clipboard. A public gallery installs one with a click.
- Custom themes are a paid feature there.

**Where procast is today**

- `[theme]` in the config: `mode` (system, dark, light), a named palette for each, and eight overrides (background, text, muted, selected, selected text, border, separator, container).
- Settings → Theme has a live preview built from the real launcher components, and swatches for the overrides.
- No editor beyond those fields, no sharing, no theme-switch command.

**Decisions for procast**

| Question | Recommendation | Why |
|---|---|---|
| How many built-in themes? | Two: Dark and Light, as in this system | Named palettes multiply every contrast check. Add more only once the glass and the tints are settled |
| What may a person change? | Six slots: body tint, text, muted text, accent, and whether glass is on. Everything else is derived | A small set is what makes sharing and contrast-checking possible. Kind tints stay fixed so icon colour keeps its meaning; selection is always neutral |
| Gradient backgrounds? | No | The body is glass over the wallpaper; a gradient fights it and is outside the resource budget |
| Follow the system? | Keep `mode = system` as the default, one theme per appearance | Matches what people expect from any desktop app |
| Guard rails | Refuse to save an override that drops text under 4.5:1 on the body; say which pair failed | An editor without this lets people make an unreadable launcher |
| Sharing | Later. A theme is already a few lines of TOML; "Copy theme" and "Paste theme" actions are enough | No gallery or account needed |
| Switching | A "Switch Theme" command in root search that lists themes and applies on Enter | Cheaper than opening Settings |
| Glass control | One toggle, "Glass", plus automatic fallback to `bg_solid` | Opacity sliders invite unreadable results |

## Settings

**What the manual describes**

- Opened by a shortcut while the launcher is open, by typing "Settings" in root search, or from an item's action panel straight to that item's settings. Settings has its own search.
- Tabs, roughly: account; general; launcher; shortcuts; keyboard; advanced; extensions; applications; AI; about.
- General holds: start at login, tray or menu-bar icon, the global hotkey, and appearance. Appearance holds: follow system, interface size (three steps), and window mode (compact or expanded, with an option to show favourites as icons in compact).
- Launcher holds: which screen to appear on, how long before the launcher resets to root, search sensitivity, clearing search history, extra result sources, and the fallback commands shown when nothing matches (add, reorder, remove).
- Shortcuts is one table of every hotkey and alias, filterable.
- Keyboard holds: what Esc does, list navigation key schemes, input-source switching.
- Advanced holds: favicon source, export and import of all settings, proxy.
- Each extension and app has its own entry: preferences, enable or disable per command, alias and hotkey.

**Where procast is today**

Nine sections: General, Launcher, Shortcuts, Apps, Clipboard, Files, Theme, Extensions, Advanced. Interface size has three steps (small, medium, large). Compact mode, a favourites strip, start at login and a tray icon exist.

**Notes for the Settings design**

| Topic | Note |
|---|---|
| Theme placement | Appearance belongs in General: follow system, the two themes, interface size, window mode, the Glass toggle. A separate Theme section is only needed once there is an editor |
| Settings search | Add a Field at the top of the sidebar. With nine sections people will not remember where a setting lives |
| Deep links | "Configure" in an item's ActionPanel should open Settings on that item's row, selected |
| Shortcuts | Keep one table for every hotkey and alias. Each row: IconTile, title, alias Field, HotkeyControl. Conflicts are reported in the footer |
| Fallback commands | A reorderable list of ListRows under Launcher. Their order is the order of the "Use … with" section |
| Launcher behaviour | Worth adding: which screen to open on, and how long before the query resets |
| Esc | One SegmentedControl: "Go back" or "Close" |
| Export and import | Advanced: two Quiet Buttons. The config is already one TOML file |
| Apply model | Everything applies at once. No Save button; the footer confirms the last change with a status dot |
| Layout | 192 px sidebar of Selectable rows; pages are `surface` cards with radius `r-lg` under ts-small labels; a row is a label column (title, optional ts-small description in `text_muted`) and a control column |
| Controls | On or off: Toggle. One of up to five: SegmentedControl. One of many: dropdown. Text: Field. Keys: HotkeyControl. No other control types |

## Action panel

**What the manual describes**

- Enter runs the primary action; a shortcut opens the full panel.
- The primary action is first; the rest follow in named sections.
- Each action shows its own shortcut on the right.
- A search field at the top filters actions; while filtering, sections flatten into one list.
- Some actions open a sub-list or an inline control (such as a hotkey recorder); Esc goes back one level, then closes.
- Destructive actions are marked in red.

**Notes for procast's ActionPanel**

- Today: no search, no sections, shortcuts shown only on the first two rows.
- Add sections once an item has more than about six actions, using SectionHeader at `control` height.
- Add a filter Field at the top only when the list is longer than the panel; otherwise it is noise.
- Show a KeyCap on every action that has a shortcut, not just the first two.
- Mark destructive actions: icon and label in `danger`. Selection stays neutral grey.
- "Set Hotkey" should open a HotkeyControl inline, in the panel, not jump to Settings.

## Not built in this system yet

Settings page (sidebar, cards, rows), theme editor, dropdown menu, form view, markdown detail view, favourites strip, split detail list.
