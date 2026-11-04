# akscast design critique and requirements

Reviewed on 2026-11-04 from screenshots of the release build (dark theme, compact window mode,
`width = 0.49` on a 1920 px screen, so the launcher is drawn at about 1.25×). Screens: empty root,
root search for `cl`, calculator, clipboard history, emoji search, Quit Applications, and every
settings page.

## Ratings

| Area | Score | In one line |
|---|---|---|
| **Usability** | **6 / 10** | Fast and keyboard-first, but the top result is often wrong for short queries and the list cuts rows in half |
| **Interface** | **7 / 10** | Calm, native Windows 11 look; let down by low-contrast hints, repeated labels and sizes that don't add up |
| **UX** | **6 / 10** | Strong feature set; weak feedback (green for every status, red for every settings message) and few previews |
| **Consistency** | **5 / 10** | Two sets of size constants, an ad-hoc type scale, and the design doc no longer matches the code |
| **Accessibility** | **5 / 10** | Placeholder and section-header text fail contrast; no visible focus ring in the launcher list |

## Design critique: akscast launcher and settings

### Overall impression
The launcher reads as a Windows 11 tool: acrylic, Segoe UI Variable, a violet selection pill, and
key caps in the action bar. The biggest opportunity is the list itself. Short queries put the
wrong item first, and the window height doesn't fit whole rows, so the last row is always cut.

### Usability

| Finding | Severity | Recommendation |
|---|---|---|
| Typing `cl` ranks **Search Files** first; Clipboard History isn't on screen. Scattered letters inside a word ("sear**c**h fi**l**es") beat word starts ("**Cl**ipboard", "**Cl**ock") | 🔴 Critical | Score word-start and acronym matches far above scattered ones; an exact alias, prefix or acronym always wins |
| The last row is cut in half in every list (root, clipboard, emoji). The window height uses 44 px rows and a 34 px footer; rows are drawn at 40 and the footer at 40 | 🔴 Critical | One source for row, bar and footer sizes; window height = whole rows |
| The calculator shows `approx. 3.1068559612 miles` | 🟡 Moderate | Round to a sensible precision and show a large result card: `5 km` → `3.11 mi`, Enter copies |
| Quit Applications titles rows by window title ("Practice - Google Chrome") | 🟡 Moderate | Title = app name, subtitle = window title, accessory = window count |
| Clipboard rows all use the same document icon | 🟡 Moderate | Icon by content type: link, email, colour (with swatch), path, code, plain text |
| Every status in the action bar is a green dot, including neutral messages ("Hidden from root search…"); the colour is guessed from the words | 🟡 Moderate | Explicit tone per message: info (neutral), success, error |
| In Settings, every message that doesn't start with "Saved" is shown in red | 🟡 Moderate | Same explicit tones in Settings |
| The empty root in compact mode is a bare bar with nothing to discover | 🟢 Minor | Show the favourites strip, or a faint hint of the hotkeys (`↓ recent · Ctrl K actions`) |
| Long choice options wrap onto two lines in Settings ("Show akscast on", "Keep the last search") | 🟢 Minor | Short option labels; the description explains them |

### Visual hierarchy
- **What draws the eye first:** the selected row (pill plus fill). Correct.
- **Reading flow:** search text → selection → action bar. Good, but the left of the action bar repeats "akscast" with a search icon, which competes with the search bar and says nothing.
- **Emphasis:** "akscast" as the subtitle of every built-in command, and "Command" or "Application" on every row, add noise without telling rows apart.

### Consistency

| Element | Issue | Recommendation |
|---|---|---|
| Sizes | `main.rs` has its own `ROW = 44` and `FOOTER = 34`; `design.rs` has `ROW = 40` and `FOOTER = 40`. The window is sized with one set and drawn with the other | Delete the copies in `main.rs`; use `design::*` everywhere |
| Type scale | Sizes in use: 10, 11, 11.5, 12, 12.5, 13, 14, 15, 17 and 22 | Six steps: caption 11, small 12, body-sm 13, body 14, title 17, display 22 |
| Selection | Launcher: `surface` fill plus pill. Settings sidebar: `surface_strong` plus pill. Settings rows: `surface` plus pill. Shortcuts: icon tile turns `surface_strong` | One selected style: `surface_strong` fill plus the accent pill, everywhere |
| Window material | Launcher uses Blurred (acrylic), Settings uses Mica | Keep both (launcher transient, settings long-lived), and record it in the doc |
| Design doc | DESIGN.md says window 760 wide (code: 750), key cap 18 tall (code: 20), icon glyph 16 (code: 15), footer icon 16 (code: 14) | Update the doc to the code after this pass |
| Subtitles | Built-ins say "akscast"; extension commands show the extension name; apps show nothing | Built-ins show their group ("Window", "System", "Clipboard"), extensions their extension, apps nothing |
| Back affordance | Sub-views show a Back tile; Settings has no back, just sections | Fine; documented as is |

