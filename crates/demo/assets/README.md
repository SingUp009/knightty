# Knightty Demo Assets

Phase 1.6 uses authored SVG key poses for the knight body, the `KNIGHTTY` wordmark, and a separate fixed-topology cape rig. These source files are original Knightty assets. Do not trace, copy, convert, or include frames, logos, fonts, images, GIFs, or videos from existing works.

## Layout

- `source/`: hand-authored SVG key poses.
- `generated/animation.kfa`: deterministic KFA2 runtime asset generated from the SVG sources.

Do not edit files in `generated/` by hand. Regenerate them from `source/`.

## SVG Rules

Every SVG source must use:

- `viewBox="0 0 320 180"`
- no external images, fonts, `href`, `url(...)`, filters, masks, or text
- simple `polygon`, `rect`, or `circle` elements
- fixed palette colors only:
  - `#CDD6F4` foreground
  - `#B4BEFE` accent
  - `#6C7086` mid-tone
  - transparent or no shape for transparent pixels

Unknown colors fail the converter instead of being approximated.

Cape pose SVGs must contain exactly these polygon IDs:

- `cape_far`
- `cape_main`
- `cape_near`
- `cape_lower`
- `ribbon_far`
- `ribbon_near`

Across all cape poses, each layer must keep the same vertex count, vertex order, winding, fill color, and first-point anchor. The converter rejects missing layers, duplicate layers, out-of-bounds vertices, anchor drift, and topology mismatches.

## Commands

```bash
cargo run -p xtask -- demo-assets build
cargo run -p xtask -- demo-assets preview
```

Preview PNGs are written under `target/knightty-demo-preview/`, including:

- `cape-contact-sheet.png`
- `cape-motion-strip.png`
- `character-contact-sheet.png`
- `terminal-contact-sheet.png`
- `source-320x180.png`
- `logical-160x90.png`
- `small-80x45.png`
- `half-block-preview.png`

The runtime `knightty-demo` binary includes `generated/animation.kfa` with `include_bytes!`; it does not parse SVG or decode image files during normal execution. KFA2 stores body/logo raster frames and cape vector topology. The runtime decodes the asset once at startup and keeps the expanded frame and cape buffers for the animation loop.
