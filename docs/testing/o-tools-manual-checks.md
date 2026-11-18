# procast `feat/o-tools`: what's built and what to check

Branch `feat/o-tools`, head `c2697f6`. It has 68 commits on top of `main`.

The **built-in tools (O1–O8)** come first in the branch: Timers, Translator, Define, Focus, Calendar, Screenshots, HDR and the Notes store. The **feature-screens redesign** (21 tasks) then rebuilt every one of them to the new design system.

**What has been checked so far:**
- 383 unit tests pass. Clippy and the build have zero warnings.
- Every task had a code review, and the whole branch had a final review.
- **Almost nothing has been run in the real app.** Agents stopped launching procast because the launches took your foreground.

Each box below is something to check by hand. A ✅ means an agent already saw it work once in a dev instance; check it again anyway.

---

## 0. Setup

```powershell
# back up your data (config, notes, pins, timers)
Copy-Item -Recurse "$env:APPDATA\procast" "$env:APPDATA\procast-backup-$(Get-Date -f yyyyMMdd)"

cd D:\Pro\akscast\.claude\worktrees\o-tools
cargo build --release
# quit your normal procast from the tray, then:
.\target\release\procast.exe
```

**Browser extension:**
1. Open `chrome://extensions` and turn on Developer mode.
2. Remove the old "procast" extension.
3. Click **Load unpacked** and pick `D:\Pro\akscast\.claude\worktrees\o-tools\browser`.
4. Reload the extension once, after procast is running.

**When you're done:** quit procast from the tray and start your normal build again. To roll back your data, copy the backup folder back.

**Reporting a problem:** give the section number and the box text, then what happened. For example: "12.3 Quick Look stays blank after arrowing".

---

## 1. Startup and the launcher

**Built:**
- The launcher window is created cloaked and without focus, so startup should never flash a frame or take focus.
- New icons and design tokens throughout: sticker icon tiles, grey selection, no purple.

- [ ] Start procast while typing in another app. The typing is not captured, and that app keeps focus.
- [ ] No empty frame or shadow flashes in the middle of the screen at startup.
- [ ] No taskbar button appears.
- [ ] The first Alt+Space shows the launcher with acrylic, at the usual position and size.
- [ ] Root list icons match the design system: sticker tiles, grey selection, no purple anywhere.
- [ ] Light theme too: set `mode = "light"` in config.toml and glance over the screens below.

## 2. Dropdown menu (shared by most screens)

**Built:** one popover menu with a search box, sections and divider lines. It is used by argument choices, Translator, Focus, the Screenshots filter, Settings pickers and Notes.

- [ ] ✅ Clicking a dropdown trigger opens the menu under it.
- [ ] Up and Down move the selection.
- [ ] Typing filters the menu, and the launcher query stays unchanged.
- [ ] ✅ Enter picks an item. Esc closes only the menu, not the launcher. A click outside the menu closes it.
- [ ] Ctrl+P opens the screen's dropdown: Translator language set, Screenshots filter, Notes browse.

## 3. Inline argument boxes

**Built:**
- Commands that need input now show boxes inside the search bar, replacing the old one-question-per-screen flow.
- This covers timers, translate, scripts, snippets and Raycast extension arguments.

- [ ] ✅ Type `timer 25m tea`. Start Timer comes first with the boxes filled, and the footer reads "Next Box Tab · Start ↵".
- [ ] Type `timer`, a space, then Enter with the box empty. The footer reads "Type a time: 5m, 1h30m, 1:30".
- [ ] ✅ Type `tr hola`, then Tab, then Down. The language menu opens; type "ger" and press Enter, and To becomes German.
- [ ] ✅ The first Esc returns from a box to the query. The second Esc clears the boxes.
- [ ] Run an extension or script command that has arguments. Its boxes show, and Enter runs it.
- [ ] Hotkeys still work while a box has focus.

## 4. Toasts

**Built:**
- Three toast kinds: Done (closes after about 2 s), Error (stays until Esc or a click) and Info.
- Toasts never take focus, and clicks only land on the pill itself.

- [ ] ✅ Start Stopwatch shows "Stopwatch started", which closes after about 2 s.
- [ ] ✅ Trigger an error (for example `open desktop 10`). The error toast stays, a click closes it, and your typing never moves to it.
- [ ] **Press Esc while an error toast shows. It closes.** (This has never been run live.)
- [ ] Esc with Ctrl, Alt or Shift held is not swallowed by the toast.
- [ ] Open the launcher while an error toast shows. The launcher's Esc closes the launcher, not the toast.
- [ ] The toast has the right size at all three interface sizes.

## 5. Timers and Stopwatch

