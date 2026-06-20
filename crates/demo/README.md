# Knightty Demo

`knightty-demo` is a standalone terminal animation workload for checking Knightty's PTY, VT parsing, cell updates, Unicode glyph rendering, color attributes, and GPU rendering path with regular terminal output.

The animation is an original low-color silhouette loop. It does not include frames, images, video, fonts, or assets from existing works.

Phase 1.6 keeps the terminal workload from Phase 1 but uses authored SVG key poses converted ahead of time into an indexed `.kfa` asset. The knight body and logo are stored as raster frames, while the cape is stored as fixed-topology vector layers and morphed at runtime with deterministic timing offsets. Runtime execution does not parse SVG or decode PNG/GIF/video data.

## Run

Use a release build for performance checks. Debug builds spend too much time in Rust debug instrumentation to represent terminal throughput.

```bash
cargo run -p knightty-demo --release -- --fps 60
```

Run it from inside Knightty to send the workload through Knightty's normal shell and PTY path.

## Controls

- `q`: exit
- `Escape`: exit
- `Space`: pause or resume

## Options

```bash
cargo run -p knightty-demo --release -- --fps 30 --duration 10
cargo run -p knightty-demo --release -- --fps 60 --duration 10
cargo run -p knightty-demo --release -- --fps 120 --duration 10
cargo run -p knightty-demo --release -- --fps 0 --duration 10
```

- `--fps <number>`: target FPS. `0` means uncapped. The default is `60`.
- `--duration <seconds>`: stop automatically after the given run duration.
- `--no-stats`: skip the final performance report.
- `--help`: print CLI help.

## Assets

Source assets live in `crates/demo/assets/source/`. They are original Knightty SVG key poses using a fixed 320 x 180 viewBox and the demo palette only. The converter downsamples them to the 160 x 90 runtime reference canvas. The generated runtime asset is `crates/demo/assets/generated/animation.kfa`; do not edit it by hand.

The Phase 1.6 cape is separated from the character frames. Cape sources must keep the same six polygon layers across every pose: `cape_far`, `cape_main`, `cape_near`, `cape_lower`, `ribbon_far`, and `ribbon_near`. Each layer keeps the same vertex count, vertex order, winding, and anchor point so the runtime can morph corresponding vertices without a generic SVG path morph engine.

Regenerate assets after changing SVG sources:

```bash
cargo run -p xtask -- demo-assets build
```

Generate visual previews and contact sheets:

```bash
cargo run -p xtask -- demo-assets preview
```

Preview PNGs are written under `target/knightty-demo-preview/`, including cape, character, terminal-scale, 80 x 45, and Half Block-style previews.

If Aseprite or another drawing tool is used for draft work, export the final shapes to SVG and keep the source within the documented palette and external-reference restrictions. Do not trace or convert external artwork, screenshots, fonts, logos, videos, GIFs, or internet-sourced images into the repository.

## Notes

Benchmark numbers depend on terminal size, OS, GPU, font, target FPS, compositor behavior, and whether the demo is run through Knightty or another terminal.

Phase 1 uses full-frame rewrites only. Future phases can add differential cell updates or alternative encodings, but this crate intentionally keeps the first workload focused on half-block Unicode plus ANSI True Color.

For Phase 1.6 performance comparisons, run the same terminal size, FPS, and duration before and after asset changes, then compare the final metrics: rendered frames, dropped frames, bytes written, average bytes per frame, encode time, and frame time percentiles. The cape morph uses preallocated buffers and should not add per-frame decompression or large allocations.
