# Screen Time: native dashboard in the launcher

Status: proposed (2026-11-26). Builds on `2026-11-25-screen-time-design.md` (branch `feat/screen-time`).

## Intent

The user tried the web dashboard on `127.0.0.1:8737` and likes it, but does not want to open a
browser to see it. They want the same kind of dashboard (not a copy) inside procast itself, opened
from the search bar the way a store extension opens its view, with Esc going back. Once it exists,
the web dashboard goes away.

Success: typing **Screen Time** shows today's total, the hourly chart, categories and the rest
right in the launcher, instantly, in procast's own look, and nothing about Screen Time needs a
browser any more.

## Goals

- A native GPUI view in the launcher window that replaces today's Screen Time list-and-text view.
- Day, Week and Month, with previous / next period, like the web page.
- The web page's sections, redrawn natively (see Layout).
- Same numbers as before: the view reads through the existing `wellbeing` functions.
- Remove the web dashboard: the HTTP server, `dashboard.html`, the **Open Screen Time Dashboard**
  command and every mention of port 8737.

## Non-goals

- Pixel-matching the web page. "Similar style, not exact."
- Hover tooltips on chart bars. Selection and the detail line cover what tooltips did.
- A separate window. The user chose inside the launcher.
- Writing it as a `@raycast/api` extension. The user chose the native view.
- Changes to the sampler, storage, import or Focus.

## Anti-goals

- A view that is slower to open than the list it replaces. It reads the database off the UI thread
  as the current view does; nothing blocks rendering.
- Charts that only read well in dark mode. Light and dark both use theme tokens.
- Losing anything the web page could do: per-category moves and CSV export must still be reachable.

## Constraints

- Rust 2024, GPUI, rusqlite as already in the branch. No new dependencies.
- procast's design system (`docs/superpowers/plans/procast-design-system.md` and its rulings):
  theme `Tokens`, `design::ts` sizes, `design::r` radii, `design::sp` spacing, Space Grotesk.
- The launcher's width is fixed (`BASE_WIDTH` 750, or `[launcher] width`). Everything fits one
  column at that width.
- Category colours: the same seven slots as the web page (Development, Productivity, Messaging,
  Social, Streaming, Gaming, Browsing) plus Other in grey; other user categories cycle a fallback
  list. Each colour must read on both themes.