**Built:**
- A Running section at the top of the root list.
- Rows tick in place with fixed-width digits.
- Enter pauses or resumes a timer.
- Ctrl+K offers Restart, Add 5 min and Rename.
- A "done" toast appears, and timers are kept across restarts.

- [ ] Start two timers and one stopwatch. With an empty query, Running sits at the top. The order there is Focus, then Running timers, then Next meeting.
- [ ] "0:59 left" ticks down without the row width jumping.
- [ ] Enter on a timer shows "Paused · 0:5x". Enter again resumes it.
- [ ] Ctrl+K on a timer: Restart, Add 5 min and Rename all work.
- [ ] When a timer ends, the toast "tea · 1 min done" stays until you close it.
- [ ] Select row 4 in an empty root search and let a timer end. The selection stays on row 4.
- [ ] A bare `t` query does not jump to a timer. Ordinary searches never select a timer.
- [ ] Restart procast. Running and paused timers come back.

## 6. Translator

**Built:**
- A split view with language tags.
- Keyless Google Translate, which replaced DeepL.
- Instant Translate Copy and Instant Translate Paste.
- Language sets, which you can switch with Ctrl+P or add with a form.
- A Settings → Translator section.

- [ ] `tr hello world` shows the split view: source on the left, translation on the right, language tags, and the detected language.
- [ ] Enter copies the translation.
- [ ] Select text in another app and run Instant Translate Copy. The translation goes to the clipboard and a toast shows.
- [ ] Instant Translate Paste replaces the selected text with its translation.
- [ ] Ctrl+P opens the language-set menu. Picking a set changes the target.
- [ ] Add Language Set opens a form. Saving it adds a `[[language_set]]` to config.toml.
- [ ] Settings → Translator shows the sets. Primary language opens a dropdown you can filter.
- [ ] A misspelled word shows the "Did you mean" or corrected suggestion.
- [ ] With the network off, the error reads "Could not reach Google Translate", and the app stays usable.

## 7. Forms (Focus, Add Calendar, Add Language Set)

**Built:** one shared form layout for native features: a label column, Tab between fields, and selects, tokens and colour swatches as field types.

- [ ] Tab and Shift+Tab move between fields, and the first field has focus when the form opens.
- [ ] Ctrl+K, and the confirm question, keep the form's typed values.
- [ ] Ctrl+Enter submits.

## 8. Settings window

**Built:**
- A new row layout with an icon tile, the label, help text and the control.
- Separate sections for Focus, Calendar, Games, Translator, Screenshots and Notes.
- Section headers use the single accent-colour tile.

- [ ] Every section in the sidebar opens and scrolls. The sidebar reaches Advanced.
- [ ] Rows are 56 px with icons, and section headers show the accent tile.
- [ ] Advanced shows "Foreground watcher: running".
- [ ] Changing any setting writes to config.toml, keeping its comments, and takes effect without a restart.

## 9. Define

**Built:**
- An offline dictionary using WordNet, with a split view and "Did you mean".
- A Download Dictionary empty state instead of a silent download.
- Define Similar Word in Ctrl+K.

- [ ] With no dictionary installed, `define word` shows the Download Dictionary empty view. Enter downloads it (about 16 MB).
- [ ] `define serendipity` shows the split view with "Noun 1" and the gloss.
- [ ] `define serendipty` shows "Did you mean serendipity", and the bar keeps your typo.
- [ ] `define xyzq` shows the empty state "No definition for 'xyzq'".
- [ ] In the root list, `define serendipity` shows the first sense as its subtitle.
- [ ] Ctrl+K offers Define Similar Word, which opens a menu.
- [ ] `timer` shows no Define fallback row. Define was removed from the default fallbacks; an old config.toml may still list it.

## 10. HDR and Auto HDR

**Built:**
- The Toggle HDR row shows its status.
- An Auto HDR section in the root list with an on/off switch.
- An "HDR on for <game>" toast.
- A Settings → Games section.

- [ ] Type `hdr`. Toggle HDR shows "No HDR display", or "On/Off · <monitor>" if you have an HDR monitor. HDR Settings reads "Windows Settings".
- [ ] The Auto HDR row shows a switch. Enter flips it and saves `games.auto_hdr`.
- [ ] With no HDR display, Enter on Toggle HDR shows the sticky error toast.
- [ ] With an HDR monitor: start a game, and HDR turns on with the toast "HDR on for <game>". Quit the game, and HDR turns back off.
- [ ] Settings → Games shows the new header and rows.

## 11. Focus sessions

**Built:**
- Block or Allow modes for apps and websites.
- A Focus form: title, duration, mode, and apps and sites as tokens.
- A running row in the root list.
- A pill at the top centre of the screen.
- An end-of-session notice with Extend and Complete.
- A blocked page in the browser with Snooze.
- A Settings → Focus section, including Import Focus Categories.

