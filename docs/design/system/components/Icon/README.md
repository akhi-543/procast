A vector icon: an SVG outline on a 24-unit grid, stroked in the current colour. The only way to draw a UI icon.

**You provide:** an icon from this set, a size (ic-sm 12, ic-md 14, ic-lg 16, ic-xl 40) and a colour token.

- The set is Lucide (ISC licence): 60 icons mapped to procast's needs. One family only, so every icon shares a stroke, a corner and a grid.
- Stroke 1.75 at 12 to 16 px; 1.25 at 40 px so large icons do not look heavy; 2 inside a KeyCap.
- Round caps and joins, no fills, no two-tone. Colour comes from the token, never from the file.
- File rows pick their icon by type: picture, video, audio, archive, code, document, folder.
- No emoji as icons, no Unicode symbols, no second icon family.
- Previews carry each icon inline as SVG, unmodified from Lucide 0.460.0. The same 60 files are stored under Assets, Icons. In the app, ship those files.
