<p align="center">
  <h1 align="center">procast</h1>
  <p align="center"><b>A fast keyboard launcher for Windows that runs Raycast store extensions.</b><br>
  Native Rust + GPUI. No web view, no Electron.</p>
</p>

<p align="center">
  <img alt="Windows 11" src="https://img.shields.io/badge/Windows-11-black.svg?style=for-the-badge&logo=windows11&logoColor=fff">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-black.svg?style=for-the-badge&logo=rust&logoColor=fff">
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/License-MIT-black.svg?style=for-the-badge"></a>
</p>

<!-- SCREENSHOT: hero image goes here once the new icon set lands, e.g.
<p align="center"><img src="docs/screenshots/hero.png" width="760"></p>
-->

Press <kbd>Alt</kbd>+<kbd>Space</kbd>, type, hit <kbd>Enter</kbd>. Apps, files, clipboard history, snippets,
calculator, window management, system commands, built-in tools (timers, translator, dictionary, focus sessions, calendar, notes,
screenshot search, YouTube Music) and Raycast store extensions, all from one search bar. <kbd>Ctrl</kbd>+<kbd>K</kbd> shows every action for the selected item.

## Speed: procast vs Raycast for Windows

Measured on 2026-11-15 on the same laptop (Intel Core i7-10610U, 16 GB RAM, SSD, Windows 11 Pro),
Raycast 2.12.1 against a procast release build, each with its usual extensions installed.

| | Raycast | procast | |
|---|--:|--:|:-:|
| **Cold start**: launch → launcher ready | 9.95 s | **0.57 s** | 17× faster |
| **Hotkey → window on screen** | not measured¹ | **22 ms** | |
| **Memory**, private bytes, all processes | 448 MB in 9 processes | **255 MB** in 3 processes² | 1.8× less |
| **Memory**, working set | 857 MB | **229 MB** | 3.7× less |
| **Install size** | 540 MB | **37 MB** exe³ | 15× smaller |
| **Whole-drive file search**, per query | not measured | **16–41 ms** over 460k entries | |
| **UI stack** | WPF + WebView2 (React) + Node | Rust + GPUI on DirectX, UI and search in one process | |

<details>
<summary>How these were measured</summary>

- **Cold start**: kill the app, wait 3 s, start it (Raycast through its Start menu entry), then poll every 2 ms until the
  launcher can paint: for procast its GPUI window exists, for Raycast its window holds the WebView2 render widget.
  Median of 5 runs (Raycast 9.2–11.1 s, procast 0.47–0.72 s after a first 3.1 s run from a cold disk cache).
- **Hotkey → window on screen**: send <kbd>Alt</kbd>+<kbd>Space</kbd> with `SendInput`, then poll the pixel at the
  window's centre until it changes. Median of 30 runs, max 37.5 ms.
- **Memory**: a few minutes after a fresh start, summed over the app's process tree (`PrivateMemorySize64`, `WorkingSet64`).
- **Install size**: Raycast's `WindowsApps` folder (which ships its own `node.exe`) against `procast.exe`.
- **File search**: `cargo test bench_real_index -- --ignored --nocapture`, median of 5 runs per query on a dev build.

¹ Raycast's keyboard hook ignores injected key presses, so an automated harness can't press its hotkey.<br>
² Launcher 82 MB, whole-drive indexer 51 MB, extension host 122 MB. The extension host only runs while an extension
does; without it procast uses 133 MB. Known issue: the extension host's memory grows while interval background
extensions run, so these figures are for a fresh start.<br>
³ Extensions and Quick AI also need Node.js and the `exthost` packages (367 MB, almost all of it the Claude Agent SDK).

</details>

## Features

**Launcher**
- Fuzzy search over apps (Start Menu, Store apps, portable folders) with frecency ranking; new installs show up on their own
- Global hotkey, double-tap <kbd>Ctrl</kbd> / <kbd>Shift</kbd> / <kbd>Alt</kbd>, and a Hyper key (Caps Lock, Right Alt, Right Ctrl or Scroll Lock becomes Ctrl+Alt+Shift+Win)
- A hotkey and an alias for any command, app, quicklink, script or extension, with conflict warnings
- Favourites, hide or disable any item, several reorderable fallback commands (web search, GitHub, …)
- Compact mode (search bar only until you type), search history on <kbd>Up</kbd>, Vim / Emacs navigation keys, game mode
- Shows on the screen with the mouse, the active window, or the primary display

