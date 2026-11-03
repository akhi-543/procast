# akscast: plan

A keyboard launcher for Windows that runs Raycast store extensions. It is your own product, with its own name, look and settings. Raycast's code, icons, art and branding are not copied. Compatibility comes from implementing the **public** extension API (MIT type definitions) in our own runtime.

Status: **Phase 0 done (2026-11-03).** Results are in §4, and the spikes live in `spikes/`.

---

## 1. How Raycast for Windows works (installed app, v2.10.1)

Sources: `C:\Program Files\WindowsApps\Raycast.Raycast_2.10.1.0_x64__qypenmj9wpt2a\`, `%LOCALAPPDATA%\Raycast`, logs, and the running processes.

```
Raycast.exe  (.NET 10 WPF host, Wpf.Ui, CommunityToolkit.Mvvm, Serilog, Sentry)
 ├─ msedgewebview2.exe x7   frontend: React + TanStack Query (Vite build)
 │     bridge: chrome.webview.hostObjects.ipcRequest + webview "message" events
 ├─ node.exe backend\index.mjs   (115 MB node.exe, 8 MB bundle)
 │     ├─ Rust napi addons: data, indexer, calculator-rs, fs-utils, process-utils
 │     ├─ Swift on Windows: swiftCore, Foundation, SoulverCore (calculator)
 │     ├─ vscode-jsonrpc over \\.\pipe\vscode-jsonrpc-*
 │     └─ extension host: worker_thread per command, heap limit per worker
 ├─ Raycast.UIAccess.exe   low-level keyboard hook + SetForegroundWindow
 └─ SystemService / NetworkMonitor / TraceRecorder / CrashHelper / StartupLauncher
```

| Observed fact | Evidence |
|---|---|
| ~980 MB private memory in total (host 308, node 272, WebView2 ~392) | process list |
| ~12 SQLite DBs, **encrypted at rest** | read-only open fails ("file is not a database"), `last_key` file |
| Window recentred and DPI-rescaled on every show (~900×/day) | logs |
| USN indexer fails on `E:\` (883×/day) and has no fallback. Indexer refers to `SystemService` over a pipe, and the manifest has `allowElevation`, so volume reads probably go through an elevated helper (inferred) | logs, strings |
| **Main hotkey = double-tap Win**, dispatched by a low-level keyboard hook in the UIAccess process. Other bindings use `RegisterHotKey`. The hook **ignores injected input**: synthetic double-taps do nothing | logs (`Binding 0 (DoubleTap…) dispatches via the keyboard hook`), test |
| Extension runtime (`api/node_modules/@raycast/api/index.js`, 744 KB, react-reconciler inside) **ships only inside the app** and has no license file. npm `@raycast/api@2.10.1` (MIT) contains **only the `ray` CLI + `types/index.d.ts`** | tarball inventory |

Hot path: keypress → WPF → WebView2/React → COM host object → JSON-RPC pipe → Node → napi/Swift and back. That's four boundaries per keystroke.

---

## 2. Design

| | Raycast (Windows) | akscast | Phase 0 evidence |
|---|---|---|---|
| UI | WPF + WebView2 + React | **Rust + GPUI** (`gpui-ce` 0.3.0, Win32 + DirectX) | spike builds and runs |
| Core | Node + Rust napi + Swift | same Rust process as the UI, no IPC on the hot path | — |
| Show/hide | hide → re-show, recentre, DPI rescale | **one window, toggled with DWM cloak** (`DWMWA_CLOAK`), swap chain stays alive | hide: 46 ms p50 → cloak: **14.6 ms p50**, 16.7 p95 |
| Focus | UIAccess helper | `RegisterHotKey` + `SetForegroundWindow` | **20/20 foreground** |
| Modifier-only hotkeys (double-tap Win/Ctrl) | LL hook in UIAccess | `WH_KEYBOARD_LL` in-process (Phase 1b) | not built yet |
| Extensions | private runtime in a worker | **our own `@raycast/api` runtime** (react-reconciler → JSON tree) in a lazily started Node sidecar | unmodified store extension runs |
| Settings | encrypted SQLite | one `config.toml`, hot-reloaded | — |
| File search | USN via elevated helper | Everything SDK → USN (needs elevation) → `jwalk` | — |
| Calculator | SoulverCore | `fend-core` | — |
| Idle RAM | ~980 MB | **21 MB** measured, plus ~70 MB while the Node sidecar runs | measured |

Core patterns taken from [open-raycast](https://github.com/JingHao-Leon/open-raycast) (MIT, macOS, no extensions):
- generation counter, so stale keystroke results never display
- a sync `isRelevant(query)` gate per source before doing any work
- a push-only coordinator with a ~33 ms flush throttle
- a failing source shows an error row and never blanks the other sources
- a sensitive-content filter for clipboard history

---

## 3. Settings (yours, plain text)

`%APPDATA%\akscast\config.toml`:

```toml
hotkey = "Alt+Space"          # or "DoubleTap+Ctrl"; conflicts are reported, not fatal
monitor = "cursor"            # cursor | primary | 1 | 2
width = 750
max_results = 8
theme = "dark"

