A launcher row in its 40 px slot: icon tile, title, a subtitle that gives way first, and one accessory on the right.

**You provide:** an IconTile, a title, an optional subtitle, an optional accessory, whether it is selected, and its kind tint (a pc-tint-* class on the row).

- The title keeps priority: it may take up to 70% of the width; the subtitle truncates first.
- The selected row is a neutral glass lozenge, whatever its kind: a fill from sel_top to sel_bottom, a 1 px sel_ring edge, a highlight along its top, and a 3 × 16 pill in the accent. The kind shows only in the icon tile. Never tint or fill a row with a hue.
- Accessory in root search: the row's hotkey as KeyCaps, else its alias, else nothing. In other lists: the row's accessory text, said once per run of rows that share it. It is text_faint, and text_muted on the selected row.
- Every row is exactly the row height, so the list virtualises without measuring. Do not make a taller row; use a card above the list (see CalculatorCard).
