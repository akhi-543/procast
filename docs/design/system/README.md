procast is a Windows-native keyboard launcher. This is how it looks, how it speaks, and what each visual choice costs the machine. This version is a redesign made here first: it is ahead of the code. Sizes, radii and component anatomy still match `src/design.rs`; colour, glass, type and icons are new.

## The idea

**Quiet glass, one grey selection, colour only in the icons.** Whatever is selected, anywhere, is the same neutral grey: launcher rows, ActionPanel rows, the calculator card, the Settings sidebar. An icon tile carries the hue of what the thing is: window commands teal (`tint_window`), system actions amber (`tint_system`), things you paste blue (`tint_paste`), the calculator pink (`tint_calc`), anything destructive red (`tint_danger`).

That is the only colour. Keep everything around it quiet:

- **The accent is monochrome.** `accent` is the text colour: white on Dark, ink on Light. Use it for the caret, the search mark, focus, on-states and the one Primary Button. It is also the selection colour. There is no brand hue.
- **The body is glass.** A thin tinted pane over a blurred backdrop, with a rim of light all the way round, a sheen, thickness and depth. The wallpaper colours the pane but never shows as a picture.
- Make what the person typed the largest text in the window (`ts-query`).
- One typeface, Space Grotesk, and one icon family, drawn as vector outlines.

Two themes ship: **Dark** and **Light**.

## Glass

The window is a pane of glass with thickness, not a frosted sheet. Six layers, bottom to top:

1. **Backdrop**, supplied by the system: whatever is behind the window, blurred (`glass-blur`), saturated (`glass-saturate`) so its colour survives, and levelled: dimmed on Dark, lifted on Light, so no wallpaper can wash out the text.
2. **A thin tint.** `bg` is the theme colour at 72% (Dark) or 85% (Light). Thin enough that the backdrop colours the pane.
3. **A sheen.** One fixed diagonal fall of light from the top-left corner, `edge_dim` fading to nothing by 42%.
4. **A rim all the way round.** The 1 px border is a gradient, `edge` at the top-left falling to `edge_dim` and then `border` at the bottom-right, as light falls off around the lip.
5. **Thickness.** `rim`: a bright line just inside the top edge and a shade just inside the bottom edge.
6. **Depth.** `depth`: one wide, soft shadow that lifts the pane off the desktop.

Rules:

- **Glass is for the floating layer only**: the window, the ActionPanel, the Dialog. Never glass on glass. Controls inside are flat washes.
- **A surface that pops over the window must stand clear of it.** Three things do that together: `bg_elevated` is a step lighter than the body and all but opaque, so no row reads through it; `float` is a deep, close shadow; and the window behind steps back under a scrim, `scrim_soft` for the ActionPanel and `scrim` for a Dialog. It keeps the same rim and takes no blur of its own.
- **The selected row is a clearer piece of glass**: a lozenge that runs from `sel_top` to `sel_bottom` with a `sel_ring` edge and a highlight along its top. Neutral, never a hue.
- **Corners nest.** The window is `r-xl` (16); rows sit 8 in and are `r-md` (8). Inner radius = outer radius minus the gap.
- **Do not thin the tint or drop the levelling.** Every text colour is checked at 4.5:1 on the levelled backdrop at exactly these strengths.
- **No refraction.** True liquid glass bends what is behind it at the edges. That needs a displacement pass over the backdrop, which the system backdrop does not offer a desktop window and which would cost a shader pass per frame. This is honest glass: light, edge and depth, no lensing. No moving highlights either.
- **Fall back to `bg_solid`** when the system has transparency effects off, on battery saver, in high-contrast mode and on remote sessions. Keep the rim; drop the sheen and the depth shadow.

## Resource budget

procast is summoned dozens of times a day and must answer in one frame, then cost nothing while hidden. Each visual choice is a trade between how it feels and what it spends.