### Accessibility
- **Colour contrast:**
  - `text_faint` `#6E6E7A` on the dark body (about `#1F1F23`) is about **3.4:1**. It fails AA for the 12.5 px section headers and the placeholder.
  - `text_muted` `#A0A0AB` is about **7:1** and passes.
  - The accent `#7C6CFF` against the body is about **4.0:1**. That's fine for the 3 px pill and icons, not for text.
- **Focus:** the selected row is the only focus indicator in the launcher, and it's subtle on light themes. Settings inputs have an accent border; the others have none.
- **Targets:** settings chips' × and ← are about 14 px wide, too small for comfortable mouse use (aim for 24 px).
- **Readability:** 14 px body text and 17 px search text are good. Settings descriptions at 12 px on 17 px line height are fine.

### What works well
- Clear three-band structure (search, list, action bar), with real key caps for the main and Ctrl+K actions.
- Native materials and fonts; it feels like part of Windows rather than a web page.
- Section headers with counts ("Today 48") and right-aligned accessories scan well.
- The Settings rework (cards, wrapping descriptions, chips) is a big step up from the stretched rows.

## Memory and UI trade-offs (measured)

| Process | Memory | What it is |
|---|---|---|
| akscast | 85 MB working set, 67 MB private | UI, catalog, app icons, file index client (whole-drive index after filters) |
| akscast-indexer | 55 MB | Elevated NTFS helper |
| akscast's node host | 73 MB | Runs Raycast extensions; only while extensions are in use |
| *Raycast for comparison* | *278 MB for its node backend alone* | |

| UI choice | RAM cost | Decision |
|---|---|---|
| App icons cached at 48×48 RGBA (208 apps) | 1.8 MB | Keep: sharp at 200% scaling |
| Running-apps icons cached by program | under 0.5 MB | Keep |
| Extension images cache (`images`) has no limit | grows with use | **Cap it** (least recently used, about 200 images) |
| Acrylic and Mica backdrops | GPU only, no RAM | Keep |
| A visual refresh (larger result cards, type colours, swatches) | none: drawn from tokens, no bitmaps | Do it |
| Animations (fades, slide-in) | small CPU and GPU, frames only while moving | Only the window fade-in; no list animations |
| Clipboard images (G2, later) | could be large | Store originals on disk, keep 64 px thumbnails in memory |
| Emoji grid (I1, later) | glyph atlas grows with emoji shown | Fine at ~10 MB worst case; render only visible rows |

## Requirements, in order

Status after the 2026-11-04 pass: ✅ done · ◐ partly · ☐ not yet.

**P0: broken or misleading. Fix first.**
1. ✅ **R1 Ranking.** Exact alias, then exact title, then title prefix, then word-start, then initials, then scattered matches. `cl` now lists Clock, Claude and Clipboard History before Search Files (tested).
2. ✅ **R2 Whole rows.** One set of size tokens in `design.rs`; window height = border + bar + rows (results plus one header) + padding + action bar.
3. ✅ **R3 Contrast.** `text_faint` raised in all seven themes (dark `#8B8B97`, light `#6B6B77`), about 4.7:1 or better.
4. ✅ **R4 Honest feedback.** Messages carry a tone (info, success, error) in the launcher and in Settings; extension toasts pass their own style.

**P1: inconsistent or noisy.**
5. ◐ **R5 Type scale.** Six named sizes in `design::ts`; Settings and the main launcher text use them. A few small labels in extension views still use their own sizes.
6. ✅ **R6 One selected style.** `surface_strong` plus pill for launcher rows, the action panel, settings rows and the sidebar; a separate, lighter `hover` token.
7. ✅ **R7 Meaningful subtitles.** Built-ins show Window, System, Clipboard, Files, Emoji, Apps or akscast.
8. ✅ **R8 Calculator card.** Two rows tall; the answer at most four decimals in display type, the full value under it; Enter copies the full value.
9. ✅ **R9 Running apps.** The program's own name as the title, the window title (without the app suffix) as the subtitle.
10. ✅ **R10 Clipboard types.** Link, email, colour, path, code and text icons and subtitles.
11. ✅ **R11 Settings choices.** Short labels with the meaning in the description; "Keep the last search" offers seven presets.
12. ✅ **R12 Hit targets.** Chip × and ← are 22 px.

**P2: polish and memory.**
13. ☐ **R13 Root hint.** Not done: the compact empty bar is still bare.
14. ✅ **R14 Action bar.** In root search the left side names the selected row's kind.
15. ✅ **R15 Image cache cap.** Cleared past 200 images.
16. ✅ **R16 Design doc.** DESIGN.md updated: tokens, type scale, kind tints, sizes, states.

**The distinctive element: kind tints.** Each row's icon tile takes the hue of what it is (window teal, system amber, paste blue, web green, files sand, calculator pink, destructive red). It's the one bold choice; the rest stays neutral, and the accent is kept for selection and focus. It costs no memory: the tints are token colours, not images.
