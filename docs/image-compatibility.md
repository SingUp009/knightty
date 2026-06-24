# Inline Image Compatibility

Knightty supports the Phase 4-F1 subset of the iTerm2 Inline Images Protocol and
the Phase 4-F6 direct-transmission, placement, and delete subset of the Kitty Graphics
Protocol.

## Supported

```text
OSC 1337 ; File=inline=1:<base64 PNG> BEL
OSC 1337 ; File=inline=1:<base64 PNG> ST
```

- Static PNG images only
- Base64 payloads
- Optional Base64 `name` metadata for diagnostics
- Optional `width` and `height` as positive cell counts or `auto`
- `preserveAspectRatio=1`
- Transparent PNG alpha blending
- Cell-relative placement at the current cursor
- Normal output scrolling and viewport scrollback visibility
- Cell-based resize behavior

Kitty graphics commands use the following APC envelope:

```text
ESC _ G <control-data> ; <base64 image data> ESC \
```

Supported Kitty operations and keys:

- `a=T`: transmit and display an image
- `a=t`: transmit an image without displaying it
- `a=p`: place a previously transmitted image
- `a=d`: delete placements with `d=a/A`, `i/I`, `p/P`, `x/X`, `y/Y`, or `z/Z`
- PNG `f=100`, RGB `f=24`, RGBA `f=32`, and direct transmission `t=d`
- Source pixel dimensions `s` and `v`, required and non-zero for RGB/RGBA
- Single-chunk `m=0` and multipart `m=1` continuation followed by final `m=0`
- Client image IDs `i`, placement IDs `p`, cell dimensions `c` and `r`
- Placement source rectangles `x`, `y`, `w`, and `h` in source-image pixels
- Placement pixel offsets `X` and `Y` from the anchor cell's top-left corner
- Signed placement z-index `z`
- Cursor policy `C=0/1` and response policy `q=0/1/2`
- `OK`, `EINVAL`, `ENOENT`, `ENODATA`, `EBADPNG`, `E2BIG`, and `ENOSPC`

Kitty image IDs are mapped to Knightty-internal image IDs. A non-zero
`(i, p)` pair identifies one named placement and placing it again replaces that
placement. An omitted or zero `p` creates another anonymous placement. Reusing
an image ID for a successful transmission replaces the old image and removes
all of its old placements.

Raw `f=24` payloads contain tightly packed RGB bytes and are expanded to RGBA
with alpha 255. Raw `f=32` payloads contain tightly packed RGBA bytes; alpha is
kept as straight alpha without premultiplication. Their decoded payload length
must equal `s * v * 3` or `s * v * 4` exactly. PNG dimensions continue to come
from the PNG itself; numeric `s` and `v` values on `f=100` are accepted but
ignored. Omitting `f` uses the Kitty default `f=32` and therefore requires
`s` and `v`.

Source rectangles are placement-specific and reuse the complete uploaded GPU
texture. Omitted `x` and `y` default to zero. Omitted or zero `w` and `h` extend
to the image's right and bottom edges. A requested rectangle is intersected
with the source image: partial boundary overflow is clipped, while an empty
intersection or coordinate arithmetic overflow returns `EINVAL`. When `c` or
`r` is inferred, the resolved crop dimensions determine the aspect ratio.

Pixel offsets must satisfy `X < cell_width` and `Y < cell_height` when the
placement command is received. They may move image pixels into adjacent cells.
Offsets remain unchanged across resize and are applied to the newly calculated
anchor-cell position even if a smaller new cell makes an offset exceed its
dimensions.

Image placements are ordered by signed z-index, then client image ID (with a
missing ID treated as zero), then insertion order. Images with
`z < -1073741824` are below non-default cell backgrounds. Other negative-z
images and zero-z images are above cell backgrounds but below selection and
text. Positive-z images are above text but below underline, cursor, and final
overlay rendering.

Multipart transfers hold one bounded partial upload at a time. Only the first
chunk carries the image and placement controls; continuation chunks carry `m`
and optionally `q`. Each chunk is at most 4096 Base64 bytes, and non-final
chunks have a length divisible by four. Decoding and image replacement happen
only after the final chunk validates successfully.

Kitty delete selectors are case-sensitive. Lowercase selectors soft-delete
placements while retaining named image data for later `a=p` commands. Uppercase
selectors additionally delete only the image resources selected by that command
whose placements are no longer referenced anywhere in live screen or scrollback
history. `d=I,i=<id>` therefore releases transfer-only images and images whose
placements were removed by an earlier soft delete. It does not trigger a global
orphan sweep, so unrelated soft-deleted images remain reusable.

`d=i/I` selects an image by `i`; an optional `p` narrows it to one named
`(image ID, placement ID)` pair. `d=p/P` selects placements intersecting the
cell specified by `x,y`. `d=x/X` and `d=y/Y` select placements intersecting a
column or row, and `d=z/Z` selects an exact signed z-index. `d=a/A` selects
placements intersecting the live screen. Protocol `x,y` delete coordinates are
1-based screen-cell positions: `x=1,y=1` is converted to live logical cell
`(0,0)`. Selection always uses the active live screen, even while the user is
viewing scrollback, and never applies the viewport display offset. Source crop
rectangles and pixel offsets do not change delete intersection tests.
Delete selectors operate only on Kitty placements; iTerm2 inline-image
placements in the shared logical placement store are not affected.