**Form and running row:**
- [ ] Start Focus Session opens the form. Duration, Mode and Block all open dropdowns.
- [ ] Type `steam.exe` in the Block menu. It is added as an app, not a website.
- [ ] Ctrl+Enter starts the session. The launcher hides and a toast shows.
- [ ] Focus is the top row of an empty root list, with Pause/Resume, Edit, Restart, Complete and Cancel in Ctrl+K.
- [ ] Edit on an Allow session keeps it as Allow.
- [ ] A blocked app is closed or blocked when you open it. In Allow mode, apps outside the list are blocked.

**Pill and notice:**
- [ ] ✅ The pill shows "MM:SS | title" at the top centre and never takes focus.
- [ ] Drag the pill and it moves. A plain click on it opens the launcher with the Focus row selected.
- [ ] ✅ When time runs out, a notice appears at the bottom right of the screen.
- [ ] Extend adds 5 minutes. Complete ends the session. × closes the notice.

**Browser (needs the worktree extension):**
- [ ] A blocked site redirects to the blocked page, which reads "{site} is blocked by procast Focus".
- [ ] Snooze on the blocked page lets the site through for a while.
- [ ] Starting a new session clears old snoozes, so the site is blocked again.

**Settings → Focus:**
- [ ] The categories are listed. Import Focus Categories works, and a slow import doesn't steal focus.

**Known bug, not fixed on this branch:** `focus Social media detox` is parsed wrongly.

## 12. Calendar (My Schedule)

**Built:**
- ICS feeds stored as `[[calendar_feed]]`.
- My Schedule with Now, Today and Tomorrow sections.
- A Next row in the root list with "in N min".
- A reminder toast.
- Join Meeting, with an optional confirm question.
- Hide This Event, which hides one occurrence.
- An Add Calendar form and Settings → Calendar.

**Setup:** use a real Google or Outlook ICS address, or serve a local file with `python -m http.server`.

- [ ] My Schedule with no feeds shows the empty state. Enter opens Add Calendar with the address field focused.
- [ ] In Add Calendar, Tab goes address, then Name, then Colour. Left and Right move the colour ring, and a click picks a swatch.
- [ ] A bad address with Ctrl+Enter shows "Reading the calendar…", then "Couldn't read this calendar". The form stays open.
- [ ] A good address with Ctrl+Enter shows the toast "Added Work". My Schedule opens with events, and config.toml has the feed.
- [ ] Section headers and counts are right. "until HH:MM" appears in the accent colour.
- [ ] Hide This Event (Ctrl+K) removes the row, and it stays hidden after a restart. Other occurrences of a repeating event still show.
- [ ] A meeting within 15 minutes shows as Next at the top of an empty root list, and "in N min" counts down.
- [ ] The reminder toast "{title} · in 5 min" stays. The next time you open the launcher, the Next row is selected, and Enter asks "Join … on host?" before opening the link.
- [ ] Join Meeting with no meeting shows "No meeting with a link right now".
- [ ] With `confirm_join` on: use the Join Meeting hotkey and wait more than 10 s, past a timer tick. The question stays, and a second Enter joins the meeting.
- [ ] Settings → Calendar shows each feed with its colour dot, name and "updated N min ago". Delete works by hover click or the Delete key. A duplicate address is refused.
- [ ] Feed URLs never appear in error text.

## 13. Screenshots

**Built:**
- A grid of thumbnails, three per row, with OCR text search via `text:word`.
- A filter: All, Screenshots, Recordings or Pinned.
- A first-scan banner.
- Actions in Ctrl+K: Open (Ctrl+O), Open With (Ctrl+Shift+O), Quick Look (Ctrl+Y), Reveal (Ctrl+Alt+O), Pin, Copy Text, and Delete to the Recycle Bin.
- Paste Latest Screenshot.
- Settings → Screenshots: folders, how long to keep shots (including 1 hour), and the column count.
- Thumbnail memory is capped.

- [ ] Search Screenshots shows a 3-wide grid with "Today, …" captions and "Screenshots & Recordings N" above it.
- [ ] Arrow keys move through the grid. Enter copies the picture.
- [ ] `text:word` finds shots by their text and shows the matched words as the caption.
- [ ] The banner shows during the first OCR scan and goes away when it finishes.
- [ ] The filter dropdown and Ctrl+P switch between All, Screenshots, Recordings and Pinned. Pinned shots survive pruning.
- [ ] **Scroll the grid hard through hundreds of shots while watching procast's memory in Task Manager. It stays flat.**
- [ ] Very tall screenshots, such as a full-page capture, show a thumbnail and don't crash.
- [ ] Quick Look (Ctrl+Y): press it twice fast, and arrow from shot A to B and back to A. There's no flicker or blank image, and the size and time line is right.
- [ ] Delete on a local drive sends the file to the Recycle Bin, and the selection stays on that cell. On a network or USB drive, Windows asks first, and No gives "Not deleted".
- [ ] Paste Latest Screenshot pastes the newest shot into the focused app, or shows a toast if there is none.
- [ ] An empty folder shows Add Folder, and Enter opens Settings → Screenshots.
- [ ] The selection ring looks right in both dark and light themes.
- [ ] Videos show a plain cell, with no thumbnail by design.

