# procast design

The design system is in `docs/design/system/`, copied from the published Design System. `README.md` is the brand book: the idea, glass, states and content rules. `tokens.json` holds the values.

`src/design.rs` implements it. The views (`main.rs`, `settings.rs`) take every colour, size, radius, text step and icon size from there and write no literals.

Assets:

- `assets/fonts/SpaceGrotesk[wght].ttf` is Space Grotesk, a variable font under the SIL Open Font License (`OFL.txt` beside it). `design::register_fonts` loads it at start-up. Emoji use the system's Segoe UI Emoji.
- `assets/icons/` holds 60 UI icons from Lucide 0.460.0, under the ISC licence (`README.md` there). The paths are unmodified. `design::Icons` serves them, and the `@xl` and `@cap` stroke variants are made at load time.

Contrast is tested with `cargo test design`. The tests check text on the solid body, on glass and on the selected fill at 4.5:1; the warning text and the Danger button label at 4.5:1; tile glyphs against their wash at 3:1, at rest and selected; and the destructive panel rows. Glass is tested over stand-in backdrops: #808080 under `bg` for Dark and #404040 for Light. The real backdrop depends on the wallpaper, so a pass here is not a guarantee for every desktop.

## Where the code does not match the design system

Each item lists the reason.

Glass and window

- **Window corners** are cut by `SetWindowRgn` to radius 16 (`win.rs`, `round_region`, re-applied after each `SetWindowPos`). The spec value is used; the DWM corner is kept as a fallback. The region edge is aliased and may clip the DWM shadow. Not yet checked on screen. A size change that gpui makes by itself keeps the old region until the next open or keystroke.
- **Rim.** The 1 px border is a gradient with two stops. The spec has a four-stop version with an `edge_dim` plateau between 30% and 70%. Reason: `linear_gradient` takes two stops here. gpui-ce also blends the stops in Oklab without premultiplied alpha, so Light's white `edge` straight into its black-at-12% `border` would pass through a grey darker than either end, darkest in the middle. So the end stop differs by mode: Dark ends on `border` (both stops are white, the blend is clean and unchanged); Light ends on `border` laid over `bg_solid`, an opaque near-white grey (#d9dadc), so the blend runs monotonically from bright to that grey and the middle stays light, near the spec's `edge_dim` plateau. Over glass the Light end is a touch lighter than the spec's translucent `border`. The same rim is used on the ActionPanel and Dialog.
- **No `depth` shadow.** The spec gives the window a wide outer shadow. Here the body fills the whole window and the rounded window region clips everything outside it, so gpui-ce could only paint the shadow inside, under the glass, where it darkened the middle of the body. Dropped; the DWM shadow is what lifts the window.
- **Blur radii are halved in code.** gpui-ce's `blur_radius` is the Gaussian sigma and a CSS blur is two sigma, so `shadow()` passes `blur / 2`. The token values stay as in the spec.
- **Glass off** switches the launcher's window backdrop from acrylic (`Blurred`) to `Transparent` wherever the glass decision is refreshed (startup, each open, config reload). Not `Opaque`: gpui-ce clears an opaque window to white, which would show in the anti-aliased corners inside the rounded region.
- **No refraction, no moving highlights.** By design; the system README explains why.
- **No backdrop levelling.** The body is `bg` at 72% (Dark) or 85% (Light) over the OS blur, with no dimming or lifting. Reason: not built. This is why the contrast tests use stand-in backdrops.
- **Glass fallback** checks EnableTransparency, high contrast and battery saver. Remote sessions are not detected. Reason not recorded. With glass off the body is solid `bg_solid` with a solid border and the rim; the sheen is dropped and the acrylic backdrop is switched off.
- **Settings is opaque.** No glass and no Glass toggle. Reason: the spec limits glass to the launcher, the ActionPanel and the Dialog. A toggle is a recommendation, not a component, so it was left out until it is needed.

Colour and contrast

- **Light tint icons on their wash over glass** measure under 3:1 over the #404040 stand-in: System 2.86, File 2.95, Window 2.97, Danger 2.99. Their test is `#[ignore]`. Reason: the spec values are kept; all hold on `bg_solid`. Muted text on the selected fill over glass holds 4.5:1.
- **Light warning as text on glass** measures 4.39:1 over the #404040 stand-in, under 4.5:1. Its test is `#[ignore]`. Reason: the spec value is kept. A darker value near #855500 would pass; that change belongs in the design system.
- **Destructive ActionPanel rows.** The icon is `danger`. The label is `danger` at rest and `text` when selected. Reason: `danger` on the selected fill measures 3.59:1 (Light) and 4.29:1 (Dark), under the 4.5:1 text rule. The icon keeps 3:1 on the selected fill.
- **Letter spacing** on `ts-display` and `ts-query` is not applied. Reason: gpui text styles have no letter spacing in this build.
- **Key cap glyphs** (return, up, down) draw at 12 px, not 11 px. Reason not recorded.
- **Image icon in the action bar** draws at 16 px (`ic-lg`), not 14 px (`ic-md`). Reason not recorded.
- **Action bar with the panel open** keeps showing the selected row's kind, not "Actions" with a settings icon. Reason: not in the brief, so unchanged.

Components

- **Focus ring** is gpui's hollow ring (`.ring(2 px)`, colour at 32%) outside the solid 1 px border, on fields, buttons and the hotkey control, not a 2 px spread `box-shadow`. Reason: gpui-ce paints an outer shadow over its whole dilated rect under the element, so under a see-through control (the 6% `surface` fill) it became a 32% accent slab and the placeholder fell to about 2.8:1.
- **Shortcuts "Show"** is a segmented control over all seven kinds; it wraps when the row is narrow. The earlier click-to-advance dropdown (`select_field`) is gone. Enter on the row still moves to the next kind.
- **Fallback commands and folders** are Field entries, not reorderable ListRows. Reason: reordering is new behaviour outside this brief's scope.

Typeface and themes

- **Typeface.** Space Grotesk is used throughout, in place of Segoe UI Variable. It covers fewer glyphs than Segoe UI Variable, and any missing glyph falls back to another font without a warning. The embedded file is the variable font, whose default instance is wght 300 (Light); every other weight relies on the text system applying the `wght` variation axis. To check on screen that SEMIBOLD and regular text are not all drawn at 300.
- **Themes.** The named themes are gone; the system ships Dark and Light. `[theme]` applies only `background`, `text`, `muted` and `selected`, which map to `bg`, `text`, `text_muted` and `accent`. `selected_text`, `border`, `separator` and `container` are still parsed but ignored, and so are old named-theme values.

Not yet checked on screen

Tasks 1 to 5 were verified with `cargo check` and `cargo test` only; the app was not run. Still to check: the corner region and rim, the scrims, SEMIBOLD on the variable font, glyph coverage, icon alignment, the Field entries and the wrapping Show segments in Settings, the hollow focus ring, the Light rim, and the glass-off backdrop.
