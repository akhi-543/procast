# procast TODO: what Raycast still has that procast doesn't

Source: the Raycast **2.12.0** settings window (labels and options read from its installed settings pages,
`frontend/*-settings-view-*.js`), compared with procast's code on `design-system` (2026-11-13).
Extensions (runtime, store, updates, OAuth, `AI.ask`, dev mode, per-extension settings) are left out on purpose.
IDs in brackets point at [docs/roadmap.md](docs/roadmap.md) where the item already has a row; **new** means it appeared since the 2.10.1 audit.

Size: **S** under a day · **M** a few days · **L** a week or more.

## Clipboard history
- [ ] Keep history by time instead of count: 5 min / 1 h / 1 day / 1 week / 1 month / 3 months / 6 months / 1 year / unlimited (warn that unlimited grows on disk) [G1] — S
- [ ] Images and files in history, with preview [G2] — M
- [ ] Text recognition (OCR) in copied images, searchable; toggle in settings [G3] — M
- [ ] Pin entries [G4] — S
- [ ] Detail pane: full text, source app, size, copied time [G5] — S
- [ ] "Never record from" picker that lists installed apps instead of typing exe names — S

## Snippets
- [ ] Auto-expand while typing anywhere [H1] — M
- [ ] Expansion trigger: immediately / after delimiter (keep it) / after delimiter (discard it) — S
- [ ] Expansion speed: instant / quick / default / delayed / extended — S
- [ ] Completion sound: none or a Windows system sound (Ding, Notify, …) — S
- [ ] Expand within words, apps where expansion is off [H2] — S
- [ ] Placeholders `{cursor}`, `{selection}`, `{argument}` (date/time/clipboard exist) [H3] — S
- [ ] Manage snippets in Settings (add, edit, duplicate, disable, delete) instead of config.toml [H4] — M

## Quicklinks
- [ ] Quicklinks page in Settings: add, edit, duplicate, disable, delete, copy link [H4] — M
- [ ] Quicklink library with featured presets to add in one click — S
- [ ] Favicons on quicklinks [M1] — S
- [ ] Selected text as the quicklink argument [M2] — M

## Emoji
- [ ] Grid layout with column count [I1] — S
- [ ] Primary action: paste to active app or copy [I2] — S
- [ ] Skin tone: none / light / medium light / medium / medium dark / dark [I2] — S
- [ ] Inline `:` picker in any text field [I3] — M

## File search
- [ ] Use Everything when it's installed [A5] — M
- [ ] Content search through Windows Search: in Search Files only, or also in root search; tell the user when Windows Search isn't indexing the chosen folders [A7] — M
- [ ] Content index on network and removable drives (toggle) — S
- [ ] OCR page limit for scanned PDFs — S, needs A7
- [ ] Stop indexing when the index would not fit on disk — S
- [ ] Ignore patterns: `.gitignore` / `.ignore` and globs, not only names [J1] — S
- [ ] Hidden files toggle; network and removable drives toggle [J2] — S
- [ ] Detail pane: preview, size, modified date [J3] — M
- [ ] Index status: disk usage, total entries, files / folders / symlinks / invalid, last crawl duration [J4] — S
- [ ] Rebuild index, reset file-search settings to defaults, restore a removed folder — S

## Window management
- [ ] Quarters, sixths, top/bottom thirds, reasonable size [K1] — S
- [ ] Window gap and screen gap [K2] — S
- [ ] Cycle on repeat: Left Half again goes ½ → ⅔ → ⅓; cycle displays on repeat [K3] — S
- [ ] Minimize, minimize others, close others, toggle full screen [K4] — S
- [ ] Virtual desktops: move window to next desktop, auto-close empty ones [K5] — M
- [ ] Custom commands: own size presets (tiny / small / medium / large / extra large) and positions — M
- [ ] Layouts: one command opens and arranges several apps, quicklinks, files or URLs (with arguments); "Add visible windows" captures the current arrangement; reorder front/back [K6] — M

## Applications
- [ ] Auto Quit: "quit apps closed to tray" option (today procast only closes windows politely) — S
- [ ] Auto Quit: per-app duration plus a default duration — S
- [ ] Apps page: per-app "toggle visibility / always show" in root search — S

