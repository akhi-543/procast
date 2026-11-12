# Notes: liquid glass

Working notes from two sources, read 2026-11-12: the gallery and explainer at liquidglassdesign.com, and the kube.io article on building liquid glass with CSS and SVG. They record what the look is made of, what it costs, and what procast takes from it.

## What liquid glass is

- A material that behaves optically, not a colour scheme. Its defining trait is **refraction**: content behind the pane bends at the edges like a lens. Blur plus tint plus a hairline border is ordinary glassmorphism.
- The test both sources imply: do straight lines passing behind the pane visibly distort near its edge?
- Around the refraction sit a specular rim that follows the surface, tint and shadow that respond to what is underneath, and shapes that merge and morph.

## How refraction is built (the article)

- Physics: Snell's law, air 1.0 to glass about 1.5, one refraction event, rays straight down.
- The pane is a height profile across a bezel at its edge. Convex profiles (a circle or squircle arc) keep rays inside the shape; concave ones throw them outside. A "lip" profile mixes both.
- The profile is turned into a **displacement map**: per pixel, how far to shift the backdrop in x (red channel) and y (green channel), 128 meaning no shift.
- The map feeds an SVG displacement filter used as a backdrop filter. A second image adds the specular rim, brightest where the surface faces a fixed light direction.
- Published settings are per component, for example a search box at 0.2 specular opacity, 0.7 refraction, blur 1; a music panel at 0.6 background opacity.
- Limits the author states: it works only in Chromium-based browsers, the map must be rebuilt whenever the shape or size changes, and it is experimental.

## What that means for procast

| Trait | Take it? | Why |
|---|---|---|
| Thin tint the backdrop can colour | Yes: 72% Dark, 85% Light | The earlier 92 to 94% read as an opaque panel |
| Backdrop blur and saturation | Yes | Supplied by the system, once, for the window |
| Backdrop levelling (dim on Dark, lift on Light) | Yes | What lets the tint be thin and text still hold 4.5:1 |
| Rim of light all the way round, brighter top-left | Yes: gradient border from `edge` to `edge_dim` | A single top line reads as a border, not an edge |
| Thickness: inner highlight and inner shade | Yes: `rim` | Static, costs nothing per frame |
| Wide soft shadow | Yes: `depth` | One static shadow |
| Sheen | Yes, one, fixed | A static gradient |
| Selected state as clearer glass | Yes: the lozenge | Replaces the flat wash |
| Large, nested corners | Yes: `r-xl` outside, `r-md` inside | Inner radius = outer minus gap |
| Glass only on floating layers, never stacked | Yes | Both sources warn against glass on glass |
| Refraction at the edges | No | A desktop window cannot run a displacement pass over the system backdrop, and it would be a shader pass every frame |
| Highlights that move with tilt or pointer | No | Animation; outside the resource budget |
| Tint that adapts to content | Partly | The thin tint and saturation let the wallpaper colour the pane; nothing is computed |
| Merging, morphing shapes | No | Animation |

## If refraction is ever wanted

The only affordable place is **inside** the window, where procast owns the pixels: for example a selected lozenge that lenses the list behind it. That would need a precomputed displacement map per lozenge size and a custom shader, and it would be the single most expensive thing in the interface. Not recommended for a launcher.

## Legibility rules both sources agree on

- Keep body text off thin glass, or level the backdrop first. procast levels it.
- Test each surface against the busiest background it can land on.
- Honour the system's reduced-transparency and high-contrast settings with a solid fallback.