| Choice | What the person gets | What it costs, and the limit |
|---|---|---|
| Glass body over the OS backdrop | Depth, and a sense of place on the desktop | The system compositor blurs behind the window while it is visible; nothing while hidden. No in-app blur, ever. Falls back to `bg_solid` |
| Rim, sheen, thickness: all static fills and 1 px lines | Reads as real glass, with an edge | Drawn once with the window. No per-frame work |
| No refraction, no moving highlights | Nothing is lost that a launcher needs | Saves a displacement pass over the backdrop on every frame, which the system backdrop cannot supply anyway |
| One wide `depth` shadow on the window | The pane floats | One blurred shadow, static |
| Keep the window alive, cloaked, and un-cloak it on the hotkey | It is there on the next frame | No window creation, layout or font load on the hot path |
| Every list slot is the height `row`, headers included | Rows never jump; the window holds whole rows | The list virtualises with no measuring: only visible rows are built |
| State changes in one frame, no tween | Nothing lags behind the keys | No animation timers; the only timer is the 500 ms caret blink |
| Alpha washes for surfaces, tiles and selection | Depth without chrome | One blended fill each: no gradients, no images |
| `float` only on ActionPanel and Dialog; focus as `ring`, a spread with no blur | The two floating things read as floating | Three blurred shadows on screen at most, counting `depth`; none in the list |
| Vector icons, one family | Its own look, the same on every machine, crisp at every scale | A few KB per icon shipped; each is rasterised once per size and cached. Ship only the icons in use |
| Space Grotesk, bundled | A voice of its own | One variable font file (about 100 KB) loaded once at start. One family only: a second costs the same again |
| One neutral selection for every row | Calm, and the same everywhere | Nothing: no per-row hue to look up, and no sampling of app icons |
| Every length through one scale factor | The same proportions on a laptop and a 4K display | One multiply |

When a new idea needs in-app blur, a gradient, an image, a tween, a second blur or another shadow, it is outside the budget. Find the flat version first.

**Motion.** None beyond the caret blink. Smooth here means immediate: the next state lands on the next frame.

## Content fundamentals

- **Write the name lowercase: procast.**
- **Command and action titles are Title Case:** "Clipboard History", "Add to Favorites", "Hide from Root Search". **Everything else is sentence case:** section labels ("Use "cl" with"), hints, status lines, descriptions.
- **Placeholders say what the field searches and end with an ellipsis:** "Search apps and commands…", "Search clipboard history…", "Ask a question…".
- **An empty state says what to do next.** Title: "Search your files". Hint: "Type a name; include \ to match a path".
- **Status lines are short, plain and past tense when done:** "Deleted from clipboard history", "Force quit". Failures say what failed and why if known: "Could not end it (it may run as administrator)".
- **Say a row's kind once, in the ActionBar** ("Application", "Command"). Never repeat it down a column.
- **Actions are verbs:** Open, Run, Copy, Close, Submit. A destructive step asks twice in the bar: "Press ↵ again to confirm".
- **Speak to the person directly and skip "please".** No exclamation marks.
- **Emoji are content, not decoration.** They appear in the emoji picker and nowhere in the interface chrome.
- **Keys are drawn as KeyCaps, never typed as text.**

## Visual foundations

### Colour

- Body `bg` (glass) or `bg_solid` (no glass); things that float `bg_elevated`; cards, inputs and key caps `surface`; chips, the current segment and the off Toggle `surface_strong`.
- `text` for titles, `text_muted` for subtitles and descriptions, `text_faint` for section labels, placeholders and accessories. An accessory steps up to `text_muted` on its selected row.
- `hover` is always lighter than a selected fill.
- `hairline` divides; `border` edges inputs and key caps; `edge` and `edge_dim` are the two ends of the glass rim.
- `accent` is monochrome. `accent_text` is the body colour, for labels on it.
- Status: `success`, `warning`, `danger`, shown as an 8 px dot beside a message. A status always carries its words; the dot never stands alone.
- **Selected fill** is always neutral: a glass lozenge from `sel_top` to `sel_bottom` with a `sel_ring` edge, plus a 3 × 16 pill in solid `accent` on the left edge. It never takes a hue.
- **Kind tints** colour an icon and its tile, and the small kind icon in the ActionBar. Nothing else.
- There is no purple anywhere, and the neutrals carry no colour cast.