## 14. Notes

**Built:**
- A floating Notes window with a live markdown editor: syntax marks hide when the cursor leaves a line.
- Saves itself after 0.5 s and on Esc.
- Pin, duplicate, delete, version history and Recently Deleted.
- A Ctrl+K action list, Ctrl+P browse, a formatting bar and Settings → Notes.
- Notes live in `%APPDATA%\procast\notes` unless you set another folder.

**Editor:**
- [ ] Open Notes from the launcher, **and from a hotkey**. The window comes to the front with focus.
- [ ] Type `# Title`, Enter, then `**bold**`. The title grows as you type, and `# ` hides once you move to line 2.
- [ ] Select a word and press Ctrl+B to wrap it in `**`. Ctrl+Z undoes and Ctrl+Y redoes.
- [ ] Each shortcut works: Ctrl+I, Ctrl+U, Ctrl+E, Ctrl+Shift+S, Ctrl+Shift+M, Ctrl+Shift+B, Ctrl+Alt+C, Ctrl+Alt+0 to 3.
- [ ] Type `[] ` and some text. A checkbox shows, and clicking it ticks the item and strikes it through without moving the cursor.
- [ ] The cursor blinks, and stops blinking while the window is inactive. Click, Shift+click and dragging all select, including dragging past the window edge.
- [ ] Rows are 8 px apart. Code blocks have rounded corners and padding.
- [ ] Ctrl+C, Ctrl+X and Ctrl+V work with text from other apps.
- [ ] A long note of a few hundred lines types smoothly, the cursor stays in view at the bottom, and the mouse wheel scrolls.
- [ ] Stop typing for about 0.5 s, and the `.md` file on disk has the text. Esc saves too.
- [ ] **Dead keys** (US-International or German): ´ then e gives `é`, not `´é`.
- [ ] AltGr characters type normally and don't trigger Ctrl+Alt shortcuts.
- [ ] Open a note saved by Notepad with CRLF line endings. There are no stray characters, and the title is right.

**Menus and lists:**
- [ ] Ctrl+K lists 8 actions with their keys. Typing filters them, and Go Back is faint in a fresh window.
- [ ] Ctrl+P, or Browse in the header, shows "● Current · N characters", then the other notes by "Opened …", with pinned notes first.
- [ ] Hovering or selecting a row shows Pin and Delete. Delete asks first, and while it asks, other shortcuts do nothing.
- [ ] Ctrl+N, Ctrl+Shift+P, Ctrl+[ and Ctrl+] work, even while a menu's search has focus.
- [ ] The footer `type` toggle shows the formatting bar. Heading ▾ then Heading 2 makes an H2. Link puts the cursor inside `()`. × closes the bar.
- [ ] Recently Deleted Notes shows deleted notes and restores them.
- [ ] Search Notes and Create Note work from the launcher.

**Settings → Notes:**
- [ ] Shows the header, the folder and keep-days rows, and the 4 Notes commands.
- [ ] Setting the folder to the relative path `Notes` shows a toast and uses the default folder.
- [ ] Changing the folder to another full path moves to that folder, and the note on screen is saved first.

## 15. Search ranking and clean-up

**Built:**
- Windows Settings pages rank below procast's own Focus and Clipboard commands.
- Define was removed from the default fallbacks.
- Icons and tints updated per the icon map.
- Clipboard image thumbnails share the memory cap.
- Relative times read "yesterday" and "N days ago".

- [ ] `focus`: Start Focus Session comes first, and the Windows Focus page is below every procast Focus command.
- [ ] `display`: Windows Display settings is still first.
- [ ] `clipboard`: Clipboard History comes before the Windows clipboard settings page.
- [ ] Clipboard History with many images: scroll and search over and over. Thumbnails stay correct, and memory holds steady.
- [ ] Relative times read "yesterday" and "3 days ago" in clipboard, calendar and notes.

---

## Known follow-ups (not on this branch)
- `focus Social media detox` is parsed wrongly.
- Error toast text needs a copy pass.
- If a timer ends while Ctrl+K is open, that row's Enter stays "Pause" until the list rebuilds.

## When everything passes
Tell me, and choose: **1.** merge into `main` locally, **2.** push and open a PR, or **3.** keep the branch as it is.
