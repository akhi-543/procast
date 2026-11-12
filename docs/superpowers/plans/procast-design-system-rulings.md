# Rulings made while implementing the design system

Ruling: Committed user's uncommitted redesign WIP as ee57e8c on new branch design-system — user chose "carry WIP, commit first" — cost if wrong: one commit to drop.
Ruling: Settings window is opaque bg_solid, no glass — spec says glass only on launcher window, ActionPanel, Dialog — cost: settings looks flatter than user may expect.
Ruling: No "Glass" toggle in Settings; automatic fallback only — notes-themes-and-settings is a recommendation, not a component spec; YAGNI — cost: add a toggle later.
Ruling: Icons extracted from Icon/preview.html inline SVGs (asset-store read failed); stroke 1.75 baked, @xl/@cap variants by string swap — cost: none if paths identical to Lucide 0.460.0 (spec says unmodified).
Ruling: Light warning (#8a5900) on glass measures 4.39:1 (<4.5) vs stand-in #404040 backdrop — spec value kept, assertion ignored — cost: armed line slightly under AA on worst light wallpaper; fix belongs in the Design System artifact
Ruling: Corners — implement SetWindowRgn r-xl 16 (spec value), verify in screenshot step; if blur breaks or edge looks bad, fall back to painting r-md to match DWM — cost: jagged region edge or 8 px corners
Ruling: Destructive ActionPanel rows — icon stays danger (3:1 holds), label danger at rest but text when selected — spec says label in danger, but 4.5:1 text rule binds harder — cost: selected destructive label not red
Ruling: Task 5 re-dispatched fresh on opus (sonnet rate-limited until 23:50) — cost: higher per-task spend
Ruling: Fallback-command / folder lists kept as Field entries, not reorderable ListRows — notes say ListRows but reordering is new behaviour, out of "look and control types" scope — cost: lists look unlike the notes
Ruling: select_field cycles on click, no menu — design system has no dropdown menu yet — cost: 7-option dropdown needs many clicks
Ruling: Task 6 doc-only reviewed by controller read instead of a separate reviewer seat; final whole-branch review includes it — cost: one less review pass on prose
Ruling: drop `depth` from glass_window — window has no room for an outside shadow and SetWindowRgn removes DWM's — cost: window has no lift off the desktop
Ruling: Shortcuts "Show" back to segmented control; delete select_field (single user, cycling dropdown is a regression) — cost: wide segmented track that may wrap
Ruling: fix wave also takes final minors 1-7 and g::ALL cfg(test); scrim click closes ActionPanel — cost: none expected
Ruling: glass-off backdrop = Transparent not Opaque — Opaque clears white, shows in AA corners — cost: no DWM blur saving if Transparent still composites
Ruling: Light tints on wash over glass under 3:1 (System 2.86, File 2.95, Window 2.97, Danger 2.99) — spec values kept, test ignored — cost: icons slightly under 3:1 on worst light wallpaper; belongs in Design System