Deleting a placement causes the renderer's existing requested-image cache
reconciliation to release any now-unused GPU texture. A later soft-deleted image
placement can upload the retained decoded pixels again. Any delete command also
aborts an incomplete multipart upload before applying the resolved delete plan.

For iTerm2 images, the cursor advances by the calculated row count using
block-style carriage-return/newline steps. Kitty `C=0` moves the cursor after
the placement rectangle, while `C=1` leaves it unchanged.

## Limits

Defaults are configurable under `[graphics]`:

```toml
[graphics]
enabled = true
max_encoded_bytes = 16777216
max_decoded_bytes = 134217728
max_width = 8192
max_height = 8192
max_pixels = 32000000
max_images = 128
max_gpu_bytes = 268435456
```

Invalid, malformed, unsupported, or oversized image payloads are never printed
as terminal text. iTerm2 failures are logged; Kitty failures produce a bounded
APC error response unless suppressed by `q`.

## Current Limitations

- iTerm2 transmission supports PNG only. Kitty transmission supports PNG and
  uncompressed RGB/RGBA; JPEG, GIF, WebP, and animation are not supported.
- Windows ConPTY can remove Kitty APC sequences before Knightty's PTY reader
  receives them. This was reproduced on Windows build `10.0.26200.7171`: the
  reader received the ASCII text immediately before and after the APC, but
  received zero `ESC _ G` starts and zero `ESC \` terminators. PowerShell
  `[Console]::Write`, `[Console]::Out.Write`, and
  `[Console]::OpenStandardOutput().Write(byte[])` all produced the same result.
  This is below the graphics router boundary; changing the Kitty parser or
  renderer cannot recover bytes that ConPTY removed.
- Pixel and percentage width/height units are rejected.
- `preserveAspectRatio=0` is rejected.
- Kitty files, temporary files, shared memory, Unicode placeholders, relative
  placement, and animation are not supported.
- iTerm2 file paths, downloads, multipart transfers, shared memory, and
  networking are not supported.
- Anonymous and iTerm2 images are retained while a placement remains in live
  terminal history. Named Kitty images also remain available after transfer-only
  commands and soft deletion. Pixel data is not serialized into persistent
  scrollback.
- iTerm2 image replacement and client-supplied IDs remain unsupported.
- Sixel is not supported.

## Smoke Test

Run Knightty, then execute:

```powershell
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-inline-png.ps1
```

The script emits a repository-local embedded PNG, followed by ordinary text.

For the Kitty direct-transmission path, run:

```powershell
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-kitty-png.ps1
```

With no arguments the script displays an embedded sample. To display an
arbitrary PNG, start Knightty from the repository and run the script inside its
shell:

```powershell
cargo run -p app --release

# Run this command inside the Knightty window.
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-kitty-png.ps1 `
  -Path "C:\path\image.png" -Columns 40
```

`-Rows` can set an explicit cell height; leaving it at zero preserves the PNG's
aspect ratio using the requested column width. `-ImageId` selects the non-zero
Kitty image ID used for replacement. The configured `max_encoded_bytes` applies
to the complete Base64 upload, not to each individual chunk.

On a Unix terminal, this command displays a 2x2 straight-alpha RGBA sample
(red, green, blue, white) through the raw direct-transmission path:

```sh
printf '\033_Ga=T,f=32,s=2,v=2,c=20,r=10,i=50;/wAA/wD/AP8AAP///////w==\033\\'
```

Windows ConPTY can remove the APC before Knightty receives it, so raw rendering
smoke tests must be performed on Unix. Windows validation uses the parser-to-
placement integration tests.

### Transport diagnostics

Set `KNIGHTTY_GRAPHICS_DIAGNOSTICS=1` before starting Knightty to inspect the
PTY boundary without logging the complete image payload:

```powershell
$env:KNIGHTTY_GRAPHICS_DIAGNOSTICS = "1"
cargo run -p app --release 2> knightty-graphics.log
```

The diagnostic records each raw PTY read's byte count and only its first and
last 16 bytes. It also records cumulative counts for `ESC _ G`, `ESC \`,
completed Kitty commands, successful image decodes, and created Kitty placements.

The three PowerShell output APIs can be compared through Windows ConPTY with:

```powershell
cargo test -p pty --test windows_conpty_apc -- --ignored --nocapture
```

On a Unix host, the raw PTY preservation E2E test is:

```sh
cargo test -p pty --test unix_apc -- --nocapture
```

This test was verified on 2026-06-21 under Arch Linux on WSL2
(`6.6.87.2-microsoft-standard-WSL2`). The Unix PTY preserved the complete
`q=0,m=0` 1x1 PNG APC byte-for-byte. The matching application smoke test also
passed, confirming the router, PNG decode, and placement path on Linux:

```sh
cargo test -p app \
  input::harness_tests::kitty_q0_m0_known_1x1_png_smoke_creates_a_placement \
  -- --exact
```