**Calculator**: math, units, dates and currencies (`10 eur to usd`), powered by `fend`, with daily ECB rates cached offline

**Clipboard history**
- Text, images and files, with thumbnails and a detail pane (source app, size, time)
- Text inside copied images is searchable (Windows built-in OCR, offline)
- Pin entries, keep history by count or for 1 hour … 1 year, skip password managers and other apps
- Encrypted at rest with Windows DPAPI

**Snippets and quicklinks**
- Snippets expand as you type anywhere (`;sig`), with `{date}`, `{time}`, `{clipboard}`, `{cursor}`, `{selection}` and `{argument}` placeholders
- Trigger, speed, completion sound, expand-within-words and per-app opt-out settings
- Quicklinks with `{query}`, `{selection}` or `{clipboard}` arguments and favicons
- Manage both from Settings

**Emoji**: grid picker (4–16 columns), skin tones, paste or copy; `:thumbsup:` typed anywhere turns into 👍

**File search**
- Whole-drive search over every NTFS drive from the MFT, kept live through the USN journal (optional admin helper)
- Or a live index of chosen folders, with `.gitignore` and glob ignore rules
- Detail pane with preview, size and modified date; index status and rebuild in Settings

**Window management**
- Halves, thirds, two-thirds, quarters, sixths, centre, almost maximise, reasonable size, next display
- Cycling (Left Half again goes ½ → ⅔ → ⅓), window and screen gaps
- Minimise others, close others, move a window to the next virtual desktop, layout presets that arrange several apps at once

**Apps and system**
- App hotkeys: launch, focus existing, or toggle; quit and force-quit; auto-quit idle apps
- Lock, sleep, restart, shut down, sign out, empty bin, dark mode, volume, Wi-Fi and Bluetooth toggles
- Restart Explorer, toggle hidden files, eject USB drives, empty temp, 55 Windows Settings pages
- Windows Run with your choice of cmd, PowerShell or pwsh

**Extensions**
- Runs unmodified Raycast store extensions on procast's own `@raycast/api` runtime: List, Grid, Detail, Form, ActionPanel, navigation, toasts, storage, preferences
- Browse and install the ~1,000 Windows-capable store extensions from the launcher
- Menu-bar extensions become tray icons with live menus; interval background commands run on schedule
- Raycast-style script commands (`@raycast.*` headers, arguments, output modes)

**Browser companion**: search open tabs in Chrome, Edge or Brave and switch to an open tab instead of opening a new one

**YouTube Music**
- Search songs, albums, artists and playlists in-process (no Chrome); browse your library (albums, playlists, artists, liked songs) in a cover grid
- Now Playing and Quick Actions (like, dislike, play/pause, next, previous, volume, shuffle, repeat), each bindable to a hotkey; the YouTube Music Desktop App plays
- Optional tray player; **Import YouTube Music Session** is optional for the full library

**Built-in tools**
- **Timers and stopwatch**: `timer 5m tea`; running ones head root search to pause, extend or stop
- **Translator**: Google Translate from the search bar or on the selection (Instant Translate Copy / Paste), with language sets
- **Dictionary**: `define dog`, offline (WordNet, downloaded once), with spelling suggestions and similar words
- **Focus sessions**: block (or allow only) apps and websites for a while, with categories, snooze and a Focus pill; Raycast Focus categories import
- **Calendar**: My Schedule and Join Meeting from ICS feeds (Google, Outlook), with a reminder before each meeting
- **Screenshots**: search your screenshot folders by the text in each picture (Windows OCR), Quick Look, pin, paste the latest
- **Notes**: a floating Markdown notes window with a live editor, search, and recently deleted notes
- **Screen Time**: records which apps and websites are in front (idle time is skipped), shows today, this week and this month by category, app and site in a dashboard inside the launcher (**Screen Time**, **Screen Time This Week**). Off by default (`[screen_time] enabled`); websites need the browser extension; **Import Screen Time from micro bot** brings that tracker's history in
- **Auto HDR for games**: turns HDR on while a full-screen game is in front

**AI**: Quick AI from the search bar with follow-ups, running the Claude Agent SDK on your own Claude Code login

