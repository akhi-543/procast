# akscast roadmap: pick what to build

How this list was built: every row title and description in Raycast for Windows 2.10.1's
settings (`frontend/*-settings-page-definition-*.js` and the settings views), compared with akscast.

**How to choose:** tick `[x]` next to what you want, or just reply with the IDs (e.g. "A1 A2 C3 D1").

Status: ✅ have · ◐ partial · ✗ missing. Size: **S** under a day · **M** a few days · **L** a week or more.
**You:** what I need from you before or while building it. Blank means I can build it alone.

---

## A. Speed: indexing and search

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [x] | A1 | **App list cached on disk** (`apps.bin`, names + icons). 208 apps load in 3.6 ms at start; icons are extracted only for new apps ("Reload Apps" re-extracts all) | ✅ | S | |
| [x] | A2 | **New apps appear by themselves.** Start Menu folders and `%LOCALAPPDATA%\Packages` are watched; an install or uninstall triggers a rescan within ~10 s | ✅ | S | |
| [x] | A3 | **Live file index.** `ReadDirectoryChangesW` on each folder applies creates, deletes, renames and moves as they happen. Safety re-crawl at most hourly | ✅ | M | |
| [x] | A4 | **File index saved to disk** (`files.bin`). 138,617 entries load in 54 ms vs a 1.3 s crawl; one background crawl at start catches changes made while akscast was off | ✅ | S | |
| [ ] | A5 | **Use Everything if it's installed.** Ask voidtools Everything over IPC: whole drives, instant, no index of our own | ✗ | M | install Everything (free) |
| [x] | A6 | **NTFS MFT scanner + USN journal** (what Raycast's "NTFS Scanner" service does). Indexes whole drives in seconds and stays live. Needs an elevated helper service | ✅ enabled: C:, D:, E:: 2.7 M entries in ~10 s at logon, 20–44 ms per search, changes show in ~60 ms | L | OK to install an admin service |
| [ ] | A7 | **File content search** via the Windows Search index (Raycast does the same) | ✗ | M | turn on Windows Search indexing for your folders |
| [x] | A8 | Faster matching over large file indexes (prefilter before fuzzy match). Only worth it if A5/A6 make the index millions of files | ✅ parallel prefilter in the NTFS search | S | |

## B. General

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Open at login, launcher hotkey, follow system appearance | ✅ | | |
| [x] | B1 | Toggle "Show in system tray" (today the akscast tray icon is always on) | ✅ | S | |
| [x] | B2 | Named themes: pick a dark theme and a light theme from a list | ✅ akscast Dark, Midnight, Graphite, Forest · akscast Light, Paper, Mist | S | |
| [x] | B3 | Theme studio: visual colour editor with live preview | ✅ live preview + swatches in Settings → Theme | M | |
| [x] | B4 | Interface size: small / default / large | ✅ | S | |
| [x] | B5 | **Compact window mode:** only the search bar until you type, then it expands | ✅ Down opens the list | M | |
| [x] | B6 | Favourites shown as icons in compact mode | ✅ | S | needs B5 + D4 |

## C. Launcher (root search)

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [x] | C1 | Show on: screen with the mouse / **active window** / primary | ✅ | S | |
| [x] | C2 | Pop to root after closing: immediately / after 5–180 s (today akscast always resets) | ✅ | S | |
| [x] | C3 | Root search sensitivity: low / medium / high | ✅ | S | |
| [x] | C4 | Include files in root search | ✅ | S | |
| [x] | C5 | Search history: Up arrow restores recent queries | ✅ + Clear Search History | S | |
| [x] | C6 | Hide any item from root search | ✅ | S | |
| [x] | C7 | Several fallback commands (Google, GitHub, ChatGPT, …), reorderable | ✅ | S | |

## D. Shortcuts page (the Raycast "Shortcuts" table)

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [x] | D1 | **One table of every command** (apps, built-ins, extensions, quicklinks, scripts), filterable by category | ✅ Settings → Shortcuts: category + name filter | M | |
| [x] | D2 | **Hotkey for any command or app**, with conflict warning | ✅ `[launcher.hotkeys]`; clashes refused in Settings, other apps' clashes reported | M | |
| [x] | D3 | **Alias for any command** (type `cl` to run Clipboard History) | ✅ exact alias goes first, even for hidden items | S | |
| [x] | D4 | Favourites (pinned to the top of root search) | ✅ Ctrl+K → Add to Favorites | S | |
| [x] | D5 | Enable / disable any command or extension | ✅ Ctrl+D in Shortcuts; `ext:<name>` for a whole extension | S | |

## E. Keyboard

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [x] | E1 | Escape: go back vs close window and pop to root | ✅ | S | |
| [x] | E2 | Navigation bindings: Emacs (Alt+B/F/P/N) or Vim (Alt+H/J/K/L) | ✅ move + back (F/L, open, left out on purpose) | S | |
| [x] | E3 | **Hyper key:** Caps Lock (or another key) becomes Ctrl+Alt+Shift+Win | ✅ Caps Lock / Right Alt / Right Ctrl / Scroll Lock; `Hyper+K` hotkeys | M | |
| [x] | E4 | Double-tap a modifier to open (Raycast's default is double-tap Win) | ✅ Ctrl / Shift / Alt (Win left out: its tap opens Start) | M | |
| [x] | E5 | Game mode: hotkeys off while a full-screen game is focused | ✅ | S | |

## F. Applications

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [x] | F1 | App hotkey action: launch new / focus existing / toggle (hide if focused) | ✅ Store apps by app ID, others by program path | S | |
| [x] | F2 | Extra app folders (portable apps), optional deep search | ✅ Settings → Apps | S | |
| [x] | F3 | Auto-quit apps you haven't used for N minutes | ✅ listed programs, polite close | M | |
| [x] | F4 | "Quit app" / "Force kill process" commands | ✅ Quit Applications: Enter quits, Ctrl+K → Force Quit | S | |

## G. Clipboard history

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Text history, ignored apps, encrypted at rest | ✅ | | |
| [ ] | G1 | Keep history for a duration (day / week / month) instead of an entry count | ◐ count only | S | |
| [ ] | G2 | **Images and files in history**, with preview | ✗ | M | |
| [ ] | G3 | OCR: search text inside copied images (Windows built-in OCR, offline) | ✗ | M | needs G2 |
| [ ] | G4 | Pin entries | ✗ | S | |
| [ ] | G5 | Detail pane with full text, source app, size | ✗ | S | |

## H. Snippets

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Search and paste snippets, `{clipboard}` placeholder | ✅ | | |
| [ ] | H1 | **Auto-expand while typing anywhere** (`;sig` turns into the text) | ✗ | M | |
| [ ] | H2 | Expansion options: expand within words, delay, completion sound, disabled apps | ✗ | S | needs H1 |
| [ ] | H3 | More placeholders: `{date}`, `{time}`, `{cursor}`, `{selection}`, `{argument}` | ◐ | S | |
| [ ] | H4 | Manage snippets and quicklinks from a UI (today: edit config.toml) | ✗ | M | |

## I. Emoji

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [ ] | I1 | Grid layout with column count | ✗ list | S | |
| [ ] | I2 | Primary action: paste or copy · skin tone | ✗ | S | |
| [ ] | I3 | Inline `:` emoji picker in any text field | ✗ | M | needs the same keyboard hook as H1 |

## J. File search

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Folders to search, names to skip | ✅ | | |
| [ ] | J1 | Respect `.gitignore` / `.ignore`, glob ignore patterns | ◐ names only | S | |
| [ ] | J2 | Include hidden files toggle · network and removable drives toggle | ✗ | S | |
| [ ] | J3 | Detail pane: preview, size, modified date | ✗ | M | |
| [ ] | J4 | Index status: entries, last crawl time, "Rebuild index" | ✗ | S | |

## K. Window management

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Halves, thirds, two-thirds, maximize, almost maximize, center, restore, next display | ✅ | | |
| [ ] | K1 | More layouts: quarters, sixths, top/bottom thirds, reasonable size | ✗ | S | |
| [ ] | K2 | Window gap and screen gap | ✗ | S | |
| [ ] | K3 | Cycling (pressing Left Half again cycles ½ → ⅔ → ⅓) | ✗ | S | |
| [ ] | K4 | Minimize, minimize others, close others, toggle full screen | ✗ | S | |
| [ ] | K5 | Virtual desktops: move window to next desktop, auto-close empty desktops | ✗ | M | |
| [ ] | K6 | Layout presets: one command arranges several apps | ✗ | M | your layouts |

## L. System commands

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Lock, sleep, restart, shut down, sign out, empty bin, toggle dark mode | ✅ | | |
| [ ] | L1 | Volume up/down/mute, toggle Wi-Fi / Bluetooth | ✗ | S | |
| [ ] | L2 | Toggle hidden files, restart Explorer, eject drive, empty temp | ✗ | S | |
| [ ] | L3 | **Windows Settings commands** (`ms-settings:` pages: Display, Bluetooth, Sound, …) | ✗ | S | |
| [ ] | L4 | Windows Run: choose shell (cmd / PowerShell / pwsh), run in terminal | ◐ `>` prefix | S | |

## M. Quicklinks and browser

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [ ] | M1 | Favicons on quicklinks | ✗ | S | |
| [ ] | M2 | Pass selected text as the quicklink argument | ✗ | M | |
| [ ] | M3 | Browser companion extension: search open tabs, prefer existing tab | ✗ | L | install a browser extension |

## N. Extensions

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| | — | Run unmodified Raycast extensions, store browser, install, preferences, menu-bar commands in akscast | ✅ | | |
| [ ] | N1 | Update installed extensions ("Updates available" in the store) | ✗ | S | |
| [ ] | N2 | OAuth for extensions that need sign-in (GitHub, Linear, Spotify, …) | ✗ | M | |
| [ ] | N3 | `AI.ask` for extensions | ✗ | M | choose the AI backend (see U1) |
| [ ] | N4 | Developer mode: `ray develop`, auto-reload on save | ✗ | S | only if you'll write extensions |
| [ ] | N5 | Script commands: more folders, inline output, refresh interval | ◐ | S | |

## O. Built-in tools Raycast has

Each of these could also be a store extension instead of built-in code.

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [ ] | O1 | Floating notes (markdown, always-on-top window) | ✗ | M | |
| [ ] | O2 | Screenshot search (index your screenshots folder + OCR) | ✗ | M | which folder |
| [ ] | O3 | Translator | ✗ | S | which service (DeepL / Google / AI) + key |
| [ ] | O4 | Dictionary / define word | ✗ | S | which source |
| [ ] | O5 | Calendar: upcoming meetings, join link | ✗ | M | Google / Outlook account |
| [ ] | O6 | Focus: block apps and sites for a session | ✗ | M | |
| [ ] | O7 | Timer / stopwatch | ✗ | S | |
| [ ] | O8 | Auto HDR when a game is in the foreground | ✗ | S | |

## P. Settings, backup, service features

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [ ] | P1 | Search box in the settings window | ✗ | S | |
| [ ] | P2 | Export / import settings, scheduled backup to a folder | ✗ | S | backup folder |
| [ ] | P3 | Use the system proxy and a custom CA certificate for network calls | ✗ | S | only behind a corporate proxy |
| [ ] | P4 | Sync between your PCs (config.toml + data via OneDrive / Git) | ✗ | M | which sync folder |
| [ ] | P5 | Hotkeys over elevated (admin) windows: UIAccess helper | ✗ | L | a code-signing certificate |
| [ ] | P6 | akscast auto-update + installer (winget / MSIX) | ✗ | M | where releases live (GitHub?) + signing |
| — | — | Account, organizations, cloud sync, iPhone remote, Pro credits | not planned | | Raycast cloud services |

## U. AI (Phase 5, research in [ai-research.md](ai-research.md))

| | ID | Feature | Status | Size | You |
|---|---|---|---|---|---|
| [ ] | U1 | Quick AI: ask from the search bar, answer streams into a detail view | ✗ | M | **backend:** Claude Code CLI (your subscription) or API key |
| [ ] | U2 | AI commands: saved prompts over selected text / clipboard | ✗ | M | your prompts |
| [ ] | U3 | AI chat window with history | ✗ | L | |
| [ ] | U4 | Dictation (speech to text) | ✗ | L | local (Whisper) or cloud |
