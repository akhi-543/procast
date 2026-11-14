# Roadmap J (file search) and K (window management): design

2026-11-14 · rows J1–J4 and K1–K6 of [docs/roadmap.md](../../roadmap.md)

## Intent

Close the gap with Raycast for Windows 2.12 in two areas procast already has a base for:
file search (folder index in `src/files.rs`, whole-drive NTFS index in `src/ntfs.rs`) and
window snapping (`Snap` in `src/commands.rs`). Success = each roadmap row can be ticked ✅
with a test or a manual check behind it, and the build stays clean (zero warnings, zero
clippy, tests green).

## Goals, non-goals, anti-goals

**Goals:** J1–J4 and K1–K6 as scoped below.

**Non-goals:**
- `.gitignore` in the whole-drive (NTFS) index. Reading every `.gitignore` on three drives
  is a project of its own; the folder index gets it.
- Network drives in file search. The NTFS index can't read them and the folder crawl
  would be slow; only removable NTFS drives are added.
- A settings UI for layout presets (K6). They live in `config.toml`, like quicklinks did
  before H4.
- Windows 10 and Windows 11 before 23H2 for virtual desktops.

**Anti-goals (a failure even if it works):**
- procast crashes or hangs because Explorer's undocumented desktop interfaces changed.
  Every virtual-desktop call must fail soft.
- A new look. J3 and J4 reuse existing pieces (clipboard's detail pane, the Information
  rows, Settings rows); no new visual components without the Design System artifact first.
- Search getting slower. Hidden-file and glob checks run at index time, not per keystroke.

## Constraints

- Two file indexes: the folder crawl (`files.rs`, with a ReadDirectoryChangesW watcher
  and `apply`) and the NTFS helper (`ntfs.rs`, runs elevated, filtered by `ntfs::Filter`).
  On this machine the NTFS index is the one in use, so every J change must work there.
- The NTFS helper is a separate elevated process; filter changes reach it through
  `Filter` over the pipe (as `exclude`, `skip` and `drives` do today).
- `windows` crate is 0.62. winvd brings 0.58 alongside it.
- Memory rule: design work goes to the Design System artifact first. J3/J4 are treated
  as reuse, not design; if they need anything new, stop and mock it there.

## Approaches