[search]
apps = true
files = { enabled = true, backend = "auto", roots = ["C:\\", "D:\\", "E:\\"], exclude = ["node_modules", ".git"] }
calculator = true
fallback = "https://www.google.com/search?q={query}"

[clipboard]
enabled = true
history = 500
ignore_apps = ["KeePassXC.exe", "1Password.exe"]

[[alias]]
keyword = "gh"
url = "https://github.com/search?q={query}"

[[hotkey_command]]
keys = "Shift+CapsLock"
command = "theme-scheduler/apply-themes"

[extensions]
node = "auto"
dir = "%APPDATA%\\akscast\\extensions"
memory_mb = 256
idle_kill_secs = 300
```

---

## 4. Phase 0 results

### 4.1 Extension compatibility
- **Built with `ray build -e dist`**, with the network blackholed via a dead proxy: exit 0 in 7.5 s, no login needed. Output is one CommonJS file per command.
- That output keeps exactly three modules external: **`@raycast/api`, `react`, `react/jsx-runtime`**. `@raycast/utils` (`useFetch`, `useCachedPromise`, …) is bundled into every extension, so we get it for free.
- **Ran unmodified `cdnjs` on `spikes/exthost`** (~200 lines: shim + worker + host). `Module._resolveFilename` redirects the three externals to our copies. Results:
  - first render at 102–568 ms. Worker start and module load vary with disk cache.
  - typing reaches the handler and fetches; 20 items rendered at ~830 ms (network-bound)
  - `unsupported=[]`
  - 11 element types hit: List, List.Item, List.Item.Detail, Metadata.*, ActionPanel, Action.CopyToClipboard/OpenInBrowser
- Gotcha found: `@raycast/utils` passes `cache.subscribe` **unbound** to `useSyncExternalStore`, so every API method must be an arrow or bound.
- Wire format is ours: `{type:"render", tree}` (full tree, ~30 KB for 20 items) and `{type:"event", id, args}`, with function props replaced by `{$fn:id}`. Diffing isn't needed yet.

### 4.2 Store reality (`raycast/extensions`, 3,505 extensions)

| `platforms` | Count |
|---|---|
| declares Windows | **995** |
| macOS only | 1,283 |
| no field (treated as macOS) | 1,227 |

Among the 995, 123 still contain AppleScript, `osascript` or `/Applications` paths.

Command modes in the Windows set: view 2,549, no-view 685, menu-bar 118.

**Coverage by the number of `@raycast/api` names implemented** (an extension counts once every name it imports exists):

| Top N names | Extensions fully covered |
|---|---|
| 10 | 7% |
| 20 | 27% |
| 30 | **65%** |
| 40 | **91%** |
| 58 (all) | 100% |

Implementation order (extension count): ActionPanel 929 · Action 927 · Icon 854 · List 806 · showToast 751 · Toast 742 · getPreferenceValues 680 · Color 505 · Detail 479 · Form 461 · Keyboard 377 · useNavigation 374 · Clipboard 348 · LocalStorage 328 · confirmAlert 305 · open 276 · environment 275 · Alert 268 · showHUD 260 · LaunchProps 233 · openExtensionPreferences 202 · Image 175 · LaunchType 168 · popToRoot 167 · launchCommand 144 · Cache 142 · closeMainWindow 142 · Grid 129 · MenuBarExtra 101 · getSelectedText 76 · OAuth 63 · openCommandPreferences 62 · Tool 58 · showInFinder 51 · AI 49 · …

`@raycast/utils` names worth testing first: useCachedPromise 286, showFailureToast 220, usePromise 185, useForm 176, useFetch 171, useCachedState 147.

### 4.3 GPUI window (`spikes/gpui-window`)
- `gpui-ce` 0.3.0 with `gpui_ce_platform` 0.3.0 builds on Rust 1.95 + MSVC 2022 + SDK 26100. The first release build takes ~10+ min; incremental builds take 7–15 s. The exe is 22.8 MB.
- API gotcha: the inherent `Window::window_handle()` shadows the raw-window-handle trait, so call `HasWindowHandle::window_handle(window)`.
- **Ctrl+Alt+Space was already taken on this machine.** `RegisterHotKey` failure must surface in the UI, not panic.

| Metric | Measured | Target |
|---|---|---|
| hotkey → UI thread | 0.3 ms p50 | — |
| hotkey → GPUI frame (cloak) | 14.6 ms p50, 16.7 ms p95 | < 16 ms ✓ |
| hotkey → composited pixel (external harness) | 24.7 ms p50, 33.6 max | resolution limited by `GetPixel` ~1 vsync |
| foreground acquired | 20/20 | — |
| idle private memory | 21 MB | < 40 MB ✓ |
| cold start → window created | 188–240 ms | < 150 ms ✗. Acceptable: start-at-login makes it rare. Revisit with a lazy font/atlas init |

### 4.4 Still open
- **Raycast hotkey baseline**: it ignores injected keys, so it needs your physical double-taps. Run:
  `python -I spikes/latency.py raycast.exe manual+win 10`
- **Elevated foreground windows** (UIPI): hotkey and focus behaviour is untested. Test by opening an admin terminal and pressing the hotkey.
- Navigation (`useNavigation` / `Action.Push`), `Form`, `Grid`, OAuth: not in the shim yet. These are Phase 3 items.
- Double-tap modifier hotkeys via `WH_KEYBOARD_LL`: not built.

---

## 5. Parity with Raycast for Windows

Source: the settings pages and feature modules in the installed v2.10.1 frontend (568 modules). "Same" means the same capability and keyboard flow. The visual design is akscast's own; Raycast's look, icons and branding are not copied.

| Raycast feature | akscast | When |
|---|---|---|
| Root search: apps, fuzzy match, frecency | ✅ | Phase 1 |
| App icons in results | ✅ | Phase 2a |
| Global hotkey, conflict error | ✅ (RegisterHotKey) | Phase 1 |
| Modifier-only hotkeys (double-tap Win), Hyper key | ❌ needs `WH_KEYBOARD_LL` | Phase 1b |
| Per-command hotkeys | ✅ main + clipboard hotkeys | Phase 2a. Arbitrary commands: Phase 3 |
| Aliases on commands | partial (quicklink keywords) | Phase 3 |
| Fallback commands (web search) | ✅ | Phase 1 |
| Calculator (units, math, currencies) | ✅ `fend` + daily ECB rates (cached, offline-safe) | Phase 1 / Phase 4 |
| Clipboard history (text, filters, sensitive-app skip, encrypted at rest) | ✅ text + DPAPI at rest | Phase 2a. Images/files: Phase 4 |
| Snippets: search + paste | ✅ | Phase 2a |
| Snippets: auto-expand while typing anywhere | ❌ needs LL keyboard hook | Phase 1b |
| Quicklinks with `{query}` argument | ✅ | Phase 2a |
| System actions (lock, sleep, restart, shut down, sign out, empty bin) | ✅ with confirm on destructive ones | Phase 2a |
| Window management (halves, thirds, maximize, centre, next display) | ✅ | Phase 2a |
| Emoji picker | ✅ (search + paste) | Phase 2a. Inline `:` autocomplete: Phase 1b |
| Windows Run | ✅ `>` prefix | Phase 2a |
| Action panel (Ctrl+K secondary actions) | ✅ (run as admin, reveal, copy path/name, delete clipboard entry) | Phase 2b |
| File search | ✅ own in-memory index of `[files] roots`, worker thread + generation counter | Phase 2b. Live index (USN/watcher): later |
| Script commands | ✅ Raycast headers, silent/compact/fullOutput, arguments, needsConfirmation | Phase 2b. Inline/refresh-interval: later |
| AI (Claude Code / Agent SDK) | research only: [docs/ai-research.md](docs/ai-research.md) | Phase 5 |
| Extensions (Raycast API, unmodified store extensions) | ✅ own runtime (`exthost/`), List / Grid / Detail / Form / ActionPanel / navigation / toasts / confirm / storage / prefs, interval background commands | Phase 3 |
| Extension install + store | ✅ "Install Extension" (name or folder) and "Browse Extension Store" (996 Windows-capable) | Phase 3 / Phase 4 |
| Menu-bar extras → tray | ✅ one tray icon per menu-bar command, live menu, interval refresh | Phase 4 |
| Settings window | ✅ sections, toggles, hotkey recorder, extension preferences; writes config.toml keeping comments | Phase 4 |
| Themes / theme studio | ✅ system/dark/light + per-colour overrides (no visual editor) | Phase 4 |
| Calendar, contacts, translator, dictionary, timer, focus sessions, screenshots search, typing practice, games, taskbar widget | ❌ | Phase 4+, most of these are good extension candidates |
| AI chat/commands/agents/MCP, dictation, notes, cloud sync, accounts, organizations | ❌ by design (paid cloud services) | bring-your-own-key AI could come later |

### Phase 2a status (2026-11-03)
Built: app icons, commands (system + window management), clipboard history, snippets, quicklinks, emoji, Windows Run, a footer showing what Enter does, and a second Enter for destructive commands.

Verified live by `spikes/e2e.py`, **9/9 pass**:
- hotkey shows + focuses the launcher
- the calculator copies its result
- focus returns to the previous window
- Left Half snaps the real window
- clipboard hotkey → search → paste
- emoji mode → paste
- `{date}` snippet → paste
- Escape goes mode → root → hidden
- the hotkey toggles the window closed

| Metric | Value |
|---|---|
| hotkey → on-screen pixel (external harness) | **20.0 ms p50**, 21.1 max |
| app scan: names searchable | 0.97 s |
| app scan: icons loaded (207/208) | 4.2 s |
| idle private memory | 41.5 MB (icons, emoji table and clipboard history in memory) |

Not run live on purpose:
- shut down, restart, sign out, sleep, lock, empty bin, dark mode (they change your session or system)
- quicklinks and Run (they open your browser or programs)

These are covered by unit tests up to the Win32 call.

### Phase 2b status (2026-11-03)

Built:
- **Ctrl+K action panel**: the selected item's actions replace the list; Esc closes it.
- **File search mode**: a `jwalk` crawl of `[files] roots` (default: your user folder minus AppData, node_modules, .git, …) into memory. A worker thread searches it, only the newest query's results are shown, and the index re-crawls when you open file search and it's more than 2 minutes old.
- **Script commands**: from `%APPDATA%\akscast\scripts`, with argument prompts and an output view.

Verified:
- **12/12 e2e checks pass** (`spikes/e2e.py`). New since Phase 2a: file search → Ctrl+K → Copy Path, a fullOutput script, a script with an argument.
- That run used the previous index layout. After the index was compacted, unit tests pass but the e2e suite has not been rerun.
- 6 unit tests pass.

Measured:
- Idle memory is 59.6 MB with ~140k indexed entries; it was 75–79 MB before the index stored each path once.
- The rest of the index cost scales with `[files] roots`.

Not run live: Run as Administrator (it shows a UAC prompt).

AI research for a later phase is in [docs/ai-research.md](docs/ai-research.md):
- **Recommendation:** run the Agent SDK (TypeScript) in the Node sidecar, with akscast's own tools exposed as an in-process MCP server and the built-in tools removed.
- **Licence:** personal use on your own Claude login is fine. Distributing the app needs per-user API keys.

### Phase 3 status (2026-11-03): Raycast extensions

**Approach: run the real store extensions; don't rewrite them.** All the extensions you actually use declare Windows support and run unmodified on akscast's own `@raycast/api` runtime. Rewriting them would cost weeks and buy nothing. Native rebuilds are only worth it for an extension that's macOS-only or needs an API akscast can't provide.

**Your extensions** (from 4 weeks of Raycast logs):

| Extension | Opens | Status in akscast |
|---|---|---|
| Theme Scheduler (your own, `D:\Pro\lightswtch`) | 29 opens + 126 background runs | ✅ installed from the folder; view works; `apply-themes` runs every 5 min |
| Obsidian | 9 | ✅ 13 commands; forms work (Quick Capture). Menu bar hidden |
| Urban Dictionary Search | 4 | ✅ live search |
| Port Manager | 2 | ✅ Open Ports lists real ports. Menu bar hidden |
| Agent Usage | 1 | ✅ view; refresh runs every 1 min |
| YouTube | 1 | ✅ renders; needs `[extension.youtube] apikey` |
| Mozaika | 1 | ✅ |

Not installed: toggl-track, remove-paywall, speedtest, skills and ~15 more. They were installed in Raycast but never run in 4 weeks; install any of them later with "Install Extension".

**How it works**
- `exthost/` (Node):
  - `host.mjs` runs one worker per command.
  - `runtime.js` is akscast's own `@raycast/api`.
  - `install.mjs` fetches an extension and runs `ray build`.
  - `smoke.mjs` is the regression gate.
- **Wire format:** newline-delimited JSON over stdio. Every Raycast action kind becomes an `onAction` handler, and handler IDs come from tree position.
- **Rust side** (`src/ext.rs` + `main.rs`):
  - manifest scan, preference values from `[extension.<name>]`
  - view model for List / Grid / Detail / Form
  - Ctrl+K actions, navigation pop, confirm dialogs
  - interval scheduler (checked every 10 s)
- **Install:** "Install Extension", then a store name or a local folder.
  - Uses one shared sparse clone of `raycast/extensions`.
  - `npm ci --ignore-scripts`, so no third-party install scripts run.
  - Local folders are built from a copy, so your repo is never touched.

**Verified**
- `smoke.mjs`: 22/22 view commands of the 7 installed extensions render, with no unsupported APIs.
- `spikes/e2e.py`: **17/17**. New this phase:
  - Urban Dictionary live search for "yeet", with the query intact
  - Ctrl+K → the extension's own Copy action
  - Port Manager listing real ports
  - your Theme Schedules list
  - Obsidian Quick Capture form, with Tab moving between fields (nothing submitted)
- Background: `agent-usage/refresh-agent-usage` and `theme-scheduler/apply-themes` ran on schedule with no errors (`%APPDATA%\akscast\extensions.log`).

Bugs found and fixed while testing:
- Text typed before an extension's first render was never sent to it.
- A late echo of the extension's own controlled `searchText` overwrote newer keystrokes.

**Memory:** akscast 65.7 MB; the Node extension host 90.4 MB while extensions run. It only starts when an extension runs, but background interval commands keep it alive.

**Known gaps (later)**
- Menu-bar commands (tray menus)
- Network images (thumbnails, favicons) show a placeholder
- Password fields show text in clear
- TextArea is single-line
- A view ends when the window hides (Raycast keeps it about 90 s)
- No store browser (install by name)
- OAuth, AI, `getSelectedText`, BrowserExtension throw "not supported"
- Theme Scheduler now runs from both Raycast and akscast. It's idempotent, but turn it off in Raycast once you switch.

### Phase 4 status (2026-11-03): polish

Built:
- **Tray menus for menu-bar commands.** Obsidian, Port Manager and Agent Usage each get a tray icon whose menu is their live `MenuBarExtra` tree.
  - Clicks call the extension's handler.
  - Commands with an interval re-run fresh and keep their icon.
- **Settings window** ("akscast Settings", or tray → Settings…).
  - Sections: General, Search, Clipboard, Files, Theme, Extensions, Advanced.
  - Toggles, choices and text, number and list fields, plus a hotkey recorder.
  - Every installed extension's preferences are editable (required ones marked).
  - Edits go into `config.toml` via `toml_edit`, so comments and layout are kept; the launcher reloads within 2 s.
- **Themes:** `[theme] mode = system | dark | light` plus per-colour hex overrides. Used by both windows.
- **Store browser** ("Browse Extension Store"):
  - lists the 996 Windows-capable extensions from the shared sparse clone, which refreshes daily
  - Enter installs; Ctrl+K opens the store page or the GitHub source
- **Calculator currencies:** ECB reference rates, cached in `rates.xml` and refreshed daily (`10 eur to usd`, `500 inr in usd`).
- **Web images for extensions** (thumbnails, favicons): downloaded once into `image-cache`, max 5 MB each, 10 s timeout.

Verified:
- `spikes/e2e.py` **25/25**. New this phase:
  - 3 tray icons
  - the store lists 996 and finds Obsidian
  - live currency conversion
  - web thumbnails cached
  - the settings window opens
  - a number field saves with comments kept
  - the hotkey recorder saves Ctrl+Alt+E
  - switching to light restyles the launcher

  `config.toml` was byte-identical after the run.
- `smoke.mjs` 23/23; 7 unit tests.
- Hotkey → on-screen in 17.2 ms p50.

Not done:
- **Auto-update:** there are no published releases to update from. Add it once the repo has GitHub releases.
- **Clipboard images and files:** text only for now.
- **No visual theme editor:** colours are hex fields in Settings.
- **Tray clicks** haven't been exercised by the automated test; menu building is unit-tested.
- **Extension host memory: 195 MB** while 3 menu-bar commands stay loaded, so tray clicks can reach a live worker. `[extensions] menu_bar = false` frees that memory.

## 6. Build plan

One Rust crate (`src/`) plus `exthost/` (grown from `spikes/exthost`).

### Phase 1: launcher core: **built 2026-11-03** (`src/`, ~900 lines)

Done:
- GPUI window with DWM cloak toggle, out of taskbar and Alt+Tab
- `RegisterHotKey` (errors open the window with a notice)
- shell:AppsFolder scan (208 apps, 1.7 s, off-thread)
- `nucleo` + frecency, `fend` calculator (50 ms time box)
- aliases, web fallback
- `config.toml` with 2 s reload
- tray (Open / Edit config / Quit), start at login, single instance

Measured: 23 MB private memory idle. Two self-check tests (`cargo test`). The end-to-end check `spikes/e2e.py` passed show, type and copy-result; its focus-return checks were inconclusive with the machine in use.

Deviations from the original list:
- **no async coordinator**, because every source is sync and sub-ms (added when files or extensions arrive)
- **frecency in JSON**, not SQLite
- **no app icons yet**
- mtime poll instead of a file watcher
- no app-list disk cache

Lesson: any Win32 call that synchronously sends messages to our window (`SetWindowPos`) must run between GPUI updates. GPUI's window proc re-borrows the app and panics with `RefCell already borrowed`.

Original Phase 1 scope:
- Window: GPUI, one window, cloak toggle, per-monitor placement on `WM_DISPLAYCHANGE` / `WM_DPICHANGED` only.
- Hotkey: `RegisterHotKey`, with conflict reported in the UI.
- Coordinator with a generation counter, `isRelevant` gates, push + 33 ms throttle.
- App index: Start Menu `.lnk` + `shell:AppsFolder`, cached in SQLite (`rusqlite`).
- `nucleo` fuzzy matching, frecency `Σ exp(-age_days/14)`, `fend-core` calculator, web fallback, aliases.
- `config.toml` + hot reload, tray icon, start at login.

### Phase 1b: modifier-only hotkeys
`WH_KEYBOARD_LL` for double-tap Win/Ctrl. Document that it doesn't fire while an elevated window has focus, unless we later ship a signed UIAccess helper.

### Phase 2: built-ins (2 weeks)
- File search (Everything SDK → elevated USN helper → `jwalk`).
- Clipboard history (`AddClipboardFormatListener`, sensitive filter, `ignore_apps`).
- Snippets, quicklinks, Raycast script commands (`@raycast.*` headers), window snapping.

### Phase 3: extension host (3–4 weeks). Order comes from the §4.2 coverage data
1. Grow the shim to the top 30 names (65% of Windows extensions): navigation stack, `Form` + `useForm`, `Keyboard` shortcuts, `confirmAlert`/`Alert`, `Image`, `Color`, preferences UI, `launchCommand`, `openExtensionPreferences`.
2. Top 40 (91%): `Grid`, `MenuBarExtra` (tray), `getSelectedText`, `OAuth` (PKCE + loopback), `showInFinder` → Explorer, `getApplications`.
3. Rust renderer for the JSON tree, with function props → event messages.
4. Persist `LocalStorage` and `Cache` to disk per extension, plus `supportPath`.
5. Install = sparse-clone the extension from `raycast/extensions`, run `npm ci --ignore-scripts && ray build -e dist`, then read the manifest. Show only `platforms` ∋ Windows by default.
6. Run every Windows extension headless through the host as a smoke test, and record which API names hit `unsupported`. That becomes the regression gate.

### Phase 4: polish
Settings UI (writes `config.toml`), themes, extension browser, auto-update.

### Skipped on purpose
AI chat, cloud sync, notes, org features, browser companion extension.