**Settings and themes**
- Settings window for everything, which writes a plain `config.toml` (comments kept) that hot-reloads
- Dark and Light glass themes (follow Windows, or pick one), three interface sizes

## procast vs Raycast for Windows

procast 0.1.0 (`main`, 2026-11-18) against Raycast for Windows 2.12.1, checked feature by feature against
Raycast's settings window and procast's code. ✅ has it · ◐ partly · ⬜ not yet.
Of the 60 rows below, procast has 36, has 9 in part and lacks 14 (cloud account and sync are not planned);
Raycast lacks 4: YouTube Music on Windows, open source, a plain-text settings file and a native UI.
The open items are tracked in [TODO.md](TODO.md) and [docs/roadmap.md](docs/roadmap.md).

| Feature | Raycast | procast |
|---|:-:|:-:|
| **Launcher** | | |
| Fuzzy app search with frecency | ✅ | ✅ |
| Global hotkey, double-tap modifier | ✅ | ✅ |
| Hyper key | ✅ | ◐ 4 keys to choose from, Raycast has more |
| Hotkey and alias for any command | ✅ | ✅ |
| Favourites, hide / disable items, fallback commands | ✅ | ✅ |
| Compact mode, search history, Vim / Emacs keys, game mode | ✅ | ✅ |
| Calculator with units and currencies | ✅ | ✅ |
| Light and dark themes | ✅ | ✅ |
| **Clipboard history** | | |
| Text, images and files | ✅ | ✅ |
| OCR search in copied images | ✅ | ✅ |
| Pins, detail pane, keep-for-duration, encrypted at rest | ✅ | ✅ |
| "Never record from" picker listing installed apps | ✅ | ◐ type exe names |
| **Snippets and quicklinks** | | |
| Auto-expand while typing anywhere | ✅ | ✅ |
| `{cursor}` `{selection}` `{argument}` placeholders | ✅ | ✅ |
| Manage in Settings | ✅ | ◐ no disable |
| Quicklinks with arguments, favicons, selected text | ✅ | ✅ |
| Quicklink library of presets | ✅ | ⬜ |
| **Emoji** | | |
| Grid picker, skin tones, paste or copy | ✅ | ✅ |
| Inline `:` picker in any text field | ✅ | ◐ `:name:` expands, no popup |
| **File search** | | |
| Whole-drive NTFS index, live | ✅ | ✅ |
| Detail pane, `.gitignore` rules, index status | ✅ | ✅ |
| Use Everything when installed | ✅ | ⬜ |
| File content search (Windows Search) | ✅ | ⬜ |
| **Window management** | | |
| Halves to sixths, gaps, cycling, next display | ✅ | ✅ |
| Virtual desktops | ✅ | ✅ |
| Layouts that arrange several apps | ✅ | ◐ config only |
| Custom size and position presets | ✅ | ⬜ |
| **Apps and system** | | |
| App hotkeys (launch / focus / toggle), quit, force-quit | ✅ | ✅ |
| Auto-quit idle apps | ✅ | ◐ one duration for all apps |
| Power, volume, Wi-Fi, Bluetooth, Explorer, drives | ✅ | ✅ |
| Windows Settings pages, Windows Run | ✅ | ✅ |
| Script commands | ✅ | ✅ |
| Several script folders, per-script settings in Settings | ✅ | ⬜ |
| **Extensions** | | |
| Run Raycast store extensions | ✅ | ✅ |
| Store browser and install | ✅ | ✅ |
| Menu-bar extensions | ✅ | ✅ as tray icons |
| Extension updates, OAuth sign-in, `AI.ask`, developer mode | ✅ | ⬜ |
| **Browser companion** | | |
| Search open tabs, prefer an open tab | ✅ | ✅ |
| Browser history and bookmarks | ✅ | ⬜ |
| **AI** | | |
| Quick AI from the search bar | ✅ | ✅ on your Claude Code login |
| AI chat with saved history | ✅ | ◐ follow-ups, not saved |
| Model choice | ✅ many, local models, own API keys | ◐ Claude Haiku / Sonnet / Opus |
| AI commands, agents, MCP servers, automations | ✅ | ⬜ |
| Dictation | ✅ | ⬜ |
| **Built-in tools** | | |
| YouTube Music controls and library | ⬜ (macOS-only store extension) | ✅ |
| Translator, dictionary, calendar | ✅ | ✅ |
| Contacts | ✅ | ⬜ |
| Floating notes, focus sessions, timer, screenshot search | ✅ | ✅ |
| Screen time (apps and sites, per day, week and month, dashboard) | ⬜ | ✅ |
| Auto HDR | ✅ | ✅ |
| Taskbar widget | ✅ | ⬜ |
| **App** | | |
| Settings window | ✅ | ✅ |
| Search in Settings, export / import / backup | ✅ | ⬜ |
| Installer | ✅ | ✅ |
| Auto-update, About page with diagnostics | ✅ | ⬜ |
| System proxy | ✅ | ◐ translator only |
| Hotkeys over admin windows | ✅ | ⬜ |
| Account, cloud sync, organizations | ✅ | not planned |
| **Openness** | | |
| Open source | ⬜ | ✅ MIT |
| Plain-text settings file, hot-reloaded | ⬜ encrypted SQLite | ✅ `config.toml` |
| Native UI, no web view | ⬜ WPF + WebView2 + Node | ✅ Rust + GPUI |

