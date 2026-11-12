A confirmation over the whole window, for a step that cannot be taken back.

**You provide:** a title phrased as a question, a one-line message, the primary label, and whether the step is destructive.

- 320 px wide, radius r-lg, bg_elevated, with the gradient rim and the float shadow. The window behind is covered by scrim, stronger than the ActionPanel dim, because nothing else can be used until it is answered.
- A 40 px icon: a warning triangle in danger when destructive, an info circle in the accent otherwise.
- Buttons stack full width: the primary (Danger or Primary) first and focused, then a Quiet "Cancel". Esc cancels.
- Use it rarely. A smaller destructive step is armed in the ActionBar instead ("Press ↵ again to confirm").