**Contrast.** Every pair the components paint is checked on glass over its worst levelled backdrop and on `bg_solid`: 4.5:1 for all text (including `warning` as text, labels on `accent` and `danger`, and `text_muted` on the selected fill), 3:1 for icons on their tiles (at rest and in a selected row), the pill and status dots.

### Type

**Space Grotesk** for everything: one variable file, weights 400 and 600, plus 500 for KeyCap labels. Cascadia Mono for code. Use only the seven `ts-` steps: `ts-caption` 11, `ts-small` 12, `ts-body_sm` 13, `ts-body` 14, `ts-heading` 16, `ts-display` 20, `ts-query` 22. The two display steps are tracked in by 0.01em. Never go below 11 px: the face loses its shapes.

### Space, size, radius

- A 4 px grid: `space-4`, `space-8`, `space-12`, `space-16`, `space-24`. `space-2` appears only inside controls.
- Fixed heights: `bar` 56, `row` 40 (the row box inside is 36), `footer` 40, `control` 32, `tile` 28, `cap` 20.
- Window width 750 before scaling. Height = 2 + `bar` + `row` × (rows shown + 1) + 2 × `list_pad` + `footer`.
- Small things get small corners; things that hold other things get large ones: `r-xs` caps and segments, `r-sm` inputs and buttons, `r-md` rows and icon tiles, `r-lg` cards, panels and dialogs, `r-xl` the glass window.

### Borders, shadows, focus

- Borders are 1 px. `hairline` and `border` are deliberately faint; the fill names the control.
- Focus is a solid 1 px `accent` border plus `ring`. A focused Danger Button rings in `danger`.
- The window casts `depth`. Only ActionPanel and Dialog cast `float`. All three carry the gradient rim and `rim`. An open ActionPanel dims the window with `scrim_soft`; a Dialog with `scrim`.

### States

| State | Recipe |
|---|---|
| Hover | `hover` fill |
| Selected | Neutral glass lozenge + `accent` pill; the tile keeps its tint and its icon steps up to `text`; the accessory steps up to `text_muted` |
| Focused | `accent` border + `ring` |
| On | `accent` track, `accent_text` knob on the right (Toggle); `surface_strong` segment, weight 600, a 6 px `accent` dot (SegmentedControl) |
| Armed | The ActionBar's right side reads "Press ↵ again to confirm" in `warning` |

## Iconography

- UI icons are **vector outlines** from one family, **Lucide** (ISC licence): SVG on a 24-unit grid, round caps and joins, no fills. The Icon component lists the 60 in use.
- Stroke `ic-stroke` (1.75) at 12 to 16 px, 1.25 at 40 px, 2 inside a KeyCap. Colour always comes from a token.
- **Why vectors and not the system icon font:** an icon font ties the look to one operating system's house style, draws nothing where the font is missing, and allows no control of stroke weight. Vectors are procast's own, identical everywhere, and tune to the grotesk. The price is a small set of files to ship and one cached rasterisation per size.
- **Icon tiles** are small panes of tinted glass: the icon in its kind hue on a 14% wash of that hue with a 1 px inner rim at 24%, radius `r-md`. In a selected row the tile keeps its tint and the icon steps up to `text`.
- App rows show the app's own icon at 24 px inside the 28 px tile, with no wash or rim. Emoji draw bare at 18 px.
- Each icon is a single-ink outline that strokes in `currentColor`: inline the SVG to colour it with a token. The 60 files are stored under Assets, Icons; previews carry them inline. As an image a file draws black.
- **There is no logo.** Set the name in plain type: "procast", lowercase, in Space Grotesk at weight 600.

## Do and don't

| Do | Don't |
|---|---|
| Take every colour, text size, radius and icon size from the tokens | Write a literal radius, text size or icon size in a view |
| Keep every selection the same neutral grey | Tint a selection by kind, or give the interface a brand hue |
| Use Field, Button and Selectable | Style a one-off input, pill or highlight |
| Keep one Primary Button on a screen | Put two solid fills side by side |
| Keep the tint and backdrop levelling at their tested strengths | Thin the glass until the wallpaper is readable through it |
| Keep glass to the window, ActionPanel and Dialog | Put glass on glass, or blur inside the window |
| Check a new effect against the resource budget | Add in-app blur, gradients, tweens or shadows to the list |