## Getting started

**Install**: download `procast-setup-<version>.exe` from [Releases](https://github.com/akhi-543/procast/releases) and run it. It installs for your user only (no admin), adds a Start menu entry, and can start procast at sign-in. Uninstall from Settings → Apps. A portable `procast.exe` is attached too.

**Or build from source**. You need
- Windows 11
- [Rust](https://rustup.rs) 1.95 or newer with the MSVC toolchain, Visual Studio 2022 Build Tools ("Desktop development with C++") and the Windows SDK
- [Node.js](https://nodejs.org), npm and git, only for extensions and Quick AI

**Build and run**

```powershell
git clone https://github.com/akhi-543/procast.git
cd procast
cargo build --release        # the first build takes about 10 minutes
cd exthost; npm ci; cd ..    # only for extensions and Quick AI
.\target\release\procast.exe
```

Press <kbd>Alt</kbd>+<kbd>Space</kbd>. procast lives in the tray and can start at login (Settings → General).

**Keys**

| Key | Does |
|---|---|
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Open or close procast |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> | Clipboard history |
| <kbd>Enter</kbd> | Run the selected item |
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | All actions for the selected item |
| <kbd>Esc</kbd> | Back, then close |

**Optional setup**
- Whole-drive file search: run **Enable Fast File Search** once (asks for admin), then turn on Settings → Files → Search whole drives
- Extensions: **Browse Extension Store**, or **Install Extension** with a store name (`obsidian`) or a local folder
- Browser tabs: run **Set Up Browser Extension**, then in Chrome, Edge or Brave load `%APPDATA%\procast\browser-extension` with "Load unpacked"
- After updating procast, reload the unpacked extension on the browser's extensions page: Screen Time's website tracking needs the newer one
- YouTube Music: run the YouTube Music Desktop App with its companion server on, then approve procast in that app (procast does not launch it). Search does not need Chrome. For the full library, run **Set Up Browser Extension**, reload it once after an update, then **Import YouTube Music Session**. Without that, library shelves ask you to sign in on music.youtube.com; Playlists can still fill from the desktop app. Settings → YouTube Music turns on the tray player
- Quick AI: sign in to [Claude Code](https://claude.com/claude-code) once; procast uses that login

Settings live in `%APPDATA%\procast\config.toml`. Edit them in the Settings window or by hand; changes apply within two seconds.

## Project layout

| Path | What |
|---|---|
| `src/` | The launcher: one Rust crate (GPUI window, search, built-in commands, settings, NTFS indexer) |
| `exthost/` | Node sidecar: procast's own `@raycast/api` runtime, extension installer, Quick AI |
| `browser/` | Companion browser extension (native messaging) |
| `assets/` | Icons and font |
| `docs/` | Roadmap, design system, AI research |
| [`PLAN.md`](PLAN.md) | How Raycast for Windows works and how procast differs |

## License

MIT, see [LICENSE](LICENSE). The bundled Space Grotesk font is under the SIL Open Font License ([assets/fonts/OFL.txt](assets/fonts/OFL.txt)).

procast is an independent project, not affiliated with or endorsed by Raycast. "Raycast" is a trademark
of its owner and is used here only to describe compatibility and compare features. No Raycast code,
icons, art or branding are included; extension support comes from implementing the public
`@raycast/api` (MIT type definitions) in procast's own runtime.
