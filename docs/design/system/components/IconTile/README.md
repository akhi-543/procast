The 28 px icon box at the start of a row, so titles line up whatever the icon is.

**You provide:** the icon (a vector icon, an app image, or an emoji), the kind tint if the row has one, and whether its row is selected.

- A vector icon at 16 px, stroke 1.75, centred in a tile with radius r-md.
- With a tint: the icon in the hue, on a 14% wash of that hue, with a 1 px inner rim at 24% so the tile reads as a small pane of tinted glass. Order here: Accent (monochrome), System, Window, Paste, Web, File, Calc, Danger.
- In a selected row the tile keeps its tint and the icon steps up to text, so it keeps 3:1 on the grey selection. The tile is the only colour in a selected row.
- With no tint: surface fill, hairline rim, icon in text_muted; selected, surface_strong and text.
- An app image fills the box at 24 px with no wash or rim. An emoji draws bare at 18 px.
- The tint is the row's kind, not decoration. Do not pick a tint for looks.
