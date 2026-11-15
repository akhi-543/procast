UI icons, as in the design system's Assets, Icons group. Only the icons in use ship.

- **Outlines**: Lucide 0.460.0 (ISC licence, lucide.dev), paths unmodified. 24-unit grid, round caps and joins, no fills. Older files keep procast stems (`paste` is Lucide `clipboard`, `restart` is `refresh-cw`); newer ones keep the Lucide name.
- **Drawn by procast**: `win-…` window layouts and `desk-…` virtual desktops, drawn from numbers: a screen frame with the snapped region solid.
- Single ink: each file strokes in `currentColor`, so inline the SVG and set its colour with a token. As an image it draws black.
- Draw at `ic-sm` 12, `ic-md` 14, `ic-lg` 16 or `ic-xl` 40, with stroke `ic-stroke` (1.75); 1.25 at 40 px; 2 inside a KeyCap.
- The glass pane (every closed shape filled at 20%) is not in the files: `design::Icons` adds it at 16 px and up.
- Which icon goes with which command: the design system's Icon map. Add an icon from the same family and version only.