## Keyboard
- [ ] More Hyper key choices: Left Ctrl / Shift / Alt / Win, Right Shift / Win, F10–F12, Copilot key (procast has Caps Lock, Right Alt, Right Ctrl, Scroll Lock) — S
- [ ] What a lone Hyper tap does: nothing, or send Escape — S
- [ ] Tab / section switching keys: square brackets or arrow keys (Raycast's option; exact scope unverified) — S
- [ ] Hyper key diagnostics: status, conflicting apps, copy log — S

## System commands
- [ ] Volume up / down / mute, toggle Wi-Fi / Bluetooth [L1] — S
- [ ] Toggle hidden files, restart Explorer, eject drive, empty temp [L2] — S
- [ ] Windows Settings commands (`ms-settings:` pages), as their own category in Shortcuts [L3] — S
- [ ] Windows Run: default shell (cmd / PowerShell / pwsh), run in terminal [L4] — S

## Script commands
- [ ] Several script folders, managed in Settings (add, remove, reveal) [N5] — S
- [ ] Per-script enable / disable / delete, background refresh interval, inline output [N5] — S

## Settings window and app
- [ ] Search box in Settings [P1] — S
- [ ] Export / import settings; scheduled backup (daily / weekly / monthly) to a folder, keep latest / last 5 / last 10 [P2] — S
- [ ] System proxy and extra CA certificate for network calls [P3] — S
- [ ] Sync between PCs through a folder (OneDrive / Git), procast's answer to Cloud Sync [P4] — M
- [ ] About page: version, diagnostics ("all checks passed"), updates [P6] — S
- [ ] Auto-update + installer (winget / MSIX) [P6] — M
- [ ] Hotkeys over elevated windows (UIAccess helper, needs signing) [P5] — L

## Built-in tools Raycast ships
- [ ] Translator, with saved "translate to…" commands; output opens the translator or replaces the selection in place [O3] — S
- [ ] Dictionary / define word [O4] — S
- [ ] Calendar: upcoming meetings, join link, reminder 2–30 min before, confirm before joining, ignore keywords [O5] — M
- [ ] Contacts search, sort by first or last name — M
- [ ] Focus sessions: block apps and sites, snooze 1–10 min [O6] — M
- [ ] Floating notes [O1] — M
- [ ] Screenshot search: several folders, retention 1 day … unlimited, OCR [O2] — M
- [ ] Timer / stopwatch [O7] — S
- [ ] **new** Taskbar widget: performance monitor and calendar, docked top / bottom / left / right, flyout — M
- [ ] **new** Browser companion: search open tabs, history, bookmarks; paired-browser list and diagnostics [M3] — L
- [ ] Auto HDR while a game is in front [O8] — S

## AI (beyond the Quick AI / Ask AI that exist)
- [ ] AI commands: saved prompts over selection or clipboard; create, duplicate, disable, export [U2] — M
- [ ] AI chat window with saved history, pinned chats [U3] — L
- [ ] **new** Projects: group chats with their own instructions, working folder, and memory — L
- [ ] **new** Profile and memory: context sent with every chat; import / reset memory — M
- [ ] **new** Tool permission rules: allow automatically / ask first, allow list, reset — M
- [ ] **new** Agents: named presets with model, instructions, reasoning effort, creativity — M
- [ ] **new** Automations: run a prompt once / hourly / daily / on a cron schedule — M
- [ ] **new** Skills: folders scanned for `SKILL.md` — M
- [ ] **new** MCP servers: add, enable, disable, uninstall — M
- [ ] **new** Models: pick a model per feature (Quick AI, chat, commands), local Ollama models, bring-your-own API keys / subscriptions — M
- [ ] **new** Screen awareness: AI reads the active window — L
- [ ] Dictation: speech to text, language, output (paste / copy / both), pause or duck media while recording, vocabulary, per-app styles, stats (WPM, time saved) [U4] — L

## Not planned (Raycast cloud services)
Account, organizations, Pro credits and usage, Cloud Sync servers, Remote (phone pairing).
