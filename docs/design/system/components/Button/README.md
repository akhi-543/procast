The one button, in three kinds: Primary (solid, monochrome), Quiet, Danger.

**You provide:** a label (a verb), the kind, and whether it has focus.

- At most one Primary on a screen. It is the only solid fill that is not a warning: the text colour as a fill, with the body colour as its label.
- Danger only when the caller says the step is destructive. Its text colour is danger_text.
- Focus adds the ring: accent for Primary and Quiet, the danger hue for Danger.
- 32 px tall, ts-body_sm, radius r-sm. In a Dialog buttons stack full width.