**K5 virtual desktops** (the only real fork; researched in
[How Raycast for Windows manages virtual desktops](https://claude.ai/artifact/NsaWoTgt4W8bzGrwwMqULL)):

| Approach | Gives | Costs |
| --- | --- | --- |
| Public `IVirtualDesktopManager` only | Move a window to another desktop | No switch, create, close, rename; no auto-close |
| **winvd crate, `multiple-windows-versions`** (chosen) | Everything Raycast has | Undocumented API; second `windows` crate; can break on a Windows update |
| Own COM bindings to the internal interfaces | Same as winvd | Re-doing winvd's 13 build tables by hand |

Chosen by the user: winvd, as Raycast does.

**J1 ignore rules:** globs in both indexes (cheap, same matcher) and `.gitignore` in the
folder index only (my call; see non-goals).

## Design

### J1 — ignore patterns

- `files.exclude` entries may contain `*` and `?`. Matched case-insensitively against a
  single name (file or folder), like plain entries today. A plain entry still means an
  exact name. One small `glob_match(pattern, name)` in `files.rs`, used by `crawl`, `apply`
  and `ntfs::Index::left_out`.
- New `files.gitignore = true`. The folder crawl honours `.gitignore` and `.ignore` files
  found under each root, including ones added later (watcher `apply` re-reads a changed
  ignore file by re-crawling its folder). Use the `ignore` crate's `gitignore` matcher for
  the semantics rather than a hand parser.
- Cache key (`cache_key`) includes the new settings so a changed rule set re-crawls.

### J2 — hidden files and removable drives

- New `files.hidden = true` (true = include, today's behaviour). Folder crawl: skip
  entries with `FILE_ATTRIBUTE_HIDDEN`. NTFS: read the attribute from the MFT record
  (`$STANDARD_INFORMATION` file attributes) into a new `HIDDEN` node flag; `left_out`
  honours it when the setting is off. Pass the setting through `Filter`.
- New `files.removable = false`. When on, the NTFS helper also indexes NTFS volumes with
  `GetDriveTypeW == DRIVE_REMOVABLE` (today only `DRIVE_FIXED`). Drives plugged in later
  are picked up at the next helper rescan; live hot-plug is out of scope.
- Settings → Files gets two toggles: "Include hidden files", "Include removable drives".

### J3 — detail pane

- File Search mode shows a split view like Clipboard History: list left, detail right.
  Root-search file rows don't get it.
- Detail = preview, then Information rows (reuse the helper at `main.rs` "Information"
  rows): Kind, Size, Modified, Created, Where (folder, with `~`).
- Preview: images (the formats `image` already decodes) as a fitted thumbnail, decoded
  off the UI thread and cached like clipboard images; text-like files under 256 KB show
  their first 40 lines in mono; folders show their item count; anything else shows its
  shell icon (`win::file_icon`).
- Metadata is read when the selection changes (`std::fs::metadata`); a missing file shows
  "Not found" instead of rows.

### J4 — index status and rebuild

- Settings → Files, top group "Index": one status row, e.g.
  "2,712,340 entries · whole drives C:, D:, E: · updated 10:42" or
  "138,617 entries · folders · crawled 10:42", or "Indexing…".
- Last-crawl time becomes wall-clock (`SystemTime`) for both indexes.
- "Rebuild index" button: folder index → forced crawl; NTFS → ask the helper for a full
  rescan (new pipe request if none exists).

### K1 — more layouts

New `Snap` variants, all in `layout()` with unit tests like the existing ones:
- Quarters: Top Left, Top Right, Bottom Left, Bottom Right.
- Sixths: Top Left / Top Center / Top Right Sixth, Bottom Left / Bottom Center / Bottom Right Sixth.
- Top Third, Middle Third, Bottom Third (horizontal bands).
- Reasonable Size: centred, 60 % of the work-area width × 70 % of its height (my call;
  Raycast's exact numbers weren't checked).

### K2 — gaps

- New `[window]` config section: `gap = 0`, `screen_gap = 0` (logical px, scaled by the
  monitor's DPI).
- `screen_gap` shrinks the work area on every side. `gap` insets each edge of the target
  rect that is not on the work-area edge by `gap / 2`, so two neighbours end `gap` apart.
- Maximize stays the OS maximize (no gaps). Almost Maximize, Center and Reasonable Size
  use the work area minus `screen_gap`.
- Settings gets a new "Window" section (in `SECTIONS`) with the two numbers and the K3/K5
  toggles.

### K3 — cycling

- New `window.cycle = true`. Stateless: if the window's visible rect already equals the
  target of the command (±2 px), use the next size in the cycle instead.
- Cycles: Left/Right Half ½ → ⅔ → ⅓ → ½ (width, same edge). Top/Bottom Half the same in
  height. Other commands don't cycle.

### K4 — window actions

- Minimize (target window).
- Minimize Others: every other visible app window (reuse `apps.rs` enumeration).
- Close Others: polite `WM_CLOSE` to every other visible app window; `destructive()` so it
  asks for a second Enter.
- Toggle Maximize (Maximize ↔ Restore). Windows has no app-agnostic full screen, so the
  roadmap's "toggle full screen" becomes this (my call).

### K5 — virtual desktops

- Dependency: `winvd = { git = "https://github.com/Ciantic/VirtualDesktopAccessor", rev = "ef74c24", features = ["multiple-windows-versions"] }`
  plus whatever feature its event listener needs.
- Gate: build ≥ 22631 (23H2). Below that the commands aren't registered, so they never
  show in search.
- Commands: Open Desktop 1–10, Move to Desktop 1–10, Move to Next Desktop, Move to
  Previous Desktop, Close Desktop, Rename Desktop.
  - Open Desktop N or Move to Desktop N past the last desktop: toast "There is no desktop N".
  - Move to Next Desktop on the last desktop creates a new one and moves the window there;
    Previous on the first does nothing. Moving does not switch (my calls).
  - Rename asks for the name through the launcher's argument prompt (as snippets do).
  - Close Desktop moves its windows to the desktop on the left (winvd's fallback).
- `window.auto_close_empty_desktops = false`. A listener thread receives desktop-change
  events; after you switch away from a desktop, wait 500 ms, then if it still exists and
  has no visible app windows, remove it with the new current desktop as fallback.
- Every winvd call goes through one wrapper that turns errors (and panics, if winvd can
  panic) into a logged message and a toast. winvd keeps COM objects per thread: make all
  calls from one thread.

### K6 — layout presets

```toml
[[window.layouts]]
name = "Coding"
slots = [
  { app = "Code.exe", snap = "FirstTwoThirds" },
  { app = "WindowsTerminal.exe", snap = "LastThird" },
]
```

- Each layout becomes a root-search command "Layout: Coding" (hotkey and alias work through
  the existing D2/D3 key).
- Running it: per slot, find a visible window of that program (by exe name, case-insensitive,
  as F1's focus-existing does); if none, launch it and wait up to 5 s for a window; then snap.
- `snap` names are the `Snap` variant names. An optional `display = 2` puts it on that monitor.

## Left to the builder

- Exact attribute plumbing for `HIDDEN` in `ntfs.rs`, and whether the helper needs a new
  pipe message for rebuild.
- Whether `ignore` is used only as a matcher alongside jwalk or replaces jwalk in `crawl`.
- winvd listener details (feature flag, thread, message loop).
- Icons for the new commands, from existing glyphs.
- Order of work. Suggested: K1–K4 (small, all in `commands.rs`), then J1–J2, J4, J3, K6, K5.