- Never bind or test against port 8737 (the user's micro bot serves there until import).

## Approaches considered

1. **Native GPUI view in the launcher (chosen by the user).** Reads `wellbeing` directly, draws
   bars and cards from `div`s and the one line chart with GPUI paths, follows the theme, works with
   no Node host. Costs: charts written by hand.
2. **A `@raycast/api` extension on the Node host.** Installable like store extensions, but the API
   has no charts, so charts would be SVG images in a Detail view, data would come over IPC, and it
   works only while the extension host runs. Rejected: static, slower, extra moving parts.
3. **Separate native window like Settings.** More room, stays open. Rejected by the user in favour
   of the launcher.

The user also chose to **remove the web dashboard** rather than keep both.

## Behaviour

### Opening

- **Screen Time** opens the dashboard on Day (today). **Screen Time This Week** opens it on Week.
  No new command for Month; Tab reaches it.
- Screen Time off: the view keeps today's "off" state (Import from micro bot / Turn on). Unchanged.
- No data in the span: the cards show zeros and a single muted line "No screen time recorded",
  not an error.
- A database read error shows the existing error state with the message.

### Periods (my call on comparison, see Calls)

- Day: one day. Week: Monday to Sunday. Month: the calendar month.
- ← / → step one period back / forward. → stops at the current period (no future).
- The header names the span: "Today", "Yesterday", "Wed 19 Nov"; "This week", "17 – 23 Nov";
  "This month", "October 2026".
- Comparison: a past period is compared with the whole period before it (`wellbeing::prior`). The
  current period is compared to date, as the launcher does today: today against yesterday,
  Monday-to-today against the same days last week, the 1st-to-today against the same days of last
  month (clamped to that month's length).

### Keys

- Tab / Shift+Tab: Day → Week → Month → Day.
- ← / →: previous / next period (the search field is empty in this view, so the arrows are free;
  if the user has typed a filter, the arrows go to the text field as usual).
- ↑ / ↓: move through the app and site rows at the bottom; the view scrolls to keep the
  selection visible.
- Typing filters the app and site rows (as the current view filters its list).
- Ctrl+K on a row: Move to Category…, Export CSV. Ctrl+K with no row: Export CSV.
- Esc: back to root, like every view.

### Window height

The launcher grows to a taller fixed height while the dashboard shows (my call: 640 px before
interface scaling, or the normal height if that is already taller), and shrinks back on leaving.
The content scrolls inside it. `target_height` already varies by state; this adds one case.

## Layout

One scrolling column of cards, 16 px page padding, 12 px gaps, cards on the theme's surface with
`r::LG` corners. Top to bottom:

```
┌ [Day│Week│Month]                         ‹  Today  › ┐
│┌ 1h 0m  so far today ──────────────────────────────┐│
││ ▁ ▁ ▂ ▅ █ ▁  stacked bars per hour (Day) or day    ││
││ 12 AM   6 AM   12 PM   6 PM          (Week/Month)  ││
│└───────────────────────────────────────────────────┘│
│┌ Categories ───────────────────────────────────────┐│
││ ● Development   3%  2m   ▬▬                         ││
││ ● Productivity 29% 17m   ▬▬▬▬▬▬                     ││
│└───────────────────────────────────────────────────┘│
│┌First use─┐┌Busiest hour┐┌Longest stretch┐┌Switches┐│
│└──────────┘└────────────┘└───────────────┘└────────┘│
│┌ Productive and distracting ───────────────────────┐│
││ 33% productive  ████████░░░░░░░░░░░                ││
││ ● 19m productive  ● 40m neutral  ● 0m distracting  ││
││ productive share per week, past 12 weeks (line)    ││
│└───────────────────────────────────────────────────┘│
│┌ Compared with the day before ─────────────────────┐│
││ Development   −37m  2m   solid bar / faded bar     ││
│└───────────────────────────────────────────────────┘│
│┌ Past year ────────────────────────────────────────┐│
││ heatmap, 53 weeks × 7 days                          ││
│└───────────────────────────────────────────────────┘│
│ Apps                                                │
│  ▸ code.exe            Development       1h 2m      │
│ Websites                                            │
│    youtube.com         Streaming           14m      │
└─────────────────────────────────────────────────────┘
```

Section details:

- **Header**: segmented Day / Week / Month control (clickable as well as Tab), ‹ › buttons
  (clickable), span name. The search field stays at the top of the launcher for filtering rows.
- **Total**: display-size total; caption "so far today" / "this week" / "this month" for the
  current period, otherwise "on Wed 19 Nov" etc. Chart: 24 hourly bars (Day), 7 day bars (Week),
  28–31 day bars (Month), each stacked by category colour, with a light 3-line grid and labels.
  Bars use the `buckets` from `stats`.
- **Categories**: every category with time > 0, largest first: dot, name, share %, time, thin bar.
- **Stat tiles** (four across, from `stats`): First use (with last use underneath), Busiest hour
  (Day) / Busiest day (Week, Month) with its time, Longest stretch, App switches.
- **Productive and distracting**: share bar and legend from `productive` / `distracting` in
  config. Below it, the productive share per week for the past 12 weeks as a line with dots (weeks
  with under 1 minute are gaps), read with `wellbeing::days` over 84 days.
- **Compared with the period before**: per category, the selected-period bar solid and the
  previous one faded, with the signed difference ("−37m", "No change").
- **Past year**: 53-column heatmap of daily totals from `wellbeing::days` over the past 371 days,
  five shades of the accent colour, month labels on top. Shown in all three periods; it does not
  depend on the selected period.
- **Apps / Websites**: the selectable rows, as in today's view (icon, name, category, time). The
  selected row shows a one-line detail under it with its per-hour numbers for Day (the data
  `hours_by_name` already loads), replacing today's right-hand detail pane.

## Data

All reads go through existing functions on a background task, as `load_screen_time` does today:

- `wellbeing::stats(con, from, to, before, rules)` for the period (total, buckets, categories,
  previous categories, apps, sites, first/last, longest, switches).
- `wellbeing::days(con, …)` once per open for the 12-week line and the year heatmap (84 and 371
  days; one call over 371 days covers both).
- `wellbeing::hours_by_name` for the per-hour row detail on Day.
- `wellbeing::span` grows a `Month` case and an anchor day so it can name any period, not only the
  current one. `Range` gains `Month`.
- Unsaved counters are flushed (`flush_pending`) before a read of a span that includes today, as
  now (R12).

## Removing the web dashboard

Delete `src/wellbeing_http.rs`, `assets/screen-time/dashboard.html`, `Cmd::OpenScreenTimeDashboard`
and its command-list entry, the "Open Dashboard" action, and the server start in
`start_screen_time`. Update: the `config.rs` sample comment that names the URL, the README feature
line and table row, the micro bot README note written in the earlier work (it mentions "the same
dashboard on 127.0.0.1:8737"), and the earlier design doc gets a one-line "superseded by" note for
the dashboard section. Tests that covered the HTTP server go with it. `export_csv` stays (the CSV
action uses it).

## Left to the builder

- How to split the view's rendering into functions or a module (a new `src/screen_time_view.rs` is
  reasonable if `main.rs` would grow by several hundred lines).
- The exact fallback colours for user categories, as long as they pass contrast on both themes.
- Chart label density at 750 px.
- Tests: the pure parts (span/period naming, comparison spans including month clamping, week
  share computation, heatmap bucketing, colour assignment) get unit tests; the GPUI drawing is
  checked by hand.

## Builder notes (from the design check)

- **Two spans per period.** The chart and the headline read the full period (Week: 7 bars,
  Month: every day of the month; future days are empty bars). The comparison card and the "vs
  before" numbers read the to-date span against the matching to-date span before it. So `span()`
  returns a display span and a comparison span, `Range` gains `Month`, and the mode carries the
  anchor day as well as the range. The stale-read check (`screen_time_loaded`) compares both.
- **Rows stay capped at 12 apps and 12 sites**, as `stats` and the web page did. Filtering
  filters those rows.
- **Row detail on Week and Month** is the row's time and share of the period. Day adds the
  per-hour numbers.
- **Empty, "No matches", error and no-row Ctrl+K** keep working through one non-selectable
  anchor item, as the header row did; Enter with no row does nothing.
- **Colours**: the slot colours (`--s1`..`--s7`, `--other`) and heatmap shades (`--h0`..`--h4`)
  for both themes live only in `assets/screen-time/dashboard.html`. Copy them into Rust before
  deleting that file.
- **Rendering**: the results body is a `uniform_list`; the dashboard needs its own non-list body
  and scroll handle. `src/notes/window.rs` has the only `canvas` use, a model for the line chart.
- **Tab** is bound to `SelectNext` in the query key context; the period switch needs its own
  binding in a Screen Time context.
- **micro bot README**: edit only the "Moved into procast" note at its top (written during the
  earlier work, still uncommitted in `D:\Pro\micro bot`), dropping the dashboard address. Leave
  the rest of that README. That step is outside the procast repo.

## Calls I made (for the user to check)

1. Comparison for the current period is "to date" (fair), not against the whole previous period
   as the web page did.
2. The launcher grows to about 640 px tall in this view and scrolls.
3. The right-hand detail pane goes; a selected row shows its detail inline.
4. Past year shows in all three periods.
5. No Month command; Tab reaches Month.
6. Apps and Websites stay at the top 12 each.
7. Week and Month bar charts always show the whole period, with empty bars for days still ahead.
